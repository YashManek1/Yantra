//! # forge-canvas::error: Typed Errors for the Visual Feature Host
//!
//! Defines `CanvasError` covering URL fetching, DOM parsing, TSX file
//! writing, WebSocket lifecycle, and HTTP server binding. All public APIs
//! return `CanvasResult<T>`.
//!
//! ## Input
//! - I/O failures from TCP listener binding and TSX file writes
//! - HTTP failures from `reqwest` during URL cloning
//! - HTML/CSS parse failures from `scraper` and `lightningcss`
//!
//! ## Output
//! - `CanvasError` typed enum
//! - `CanvasResult<T>` alias for `Result<T, CanvasError>`
//!
//! ## Related
//! - `forge-canvas::clone` — emits `CloneFailed`
//! - `forge-canvas::tsx_writer` — emits `TsxWriteFailed`
//! - `forge-canvas::server` — emits `Bind`

use std::path::PathBuf;

/// All errors produced by the canvas server, clone pipeline, and editor.
#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    /// The TCP listener could not be bound to the requested address.
    #[error("failed to bind TCP listener on {addr}: {source}")]
    Bind {
        /// The address string that was attempted.
        addr: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// Failed to fetch or parse the URL during clone.
    #[error("failed to clone {url}: {reason}")]
    CloneFailed {
        /// URL that failed to clone.
        url: String,
        /// Human-readable reason (network, HTTP status, parse failure).
        reason: String,
    },

    /// Failed to write a TSX component file to disk.
    #[error("failed to write TSX component at {path}: {reason}")]
    TsxWriteFailed {
        /// Path that could not be written.
        path: PathBuf,
        /// Human-readable I/O failure reason.
        reason: String,
    },

    /// HTML DOM parsing failed.
    #[error("DOM parse failed: {0}")]
    DomParseFailed(String),

    /// CSS declaration parsing failed.
    #[error("CSS parse failed: {0}")]
    CssParseFailed(String),

    /// A project slug was requested that has not been registered.
    #[error("project {0:?} is not registered with the canvas server")]
    UnknownProject(String),

    /// A `yantra_id` was looked up that doesn't exist in the project's DOM.
    #[error("unknown yantra_id {yantra_id} in project {project}")]
    UnknownYantraId {
        /// Project slug being queried.
        project: String,
        /// The yantra_id that wasn't found.
        yantra_id: String,
    },

    /// JSON serialization or deserialization failed.
    #[error("JSON conversion failed: {0}")]
    Json(String),

    /// A filesystem I/O operation outside of the TSX writer failed.
    #[error("filesystem error at {path}: {source}")]
    Io {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

/// Convenience alias for results returned from canvas APIs.
pub type CanvasResult<T> = Result<T, CanvasError>;
