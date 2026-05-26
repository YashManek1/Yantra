//! # forge-stvp: Error Types
//!
//! Typed error variants covering every STVP operation: task classification,
//! questionnaire execution, source-truth YAML serialization, disk storage,
//! and content-hash verification.
//!
//! ## Input
//! - Lower-level I/O, YAML, SQLite, and core errors surfaced during STVP runs
//!
//! ## Output
//! - `StvpError` returned by all public STVP APIs
//!
//! ## Related
//! - `forge-stvp::interrogator` — the primary producer of `StvpError` values
//! - `forge-stvp::source_truth` — produces `FileRead`, `FileWrite`, `HashMismatch`

use thiserror::Error;

use yantra_core::CoreError;

/// All errors that can arise during STVP execution.
#[derive(Debug, Error)]
pub enum StvpError {
    /// The task description string was empty or contained only whitespace.
    #[error("task description must not be empty")]
    EmptyDescription,

    /// The user aborted the questionnaire before completing it.
    #[error("questionnaire was aborted before all required questions were answered")]
    QuestionnaireAborted,

    /// A required question received a blank answer.
    #[error("required question {question_id:?} received an empty answer")]
    MissingRequiredAnswer {
        /// Identifier of the question whose answer was missing.
        question_id: String,
    },

    /// YAML serialization or deserialization of a `SourceTruth` failed.
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// The source-truth YAML file could not be read from disk.
    #[error("could not read source truth at {path}: {source}")]
    FileRead {
        /// Path that could not be read.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The source-truth YAML file could not be written to disk.
    #[error("could not write source truth at {path}: {source}")]
    FileWrite {
        /// Path that could not be written.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The content hash computed on load does not match the hash stored in the DB.
    #[error(
        "source truth hash mismatch for task {task_id}: \
         stored {stored_hash}, computed {computed_hash}"
    )]
    HashMismatch {
        /// Task whose artifact failed hash verification.
        task_id: String,
        /// Hash that was stored at write time.
        stored_hash: String,
        /// Hash recomputed from the file content on load.
        computed_hash: String,
    },

    /// No source-truth artifact found for the requested task.
    #[error("source truth for task {task_id} not found")]
    TruthNotFound {
        /// Task identifier that was not found.
        task_id: String,
    },

    /// The directory holding source-truth artifacts could not be created.
    #[error("could not create truth directory {path}: {source}")]
    DirectoryCreate {
        /// Path that could not be created.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A SQLite operation failed.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// The internal database mutex was poisoned by a panicking thread.
    #[error("internal database mutex poisoned")]
    LockPoisoned,

    /// Ed25519 key generation failed.
    #[error("session key generation failed")]
    KeyGeneration,

    /// The session key file could not be written.
    #[error("could not write session key to {path}: {source}")]
    KeyWrite {
        /// Path that could not be written.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The session key file could not be read.
    #[error("could not read session key from {path}: {source}")]
    KeyRead {
        /// Path that could not be read.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The key material was rejected by the `ring` crypto library.
    #[error("session key rejected: {reason}")]
    KeyRejected {
        /// Description of why the key was rejected.
        reason: String,
    },

    /// A foundational core operation failed.
    #[error("core error: {0}")]
    Core(#[from] CoreError),
}
