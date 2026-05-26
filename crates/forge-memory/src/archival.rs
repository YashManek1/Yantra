//! # forge-memory: Archival (Long-Term) Memory Tier
//!
//! Persists distilled knowledge shards (code chunks, research memos, skills,
//! session summaries) with brute-force cosine-similarity search over
//! character-trigram bag-of-words embeddings stored as SQLite BLOBs.
//!
//! ## Input
//! - `MemoryShard` values written by the Memory Consolidation agent or Night Mode
//! - A natural-language query string for similarity search
//!
//! ## Output
//! - Ranked `Vec<MemoryShard>` for injection into agent context prompts
//!
//! ## Related
//! - `forge-memory::working` — assembles shards into the prompt
//! - `forge-memory::heartbeat` — re-indexes changed content on a schedule

use std::path::Path;

use chrono::{DateTime, Utc};
use yantra_core::db::{apply_migrations, connection_pool, DbPool};

use crate::error::{MemoryError, MemoryResult};

const ARCHIVAL_MIGRATION: &str = "
CREATE TABLE IF NOT EXISTS memory_shards (
    id          TEXT PRIMARY KEY,
    shard_type  TEXT NOT NULL,
    source_ref  TEXT,
    content     TEXT NOT NULL,
    embedding   BLOB,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shards_type ON memory_shards(shard_type);
";

/// Number of float dimensions in the packed embedding vector.
const EMBEDDING_DIMS: usize = 64;

/// Category of a stored memory shard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShardType {
    /// A chunk of source code extracted from the CRG.
    CodeChunk,
    /// A prose research finding or analysis memo.
    ResearchMemo,
    /// A SKILL.md workflow document.
    Skill,
    /// A compressed end-of-session summary.
    SessionSummary,
}

impl ShardType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::CodeChunk => "code_chunk",
            Self::ResearchMemo => "research_memo",
            Self::Skill => "skill",
            Self::SessionSummary => "session_summary",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "code_chunk" => Self::CodeChunk,
            "research_memo" => Self::ResearchMemo,
            "skill" => Self::Skill,
            _ => Self::SessionSummary,
        }
    }
}

/// A unit of distilled knowledge stored in the archival tier.
#[derive(Debug, Clone)]
pub struct MemoryShard {
    /// Unique shard identifier (UUID).
    pub id: String,
    /// Category that determines retrieval priority.
    pub shard_type: ShardType,
    /// Pointer to the source (file path, session ID, or URL).
    pub source_ref: Option<String>,
    /// Raw distilled content.
    pub content: String,
    /// When the shard was stored.
    pub created_at: DateTime<Utc>,
}

/// Long-term archival memory store backed by SQLite.
pub struct ArchivalStore {
    database_pool: DbPool,
}

impl ArchivalStore {
    /// Opens or creates the archival database at `db_path`.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database open or migration failure.
    pub fn new(db_path: &Path) -> MemoryResult<Self> {
        if let Some(parent_dir) = db_path.parent() {
            std::fs::create_dir_all(parent_dir).map_err(|source| MemoryError::DirectoryCreate {
                path: parent_dir.display().to_string(),
                source,
            })?;
        }
        let database_pool = connection_pool(db_path)?;
        let database_guard = database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        apply_migrations(&database_guard, &[ARCHIVAL_MIGRATION])?;
        drop(database_guard);
        Ok(Self { database_pool })
    }

