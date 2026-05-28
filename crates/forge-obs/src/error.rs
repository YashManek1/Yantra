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

    /// Global tracing subscriber was already initialized by another caller.
    ///
    /// `init_tracing()` must be called exactly once per process. Ensure no
    /// other crate, test harness, or binary entrypoint also calls it.
    #[error(
        "tracing subscriber already initialized — \
         call init_tracing() exactly once per process"
    )]
    TracingAlreadyInitialized,

    /// The watchdog heartbeat channel closed before the session ended normally.
    ///
    /// This usually means the watchdog task panicked or was dropped while the
    /// orchestrator was still running. Check logs for a prior panic in the
    /// watchdog thread.
    #[error(
        "watchdog heartbeat channel closed — \
         the watchdog task may have panicked or been dropped prematurely"
    )]
    WatchdogChannelClosed,
}
