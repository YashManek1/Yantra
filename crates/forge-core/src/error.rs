//! # forge-core: Error Types
//!
//! Defines typed errors shared by all foundational modules in `forge-core`.
//! Callers receive `CoreError` values for identifier parsing, truth-token
//! signing, path validation, and database setup failures.
//!
//! ## Input
//! - Lower-level parse, crypto, filesystem, and `SQLite` errors
//!
//! ## Output
//! - `CoreError` and `CoreResult<T>` for shared error handling
//!
//! ## Related
//! - `forge-core::id` — identifier parsing errors
//! - `forge-core::truth` — truth-token signing and verification errors
//! - `forge-core::path` — project-root containment errors

use std::path::PathBuf;

use thiserror::Error;

/// Result alias used by all `forge-core` modules.
pub type CoreResult<T> = Result<T, CoreError>;

/// Shared error type for the foundational Yantra type crate.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A UUID-backed identifier could not be parsed.
    #[error("invalid UUID for {type_name}: {source}")]
    InvalidUuid {
        /// Identifier type being parsed.
        type_name: &'static str,
        /// UUID parse failure.
        source: uuid::Error,
    },

    /// A string-backed identifier was empty or otherwise invalid.
    #[error("invalid symbol id: {reason}")]
    InvalidSymbolId {
        /// Human-readable validation failure.
        reason: String,
    },

    /// A string-backed model identifier was empty or otherwise invalid.
    #[error("invalid model id: {reason}")]
    InvalidModelId {
        /// Human-readable validation failure.
        reason: String,
    },

    /// A requested path escaped the configured project root.
    #[error("path escapes project root: root={root}, requested={requested}")]
    PathEscapesRoot {
        /// Canonical project root.
        root: PathBuf,
        /// Requested path after normalization.
        requested: PathBuf,
    },

    /// A path operation failed.
    #[error("path operation failed for {path}: {source}")]
    PathIo {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Filesystem failure.
        source: std::io::Error,
    },

    /// A cryptographic key or signature operation failed.
    #[error("truth-token signing failed")]
    SigningFailed,

    /// `SQLite` setup or migration failed.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
}
