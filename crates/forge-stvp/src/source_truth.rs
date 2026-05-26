//! # forge-stvp: Source-Truth Artifact
//!
//! Defines `SourceTruth`, the validated task specification produced by the
//! `Interrogator`. Provides YAML serialization, disk storage under
//! `.yantra/source_truth/{task_id}.yaml`, and SHA-256 content-hash verification
//! backed by a SQLite manifest database.
//!
//! ## Input
//! - A completed set of questionnaire answers and task metadata
//! - A project root path for filesystem operations
//!
//! ## Output
//! - `SourceTruth` values serialized as YAML on disk
//! - A content hash stored in `.yantra/source_truth/manifest.sqlite` for
//!   tamper detection on subsequent loads
//!
//! ## Related
//! - `forge-stvp::interrogator` — constructs and stores `SourceTruth` values
//! - `forge-stvp::error` — surfaces I/O and hash-verification failures
//! - `forge-core::db` — provides the SQLite connection pool and migration helpers

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use ring::digest;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use yantra_core::db::{apply_migrations, connection_pool};
use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};

use crate::error::StvpError;

const MANIFEST_DB_FILE: &str = "manifest.sqlite";

const HASH_TABLE_MIGRATION: &str = "
CREATE TABLE IF NOT EXISTS truth_hashes (
    task_id      TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    stored_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
)";

/// The fully validated task specification produced by STVP interrogation.
///
/// Serializes to a canonical YAML artifact stored immutably in
/// `.yantra/source_truth/{task_id}.yaml`. The content hash is stored
/// separately in `manifest.sqlite` and verified on every load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTruth {
    /// Unique identifier for this task.
    pub task_id: TaskId,
    /// Classification of the task, used to select validation strictness.
    pub task_class: TaskClass,
    /// STVP strictness mode applied when validating this task.
    pub strictness: Strictness,
    /// The original natural-language task description from the user.
    pub description: String,
    /// Timestamp at which this artifact was created.
    pub created_at: DateTime<Utc>,
    /// Question-ID to answer-text map produced by the questionnaire.
    ///
    /// `BTreeMap` guarantees stable YAML key order, which is required for
    /// deterministic content-hash computation.
    pub answers: BTreeMap<String, String>,
}

impl SourceTruth {
    /// Serializes this artifact to a YAML string.
    ///
    /// # Errors
    ///
    /// Returns `StvpError::Yaml` if serialization fails.
    pub fn to_yaml(&self) -> Result<String, StvpError> {
        serde_yaml::to_string(self).map_err(StvpError::Yaml)
    }

    /// Deserializes a `SourceTruth` from a YAML string.
    ///
    /// # Errors
    ///
    /// Returns `StvpError::Yaml` if the YAML is malformed or missing required
    /// fields.
    pub fn from_yaml(yaml_text: &str) -> Result<Self, StvpError> {
        serde_yaml::from_str(yaml_text).map_err(StvpError::Yaml)
    }

    /// Writes this artifact to `<truth_dir>/<task_id>.yaml` and records the
    /// SHA-256 content hash in the manifest SQLite database.
    ///
    /// Returns the hex-encoded content hash.
    ///
    /// # Errors
    ///
    /// Returns `StvpError` on directory creation failure, YAML serialization
    /// failure, file-write failure, or SQLite failure.
    pub fn store(project_root: &ProjectRoot, truth: &SourceTruth) -> Result<String, StvpError> {
        let truth_directory = truth_directory(project_root);
        std::fs::create_dir_all(&truth_directory).map_err(|source| StvpError::DirectoryCreate {
            path: truth_directory.display().to_string(),
            source,
        })?;

        let yaml_content = truth.to_yaml()?;
        let content_hash = sha256_hex(yaml_content.as_bytes());

        let yaml_path = truth_directory.join(format!("{}.yaml", truth.task_id));
        std::fs::write(&yaml_path, &yaml_content).map_err(|source| StvpError::FileWrite {
            path: yaml_path.display().to_string(),
            source,
        })?;

        store_hash(&truth_directory, truth.task_id, &content_hash)?;

        Ok(content_hash)
    }

    /// Loads and hash-verifies the `SourceTruth` artifact for `task_id`.
    ///
    /// # Errors
    ///
    /// Returns `StvpError::TruthNotFound` when the YAML file does not exist,
    /// `StvpError::HashMismatch` when the content does not match the stored
    /// hash, and other `StvpError` variants for I/O or parse failures.
    pub fn load(project_root: &ProjectRoot, task_id: TaskId) -> Result<SourceTruth, StvpError> {
        let truth_directory = truth_directory(project_root);
        let yaml_path = truth_directory.join(format!("{task_id}.yaml"));

        if !yaml_path.exists() {
            return Err(StvpError::TruthNotFound {
                task_id: task_id.to_string(),
            });
        }

        let yaml_content =
            std::fs::read_to_string(&yaml_path).map_err(|source| StvpError::FileRead {
                path: yaml_path.display().to_string(),
                source,
            })?;

        let computed_hash = sha256_hex(yaml_content.as_bytes());
        let stored_hash = load_hash(&truth_directory, task_id)?;

        if computed_hash != stored_hash {
            return Err(StvpError::HashMismatch {
                task_id: task_id.to_string(),
                stored_hash,
                computed_hash,
            });
        }

        SourceTruth::from_yaml(&yaml_content)
    }
}