    /// Computes an embedding for `shard.content` and stores the shard.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database write failure.
    pub fn index(&self, shard: &MemoryShard) -> MemoryResult<()> {
        let embedding_vector = compute_embedding(&shard.content);
        let embedding_blob = pack_embedding(&embedding_vector);

        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        database_guard.execute(
            "INSERT OR REPLACE INTO memory_shards
             (id, shard_type, source_ref, content, embedding, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                shard.id,
                shard.shard_type.as_str(),
                shard.source_ref,
                shard.content,
                embedding_blob,
                shard.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Returns the `k` shards most similar to `query` by cosine similarity.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database read failure.
    pub fn search(&self, query: &str, k: usize) -> MemoryResult<Vec<MemoryShard>> {
        let query_embedding = compute_embedding(query);

        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;

        let mut statement = database_guard.prepare(
            "SELECT id, shard_type, source_ref, content, embedding, created_at
             FROM memory_shards",
        )?;

        let mut scored_shards: Vec<(f32, MemoryShard)> = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .filter_map(|row_result| {
                let (id, shard_type_str, source_ref, content, embedding_blob, created_at_str) =
                    row_result.ok()?;
                let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                    .ok()?
                    .with_timezone(&Utc);

                let similarity_score = embedding_blob
                    .as_deref()
                    .and_then(unpack_embedding)
                    .map_or(0.0_f32, |stored_embedding| {
                        cosine_similarity(&query_embedding, &stored_embedding)
                    });

                let shard = MemoryShard {
                    id,
                    shard_type: ShardType::from_str(&shard_type_str),
                    source_ref,
                    content,
                    created_at,
                };
                Some((similarity_score, shard))
            })
            .collect();

        scored_shards.sort_by(|(score_a, _), (score_b, _)| {
            score_b
                .partial_cmp(score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(scored_shards
            .into_iter()
            .take(k)
            .map(|(_, shard)| shard)
            .collect())
    }
}

/// Computes a 64-dimensional character-trigram hash bag-of-words embedding.
///
/// Each character trigram in the text is hashed into one of 64 bins. The
/// resulting frequency vector is L2-normalized to unit length, enabling
/// cosine similarity via dot product.
fn compute_embedding(text: &str) -> [f32; EMBEDDING_DIMS] {
    let mut bucket_counts = [0.0_f32; EMBEDDING_DIMS];

    let chars: Vec<char> = text.chars().collect();
    for trigram_window in chars.windows(3) {
        let trigram_hash = simple_hash(trigram_window);
        let bucket_index = trigram_hash % EMBEDDING_DIMS;
        bucket_counts[bucket_index] += 1.0;
    }

    if chars.len() < 3 {
        for single_char in &chars {
            let char_hash = simple_hash(&[*single_char]);
            let bucket_index = char_hash % EMBEDDING_DIMS;
            bucket_counts[bucket_index] += 1.0;
        }
    }

    l2_normalize(&mut bucket_counts);
    bucket_counts
}

fn simple_hash(chars: &[char]) -> usize {
    chars
        .iter()
        .fold(14_695_981_039_346_656_037_usize, |hash, character| {
            hash.wrapping_mul(1_099_511_628_211)
                .wrapping_add(*character as usize)
        })
}

fn l2_normalize(vector: &mut [f32; EMBEDDING_DIMS]) {
    let magnitude: f32 = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if magnitude > 1e-9 {
        for value in vector.iter_mut() {
            *value /= magnitude;
        }
    }
}

fn cosine_similarity(vector_a: &[f32; EMBEDDING_DIMS], vector_b: &[f32; EMBEDDING_DIMS]) -> f32 {
    vector_a
        .iter()
        .zip(vector_b.iter())
        .map(|(value_a, value_b)| value_a * value_b)
        .sum()
}

fn pack_embedding(vector: &[f32; EMBEDDING_DIMS]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn unpack_embedding(blob: &[u8]) -> Option<[f32; EMBEDDING_DIMS]> {
    if blob.len() != EMBEDDING_DIMS * 4 {
        return None;
    }
    let mut result = [0.0_f32; EMBEDDING_DIMS];
    for (float_index, float_value) in result.iter_mut().enumerate() {
        let byte_offset = float_index * 4;
        *float_value = f32::from_le_bytes(
            blob[byte_offset..byte_offset + 4]
                .try_into()
                .expect("slice is exactly 4 bytes"),
        );
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use yantra_core::TaskId;

    use super::*;

    fn temp_db_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("yantra-archival-test-{}", TaskId::new()))
            .join("archival.sqlite")
    }

    fn sample_shard(content: &str, shard_type: ShardType) -> MemoryShard {
        MemoryShard {
            id: TaskId::new().to_string(),
            shard_type,
            source_ref: None,
            content: content.to_owned(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn index_and_search_returns_most_similar_shard() {
        let archival_store = ArchivalStore::new(&temp_db_path()).expect("store created");

        let rust_shard = sample_shard(
            "fn authenticate_user(token: &str) -> Result<User, AuthError>",
            ShardType::CodeChunk,
        );
        let python_shard = sample_shard(
            "def compute_fibonacci(number: int) -> int",
            ShardType::ResearchMemo,
        );
        archival_store
            .index(&rust_shard)
            .expect("rust shard indexed");
        archival_store
            .index(&python_shard)
            .expect("python shard indexed");

        let results = archival_store
            .search("authenticate user token", 1)
            .expect("search succeeds");
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("authenticate"),
            "top result should be the auth shard"
        );
    }

    #[test]
    fn search_returns_at_most_k_results() {
        let archival_store = ArchivalStore::new(&temp_db_path()).expect("store created");
        for index in 0..5 {
            let shard = sample_shard(
                &format!("content item number {index}"),
                ShardType::SessionSummary,
            );
            archival_store.index(&shard).expect("shard indexed");
        }
        let results = archival_store
            .search("content item", 3)
            .expect("search succeeds");
        assert!(results.len() <= 3, "must return at most k results");
    }

    #[test]
    fn embedding_pack_unpack_round_trip() {
        let original = compute_embedding("hello world function returns token");
        let packed = pack_embedding(&original);
        let unpacked = unpack_embedding(&packed).expect("unpack succeeds");
        for (original_value, unpacked_value) in original.iter().zip(unpacked.iter()) {
            assert!(
                (original_value - unpacked_value).abs() < 1e-6,
                "values must survive f32 round-trip"
            );
        }
    }

    #[test]
    fn cosine_similarity_of_identical_embeddings_is_one() {
        let embedding = compute_embedding("implement JWT rotation for auth service");
        let similarity = cosine_similarity(&embedding, &embedding);
        assert!(
            (similarity - 1.0).abs() < 1e-5,
            "identical vectors must have cosine similarity 1.0"
        );
    }
}
