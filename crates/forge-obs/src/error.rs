//! # forge-obs: Error Types
//!
//! Defines typed observability errors for `SQLite` persistence, tracing setup,
//! serialization, channel delivery, and identifier parsing. All public
//! functions in `forge-obs` return this crate-local result type.
//!
//! ## Input
//! - Lower-level `SQLite`, serde, tracing, and channel errors
//!
//! ## Output
//! - `ObsError` and `ObsResult<T>`
//!
//! ## Related
//! - `forge-obs::traces` — maps database failures into `ObsError`
//! - `forge-obs::decision_archaeology` — maps traversal failures into `ObsError`

use thiserror::Error;

/// Result alias used throughout `forge-obs`.
pub type ObsResult<T> = Result<T, ObsError>;

/// Error type for observability infrastructure.
#[derive(Debug, Error)]
pub enum ObsError {
    /// `SQLite` operation failed.
    #[error("SQLite error: {0}")]
    Database(#[from] rusqlite::Error),

    /// JSON serialization or deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Core identifier or path parsing failed.
    #[error("core error: {0}")]
    Core(#[from] yantra_core::CoreError),

    /// Date-time parsing failed.
    #[error("timestamp parse error: {0}")]
    Timestamp(#[from] chrono::ParseError),

    /// Global tracing subscriber initialization failed.
    #[error("tracing subscriber initialization failed")]
    TracingAlreadyInitialized,

    /// Watchdog channel delivery failed.
    #[error("watchdog channel is closed")]
    WatchdogChannelClosed,
}