fn truth_directory(project_root: &ProjectRoot) -> PathBuf {
    project_root.as_path().join(".yantra").join("source_truth")
}

fn manifest_db_path(truth_directory: &Path) -> PathBuf {
    truth_directory.join(MANIFEST_DB_FILE)
}

fn store_hash(
    truth_directory: &Path,
    task_id: TaskId,
    content_hash: &str,
) -> Result<(), StvpError> {
    let database_pool = connection_pool(&manifest_db_path(truth_directory))?;
    let database_guard = database_pool.lock().map_err(|_| StvpError::LockPoisoned)?;

    apply_migrations(&database_guard, &[HASH_TABLE_MIGRATION])?;

    database_guard.execute(
        "INSERT OR REPLACE INTO truth_hashes (task_id, content_hash) VALUES (?1, ?2)",
        rusqlite::params![task_id.to_string(), content_hash],
    )?;

    Ok(())
}

fn load_hash(truth_directory: &Path, task_id: TaskId) -> Result<String, StvpError> {
    let db_path = manifest_db_path(truth_directory);
    if !db_path.exists() {
        return Err(StvpError::TruthNotFound {
            task_id: task_id.to_string(),
        });
    }

    let database_pool = connection_pool(&db_path)?;
    let database_guard = database_pool.lock().map_err(|_| StvpError::LockPoisoned)?;

    apply_migrations(&database_guard, &[HASH_TABLE_MIGRATION])?;

    let task_id_string = task_id.to_string();
    let stored_hash: Option<String> = database_guard
        .query_row(
            "SELECT content_hash FROM truth_hashes WHERE task_id = ?1",
            rusqlite::params![task_id_string],
            |row| row.get(0),
        )
        .optional()?;

    stored_hash.ok_or_else(|| StvpError::TruthNotFound {
        task_id: task_id.to_string(),
    })
}

fn sha256_hex(data: &[u8]) -> String {
    let digest_output = digest::digest(&digest::SHA256, data);
    hex_encode(digest_output.as_ref())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut hex_string, byte| {
            write!(hex_string, "{byte:02x}").expect("writing to a String never fails");
            hex_string
        },
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{Strictness, TaskClass, TaskId};

    use super::*;

    fn sample_truth() -> SourceTruth {
        let mut collected_answers = BTreeMap::new();
        collected_answers.insert("success_criterion".to_owned(), "all tests pass".to_owned());

        SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::NewFeature,
            strictness: Strictness::Strict,
            description: "add JWT rotation".to_owned(),
            created_at: Utc::now(),
            answers: collected_answers,
        }
    }

    #[test]
    fn source_truth_yaml_round_trip() {
        let original_truth = sample_truth();
        let yaml_content = original_truth.to_yaml().expect("serialization succeeds");
        let restored_truth =
            SourceTruth::from_yaml(&yaml_content).expect("deserialization succeeds");
        assert_eq!(original_truth, restored_truth);
    }

    #[test]
    fn yaml_contains_expected_fields() {
        let source_truth = sample_truth();
        let yaml_content = source_truth.to_yaml().expect("serialization succeeds");
        assert!(yaml_content.contains("task_id"));
        assert!(yaml_content.contains("task_class"));
        assert!(yaml_content.contains("strictness"));
        assert!(yaml_content.contains("description"));
        assert!(yaml_content.contains("answers"));
        assert!(yaml_content.contains("success_criterion"));
    }

    #[test]
    fn store_and_load_round_trips_with_hash_verification() {
        let temp_root_path =
            std::env::temp_dir().join(format!("yantra-stvp-test-{}", TaskId::new()));
        std::fs::create_dir_all(&temp_root_path).unwrap();
        let project_root = ProjectRoot::new(&temp_root_path).unwrap();

        let source_truth = sample_truth();
        let task_id = source_truth.task_id;
        SourceTruth::store(&project_root, &source_truth).expect("store succeeds");

        let loaded_truth = SourceTruth::load(&project_root, task_id).expect("load succeeds");
        assert_eq!(source_truth, loaded_truth);
    }

    #[test]
    fn load_detects_tampered_yaml_file() {
        let temp_root_path =
            std::env::temp_dir().join(format!("yantra-stvp-tamper-{}", TaskId::new()));
        std::fs::create_dir_all(&temp_root_path).unwrap();
        let project_root = ProjectRoot::new(&temp_root_path).unwrap();

        let source_truth = sample_truth();
        let task_id = source_truth.task_id;
        SourceTruth::store(&project_root, &source_truth).expect("store succeeds");

        let yaml_path = temp_root_path
            .join(".yantra")
            .join("source_truth")
            .join(format!("{task_id}.yaml"));
        std::fs::write(&yaml_path, "tampered: content\n").unwrap();

        let load_result = SourceTruth::load(&project_root, task_id);
        assert!(
            matches!(load_result, Err(StvpError::HashMismatch { .. })),
            "expected HashMismatch but got: {load_result:?}"
        );
    }

    #[test]
    fn load_returns_truth_not_found_for_missing_task() {
        let temp_root_path =
            std::env::temp_dir().join(format!("yantra-stvp-missing-{}", TaskId::new()));
        std::fs::create_dir_all(&temp_root_path).unwrap();
        let project_root = ProjectRoot::new(&temp_root_path).unwrap();

        let missing_task_id = TaskId::new();
        let load_result = SourceTruth::load(&project_root, missing_task_id);
        assert!(matches!(load_result, Err(StvpError::TruthNotFound { .. })));
    }
}
