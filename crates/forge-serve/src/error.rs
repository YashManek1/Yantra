//! # forge-serve::error: Serve-Layer Error Types
//!
//! Defines the typed error vocabulary for the Axum HTTP server and SSE
//! streaming layer. All public functions in `forge-serve` return
//! `ServeResult<T>` so callers get structured errors rather than strings.
//!
//! ## Input
//! - I/O errors from TCP listener binding
//! - Channel closure signals from the broadcast bus
//! - JSON serialization failures in route handlers
//!
//! ## Output
//! - `ServeError` — typed error enum
//! - `ServeResult<T>` — convenience alias for `Result<T, ServeError>`
//!
//! ## Related
//! - `forge-serve::server` — primary consumer of `ServeResult`
//! - `forge-serve::sse` — emits `BroadcastClosed` on channel shutdown

/// All errors that can originate from the HTTP and SSE surface.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The TCP listener could not be bound to the requested address.
    #[error("failed to bind TCP listener on {addr}: {source}")]
    Bind {
        /// The address string that was attempted.
        addr: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// The broadcast channel used for SSE was closed before the stream ended.
    #[error("SSE broadcast channel closed")]
    BroadcastClosed,

    /// A JSON serialization error occurred in a route handler.
    #[error("JSON serialization failed: {0}")]
    Json(String),
}

/// Convenience alias used by all public functions in `forge-serve`.
pub type ServeResult<T> = Result<T, ServeError>;
