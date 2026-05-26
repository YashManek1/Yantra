//! # Code-Review Graph: Symbol Embedding Store
//!
//! Generates and stores dense vector embeddings for codebase symbols using
//! `fastembed` and `BAAI/bge-small-en-v1.5`. Embeddings are persisted in SQLite
//! and held in an in-memory vector cache after `embed_all` for zero-SQL warm
//! searches via brute-force cosine similarity.
//!
//! ## Input
//! - Active SQLite database connection (for `embed_all` and cold-start `search`)
//! - Text query string
//!
//! ## Output
//! - Dense vector embeddings stored in database and in-memory cache
//! - List of semantically matching symbols and similarity scores
//!
//! ## Related
//! - `forge-crg::seed` — calls `embed_query` and `search_with_embedding`
//! - `forge-crg::subgraph` — passes the embedding store to `extract_subgraph`

use rusqlite::{params, Connection};
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use yantra_core::SymbolId;
use std::str::FromStr;
use std::sync::Mutex;
use std::collections::HashMap;

pub struct EmbeddingStore {
    embedding_model: TextEmbedding,
    query_cache: Mutex<HashMap<String, Vec<f32>>>,
    vector_cache: Mutex<Vec<(SymbolId, Vec<f32>)>>,
}

impl EmbeddingStore {
    /// Creates a new `EmbeddingStore` loading the BGE-Small-EN-v1.5 model.
    pub fn new() -> anyhow::Result<Self> {
        let embedding_model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_show_download_progress(false),
        )?;
        Ok(Self {
            embedding_model,
            query_cache: Mutex::new(HashMap::new()),
            vector_cache: Mutex::new(Vec::new()),
        })
    }

    /// Embeds all symbols from the database, persists vectors to SQLite, and
    /// populates the in-memory `vector_cache` for fast subsequent searches.
    pub fn embed_all(&self, sqlite_connection: &Connection) -> anyhow::Result<()> {
        sqlite_connection.execute(
            "CREATE TABLE IF NOT EXISTS symbol_embeddings (
                symbol_id TEXT PRIMARY KEY,
                vector BLOB NOT NULL
            )",
            [],
        )?;

        let mut select_statement = sqlite_connection.prepare(
            "SELECT id, name, docstring FROM symbols WHERE kind != 'file'"
        )?;
        let symbol_rows = select_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;

        let mut symbol_identifiers: Vec<String> = Vec::new();
        let mut text_passages: Vec<String> = Vec::new();

        for row_result in symbol_rows {
            let (symbol_id_string, symbol_name, symbol_docstring) = row_result?;
            let docstring_content = symbol_docstring.unwrap_or_default();
            let passage_text = format!("passage: Symbol name: {}\nDocstring: {}", symbol_name, docstring_content);
            symbol_identifiers.push(symbol_id_string);
            text_passages.push(passage_text);
        }

        if !text_passages.is_empty() {
            let embeddings = self.embedding_model.embed(text_passages, None)?;
            let mut insert_statement = sqlite_connection.prepare(
                "INSERT OR REPLACE INTO symbol_embeddings (symbol_id, vector) VALUES (?1, ?2)"
            )?;

            let mut in_memory_vectors: Vec<(SymbolId, Vec<f32>)> = Vec::with_capacity(symbol_identifiers.len());

            for (index, symbol_id_string) in symbol_identifiers.into_iter().enumerate() {
                let embedding_vector = embeddings[index].clone();
                let vector_bytes = vector_to_bytes(&embedding_vector);
                insert_statement.execute(params![symbol_id_string, vector_bytes])?;
                if let Ok(symbol_identifier) = SymbolId::from_str(&symbol_id_string) {
                    in_memory_vectors.push((symbol_identifier, embedding_vector));
                }
            }

            if let Ok(mut cache) = self.vector_cache.lock() {
                *cache = in_memory_vectors;
            }
        }

        Ok(())
    }

    /// Embeds `query_text`, returning a cached vector if already computed.
    pub fn embed_query(&self, query_text: &str) -> anyhow::Result<Vec<f32>> {
        {
            if let Ok(cache) = self.query_cache.lock() {
                if let Some(cached_vector) = cache.get(query_text) {
                    return Ok(cached_vector.clone());
                }
            }
        }

        let formatted_query = format!("query: {}", query_text);
        let query_embeddings = self.embedding_model.embed(vec![formatted_query], None)?;
        let result_vector = query_embeddings[0].clone();

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
    pub fn search_with_embedding(&self, query_vector: &[f32], limit: usize) -> anyhow::Result<Vec<(SymbolId, f32)>> {
        let cache = self.vector_cache.lock()
            .map_err(|_| anyhow::anyhow!("vector_cache mutex poisoned"))?;

        if cache.is_empty() {
            return Err(anyhow::anyhow!(
                "EmbeddingStore vector cache is empty — call embed_all() before search_with_embedding()"
            ));
        }

        let mut search_results: Vec<(SymbolId, f32)> = cache.iter()
            .map(|(symbol_identifier, stored_vector)| {
                let similarity_score = compute_cosine_similarity(query_vector, stored_vector);
                (symbol_identifier.clone(), similarity_score)
            })
            .collect();

        search_results.sort_by(|first, second| second.1.partial_cmp(&first.1).unwrap());
        search_results.truncate(limit);

        Ok(search_results)
    }

    /// Searches for the top-`limit` symbols semantically similar to `query_text`.
    /// Lazily calls `embed_all` if embeddings have not yet been generated, loading
    /// from SQLite if they already exist on disk, or computing fresh if not.
    pub fn search(&self, sqlite_connection: &Connection, query_text: &str, limit: usize) -> anyhow::Result<Vec<(SymbolId, f32)>> {
        let cache_is_empty = self.vector_cache.lock()
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

            let existing_count: i64 = sqlite_connection.query_row(
                "SELECT COUNT(*) FROM symbol_embeddings",
                [],
                |row| row.get(0)
            ).unwrap_or(0);

            if existing_count == 0 {
                self.embed_all(sqlite_connection)?;
            } else {
                self.load_vectors_from_db(sqlite_connection)?;
            }
        }

        let query_vector = self.embed_query(query_text)?;
        self.search_with_embedding(&query_vector, limit)
    }

    fn load_vectors_from_db(&self, sqlite_connection: &Connection) -> anyhow::Result<()> {
        let mut select_statement = sqlite_connection.prepare(
            "SELECT symbol_id, vector FROM symbol_embeddings"
        )?;
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
    let mut dot_product = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for index in 0..vector_a.len() {
        let value_a = vector_a[index];
        let value_b = vector_b[index];
        dot_product += value_a * value_b;
        norm_a += value_a * value_a;
        norm_b += value_b * value_b;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
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
        let vector = vec![1.0f32, 0.0, 0.0];
        let similarity = compute_cosine_similarity(&vector, &vector);
        assert!((similarity - 1.0).abs() < 1e-6, "identical vectors should have similarity 1.0");
    }

    #[test]
    fn test_cosine_similarity_orthogonal_vectors() {
        let vector_a = vec![1.0f32, 0.0, 0.0];
        let vector_b = vec![0.0f32, 1.0, 0.0];
        let similarity = compute_cosine_similarity(&vector_a, &vector_b);
        assert!((similarity - 0.0).abs() < 1e-6, "orthogonal vectors should have similarity 0.0");
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let vector_zero = vec![0.0f32, 0.0, 0.0];
        let vector_a = vec![1.0f32, 0.0, 0.0];
        let similarity = compute_cosine_similarity(&vector_zero, &vector_a);
        assert_eq!(similarity, 0.0, "zero vector should yield similarity 0.0");
    }
}
