//! # forge-memory: Truth Vault
//!
//! Stores and retrieves opaque YAML content for `SourceTruth` artifacts
//! indexed by task ID and SHA-256 content hash. The vault acts as the
//! authoritative reference store that `forge-stvp` reads during token
//! verification and truth drift checking.
//!
//! ## Input
//! - YAML-serialized `SourceTruth` content (serialized by the caller — typically
//!   `forge-stvp`) keyed by `TaskId`
//! - A content hash for tamper detection
//!
//! ## Output
//! - YAML content retrieved by `task_id` or content hash
//!
//! ## Related
//! - `forge-stvp::source_truth` — serializes and calls `TruthVault::store`
//! - `forge-memory::recall` — shares the same SQLite database

use std::path::Path;

use chrono::Utc;
use rusqlite::OptionalExtension;
use yantra_core::db::{apply_migrations, connection_pool, DbPool};
use yantra_core::TaskId;

use crate::error::{MemoryError, MemoryResult};

const VAULT_MIGRATION: &str = "
CREATE TABLE IF NOT EXISTS truth_vault (
    task_id      TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    yaml_content TEXT NOT NULL,
    stored_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_vault_hash ON truth_vault(content_hash);
";

/// SHA-256 hex digest of stored YAML content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TruthHash(pub String);

impl TruthHash {
    /// Returns the hex string of the hash.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TruthHash {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Persistent store for YAML-serialized `SourceTruth` artifacts.
pub struct TruthVault {
    database_pool: DbPool,
}

impl TruthVault {
    /// Opens or creates the truth vault database at `db_path`.
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
        apply_migrations(&database_guard, &[VAULT_MIGRATION])?;
        drop(database_guard);
        Ok(Self { database_pool })
    }

    /// Stores `yaml_content` indexed by `task_id` and returns its content hash.
    ///
    /// If a truth for this `task_id` already exists, it is replaced.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database write failure.
    pub fn store(&self, task_id: TaskId, yaml_content: &str) -> MemoryResult<TruthHash> {
        let content_hash = sha256_hex(yaml_content.as_bytes());
        let stored_at = Utc::now().to_rfc3339();
        let task_id_string = task_id.to_string();

        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        database_guard.execute(
            "INSERT OR REPLACE INTO truth_vault (task_id, content_hash, yaml_content, stored_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![task_id_string, content_hash, yaml_content, stored_at],
        )?;

        Ok(TruthHash(content_hash))
    }

    /// Returns the YAML content for `task_id`, or `None` if not found.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database read failure.
    pub fn get_by_task_id(&self, task_id: TaskId) -> MemoryResult<Option<String>> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        let yaml_content = database_guard
            .query_row(
                "SELECT yaml_content FROM truth_vault WHERE task_id = ?1",
                rusqlite::params![task_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(yaml_content)
    }

    /// Returns the YAML content matching `hash`, or `None` if not found.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database read failure.
    pub fn get_by_hash(&self, hash: &TruthHash) -> MemoryResult<Option<String>> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        let yaml_content = database_guard
            .query_row(
                "SELECT yaml_content FROM truth_vault WHERE content_hash = ?1",
                rusqlite::params![hash.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(yaml_content)
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    let digest = ring::digest::digest(&ring::digest::SHA256, data);
    digest
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut hex_string, byte| {
            write!(hex_string, "{byte:02x}").expect("writing to a String never fails");
            hex_string
        })
}

#[cfg(test)]
mod tests {
    use yantra_core::TaskId;

    use super::*;

    fn temp_db_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("yantra-vault-test-{}", TaskId::new()))
            .join("vault.sqlite")
    }

    #[test]
    fn store_and_get_by_task_id_round_trip() {
        let vault = TruthVault::new(&temp_db_path()).expect("vault created");
        let task_id = TaskId::new();
        let yaml_content = "task_id: abc\ndescription: implement rate limiting\n";

        let truth_hash = vault.store(task_id, yaml_content).expect("store succeeds");
        assert!(!truth_hash.as_str().is_empty());

        let retrieved = vault
            .get_by_task_id(task_id)
            .expect("query succeeds")
            .expect("content present");
        assert_eq!(retrieved, yaml_content);
    }

    #[test]
    fn get_by_hash_retrieves_stored_content() {
        let vault = TruthVault::new(&temp_db_path()).expect("vault created");
        let task_id = TaskId::new();
        let yaml_content = "description: add JWT rotation\nstrictness: Strict\n";

        let truth_hash = vault.store(task_id, yaml_content).expect("store succeeds");
        let retrieved = vault
            .get_by_hash(&truth_hash)
            .expect("query succeeds")
            .expect("content present");
        assert_eq!(retrieved, yaml_content);
    }

    #[test]
    fn get_by_task_id_returns_none_for_missing_task() {
        let vault = TruthVault::new(&temp_db_path()).expect("vault created");
        let missing_id = TaskId::new();
        let result = vault.get_by_task_id(missing_id).expect("query succeeds");
        assert!(result.is_none());
    }

    #[test]
    fn same_content_produces_same_hash() {
        let vault = TruthVault::new(&temp_db_path()).expect("vault created");
        let yaml_content = "description: deterministic content\n";

        let hash_a = vault
            .store(TaskId::new(), yaml_content)
            .expect("first store");
        let hash_b = vault
            .store(TaskId::new(), yaml_content)
            .expect("second store");
        assert_eq!(hash_a, hash_b, "same content must yield same hash");
    }
}
