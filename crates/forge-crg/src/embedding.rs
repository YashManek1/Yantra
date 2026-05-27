//! # Code-Review Graph: Symbol Embedding Store
//!
//! Generates and stores dense vector embeddings for codebase symbols using
//! `fastembed` and `BAAI/bge-small-en-v1.5`. Embeddings are persisted in `SQLite`
//! and held in an in-memory vector cache after `embed_all` for zero-SQL warm
//! searches via brute-force cosine similarity.
//!
//! ## Input
//! - Active `SQLite` database connection (for `embed_all` and cold-start `search`)
//! - Text query string
//!
//! ## Output
//! - Dense vector embeddings stored in database and in-memory cache
//! - List of semantically matching symbols and similarity scores
//!
//! ## Related
//! - `forge-crg::seed` — calls `embed_query` and `search_with_embedding`
//! - `forge-crg::subgraph` — passes the embedding store to `extract_subgraph`

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::{params, Connection};
use yantra_core::SymbolId;

pub struct EmbeddingStore {
    embedding_model: TextEmbedding,
    query_cache: Mutex<HashMap<String, Vec<f32>>>,
    vector_cache: Mutex<Vec<(SymbolId, Vec<f32>)>>,
}

impl EmbeddingStore {
    /// Creates a new `EmbeddingStore` loading the BGE-Small-EN-v1.5 model.
    ///
    /// # Errors
    ///
    /// Returns an error if the fastembed model fails to initialise.
    pub fn new() -> anyhow::Result<Self> {
        let mut attempts = 0;
        let embedding_model = loop {
            match TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(false),
            ) {
                Ok(model) => break model,
                Err(error) => {
                    attempts += 1;
                    if attempts >= 10 {
                        return Err(anyhow::anyhow!(
                            "Failed to initialise TextEmbedding after 10 attempts: {error}"
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        };
        Ok(Self {
            embedding_model,
            query_cache: Mutex::new(HashMap::new()),
            vector_cache: Mutex::new(Vec::new()),
        })
    }

    /// Embeds all symbols from the database, persists vectors to `SQLite`, and
    /// populates the in-memory `vector_cache` for fast subsequent searches.
    ///
    /// # Errors
    ///
    /// Returns an error if any `SQLite` operation fails or the embedding model
    /// returns an error.
    pub fn embed_all(&self, sqlite_connection: &Connection) -> anyhow::Result<()> {
        sqlite_connection.execute(
            "CREATE TABLE IF NOT EXISTS symbol_embeddings (
                symbol_id TEXT PRIMARY KEY,
                vector BLOB NOT NULL
            )",
            [],
        )?;

        let mut select_statement = sqlite_connection.prepare_cached(
            "SELECT s.id, s.name, s.docstring, s.kind, s.connectivity_score, f.path
             FROM symbols s
             JOIN files f ON s.file_id = f.id
             LEFT JOIN symbol_embeddings e ON s.id = e.symbol_id
             WHERE s.kind != 'file' AND e.symbol_id IS NULL",
        )?;
        let symbol_rows = select_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i32>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;

        let mut symbol_identifiers: Vec<String> = Vec::new();
        let mut passage_texts: Vec<String> = Vec::new();

        for row_result in symbol_rows {
            let (
                symbol_identifier_string,
                symbol_name,
                symbol_docstring,
                symbol_kind,
                connectivity_score,
                file_path,
            ) = row_result?;
            let docstring_content = symbol_docstring.unwrap_or_default();
            let text_passage_content = format!(
                "passage: [{symbol_kind}] {symbol_name} (connectivity: {connectivity_score}, file: {file_path})\nDocstring: {docstring_content}"
            );
            symbol_identifiers.push(symbol_identifier_string);
            passage_texts.push(text_passage_content);
        }

        if !passage_texts.is_empty() {
            let embedding_vectors = self.embedding_model.embed(passage_texts, None)?;

            let in_tx = !sqlite_connection.is_autocommit();
            if !in_tx {
                sqlite_connection.execute("BEGIN TRANSACTION", [])?;
            }

            let result = (|| -> anyhow::Result<()> {
                let mut insert_statement = sqlite_connection.prepare_cached(
                    "INSERT OR REPLACE INTO symbol_embeddings (symbol_id, vector) VALUES (?1, ?2)",
                )?;

                for (symbol_identifier_string, embedding_vector) in
                    symbol_identifiers.into_iter().zip(embedding_vectors)
                {
                    let vector_bytes = vector_to_bytes(&embedding_vector);
                    insert_statement.execute(params![symbol_identifier_string, vector_bytes])?;
                }
                Ok(())
            })();

            match result {
                Ok(()) => {
                    if !in_tx {
                        sqlite_connection.execute("COMMIT", [])?;
                    }
                }
                Err(error) => {
                    if !in_tx {
                        sqlite_connection.execute("ROLLBACK", []).ok();
                    }
                    return Err(error);
                }
            }
        }

        self.load_vectors_from_db(sqlite_connection)?;

        Ok(())
    }

    /// Embeds `query_text`, returning a cached vector if already computed.
    ///
    /// # Errors
    ///
    /// Returns an error if the embedding model fails or returns an empty result.
    pub fn embed_query(&self, query_text: &str) -> anyhow::Result<Vec<f32>> {
        {
            if let Ok(cache) = self.query_cache.lock() {
                if let Some(cached_vector) = cache.get(query_text) {
                    return Ok(cached_vector.clone());
                }
            }
        }

        let expanded_query_text = expand_query_keywords(query_text);
        let formatted_query = format!("query: {expanded_query_text}");
        let mut query_embeddings = self.embedding_model.embed(vec![formatted_query], None)?;
        let result_vector = query_embeddings
            .pop()
            .ok_or_else(|| anyhow::anyhow!("embedding model returned empty result for query"))?;

        {
            if let Ok(mut cache) = self.query_cache.lock() {
                cache.insert(query_text.to_string(), result_vector.clone());
            }
        }

        Ok(result_vector)
    }

    /// Returns the top-`limit` symbols by cosine similarity against `query_vector`.
    /// Uses the in-memory vector cache (populated by `embed_all`). Returns an error
    /// if `embed_all` has not been called yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the vector cache mutex is poisoned or the cache is empty.
    pub fn search_with_embedding(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<(SymbolId, f32)>> {
        let cache = self.vector_cache.lock().map_err(|poison_error| {
            anyhow::anyhow!("vector_cache mutex poisoned: {poison_error}")
        })?;

        if cache.is_empty() {
            return Err(anyhow::anyhow!(
                "EmbeddingStore vector cache is empty — call embed_all() before search_with_embedding()"
            ));
        }

        let mut search_results: Vec<(SymbolId, f32)> = cache
            .iter()
            .map(|(symbol_identifier, stored_vector)| {
                let similarity_score = compute_cosine_similarity(query_vector, stored_vector);
                (symbol_identifier.clone(), similarity_score)
            })
            .collect();

        search_results.sort_by(|first, second| second.1.total_cmp(&first.1));
        search_results.truncate(limit);

        Ok(search_results)
    }

    /// Searches for the top-`limit` symbols semantically similar to `query_text`.
    /// Lazily calls `embed_all` if embeddings have not yet been generated, loading
    /// from `SQLite` if they already exist on disk, or computing fresh if not.
    ///
    /// # Errors
    ///
    /// Returns an error if `SQLite` operations fail, the embedding model errors, or
    /// the vector cache mutex is poisoned.
    pub fn search(
        &self,
        sqlite_connection: &Connection,
        query_text: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(SymbolId, f32)>> {
        let cache_is_empty = self
            .vector_cache
            .lock()
            .map(|cache| cache.is_empty())
            .unwrap_or(true);

        if cache_is_empty {
            sqlite_connection.execute(
                "CREATE TABLE IF NOT EXISTS symbol_embeddings (
                    symbol_id TEXT PRIMARY KEY,
                    vector BLOB NOT NULL
                )",
                [],
            )?;

            let existing_count: i64 = sqlite_connection
                .query_row("SELECT COUNT(*) FROM symbol_embeddings", [], |row| {
                    row.get(0)
                })
                .unwrap_or(0);

            if existing_count == 0 {
                self.embed_all(sqlite_connection)?;
            } else {
                self.load_vectors_from_db(sqlite_connection)?;
            }
        }

        let query_vector = self.embed_query(query_text)?;
        self.search_with_embedding(&query_vector, limit)
    }

    pub fn load_vectors_from_db(&self, sqlite_connection: &Connection) -> anyhow::Result<()> {
        let mut select_statement =
            sqlite_connection.prepare("SELECT symbol_id, vector FROM symbol_embeddings")?;
        let embedding_rows = select_statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;

        let mut in_memory_vectors: Vec<(SymbolId, Vec<f32>)> = Vec::new();
        for row_result in embedding_rows {
            let (symbol_id_string, vector_bytes) = row_result?;
            if let Ok(symbol_identifier) = SymbolId::from_str(&symbol_id_string) {
                in_memory_vectors.push((symbol_identifier, bytes_to_vector(&vector_bytes)));
            }
        }

        if let Ok(mut cache) = self.vector_cache.lock() {
            *cache = in_memory_vectors;
        }

        Ok(())
    }
}

fn expand_query_keywords(query_text: &str) -> String {
    let lowercase_query = query_text.to_lowercase();
    let mut words_list: Vec<&str> = lowercase_query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|word| !word.is_empty())
        .collect();

    let stopwords_set: std::collections::HashSet<&str> = [
        "where", "should", "how", "does", "what", "can", "the", "and", "for", "you", "file",
        "with", "this", "that", "from", "into", "onto", "your", "under", "over", "about", "there",
        "their", "here", "been", "have", "were",
    ]
    .iter()
    .copied()
    .collect();

    words_list.retain(|word| !stopwords_set.contains(word) && word.len() >= 2);

    let mut result_string = query_text.to_string();
    if !words_list.is_empty() {
        result_string.push(' ');
        result_string.push_str(&words_list.join(" "));
    }
    result_string.push_str(" struct impl trait interface function class definition module");
    result_string
}

fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn bytes_to_vector(bytes: &[u8]) -> Vec<f32> {
    let mut vector = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let array: [u8; 4] = chunk.try_into().unwrap();
        vector.push(f32::from_le_bytes(array));
    }
    vector
}

fn compute_cosine_similarity(vector_a: &[f32], vector_b: &[f32]) -> f32 {
    let mut dot_product = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;
    for (value_a, value_b) in vector_a.iter().zip(vector_b.iter()) {
        dot_product += value_a * value_b;
        norm_a += value_a * value_a;
        norm_b += value_b * value_b;
    }
    if norm_a < f32::EPSILON || norm_b < f32::EPSILON {
        0.0
    } else {
        dot_product / (norm_a.sqrt() * norm_b.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical_vectors() {
        let vector = vec![1.0_f32, 0.0, 0.0];
        let similarity = compute_cosine_similarity(&vector, &vector);
        assert!(
            (similarity - 1.0).abs() < 1e-6,
            "identical vectors should have similarity 1.0"
        );
    }

    #[test]
    fn test_cosine_similarity_orthogonal_vectors() {
        let vector_a = vec![1.0_f32, 0.0, 0.0];
        let vector_b = vec![0.0_f32, 1.0, 0.0];
        let similarity = compute_cosine_similarity(&vector_a, &vector_b);
        assert!(
            similarity.abs() < 1e-6,
            "orthogonal vectors should have similarity 0.0"
        );
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let vector_zero = vec![0.0_f32, 0.0, 0.0];
        let vector_a = vec![1.0_f32, 0.0, 0.0];
        let similarity = compute_cosine_similarity(&vector_zero, &vector_a);
        assert!(
            similarity.abs() < f32::EPSILON,
            "zero vector should yield similarity 0.0"
        );
    }
}
