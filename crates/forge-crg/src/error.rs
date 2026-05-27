//! # forge-crg: Error Types
//!
//! Defines typed failures for database operations, AST extraction, embedding,
//! graph queries, and I/O. All public APIs return `CrgResult<T>`.
//!
//! ## Input
//! - Underlying `rusqlite`, `yantra_ast`, `std::io`, and serialization failures
//!
//! ## Output
//! - `CrgError` and `CrgResult<T>`
//!
//! ## Related
//! - `forge-crg::builder` — returns `CrgResult` from build operations
//! - `forge-crg::subgraph` — returns `CrgResult` from query operations

use thiserror::Error;

/// Error type for Code-Review Graph operations.
#[derive(Debug, Error)]
pub enum CrgError {
    /// A `SQLite` operation failed.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// AST extraction from a source file failed.
    #[error("AST extraction failed: {0}")]
    AstExtraction(#[from] yantra_ast::AstError),

    /// The embedding model returned an error or could not be initialised.
    #[error("embedding model error: {0}")]
    Embedding(String),

    /// A symbol identifier was not found in the graph cache.
    #[error("symbol not found: {symbol_id}")]
    SymbolNotFound {
        /// The identifier that was not present.
        symbol_id: String,
    },

    /// A filesystem I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The graph cache has not been built yet.
    #[error("graph cache not built: call build() before querying")]
    CacheNotBuilt,

    /// A serialization or deserialization operation failed.
    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Result alias used across `forge-crg`.
pub type CrgResult<T> = Result<T, CrgError>;
