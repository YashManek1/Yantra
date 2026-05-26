//! # forge-memory: Error Types
//!
//! Typed error variants for all memory-tier operations: SQLite persistence,
//! filesystem I/O, lock poisoning, and JSON serialization.
//!
//! ## Input
//! - Lower-level I/O, SQLite, and JSON errors surfaced during memory operations
//!
//! ## Output
//! - `MemoryError` returned by all public memory APIs
//!
//! ## Related
//! - `forge-memory::recall` — primary producer of database errors
//! - `forge-memory::archival` — primary producer of I/O errors
//! - `forge-core::error` — foundational error type wrapped by `Core` variant

use thiserror::Error;

use yantra_core::CoreError;

/// Result alias used across `forge-memory`.
pub type MemoryResult<T> = Result<T, MemoryError>;

/// All errors that can arise during memory-tier operations.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// A SQLite operation failed.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// The internal database mutex was poisoned by a panicking thread.
    #[error("internal database mutex poisoned")]
    LockPoisoned,

    /// A directory required for storage could not be created.
    #[error("could not create directory {path}: {source}")]
    DirectoryCreate {
        /// Path that could not be created.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A file could not be read.
    #[error("could not read file {path}: {source}")]
    FileRead {
        /// Path that could not be read.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A file could not be written.
    #[error("could not write file {path}: {source}")]
    FileWrite {
        /// Path that could not be written.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// JSON serialization or deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A foundational core operation failed.
    #[error("core error: {0}")]
    Core(#[from] CoreError),
}
