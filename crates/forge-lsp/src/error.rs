//! # forge-lsp: Error Types
//!
//! Typed failures for LSP client connections, protocol framing, process
//! spawning, and response correlation. Converts to `anyhow::Error` for
//! callers that use the simpler error-chain API.
//!
//! ## Input
//! - OS process, IO, and JSON-RPC framing failures
//!
//! ## Output
//! - `LspError`, `LspResult<T>`
//!
//! ## Related
//! - `forge-lsp::client` — maps spawn and framing failures here
//! - `forge-lsp::bridge` — surfaces errors to MCP callers

use thiserror::Error;

/// Result alias for LSP bridge operations.
pub type LspResult<T> = Result<T, LspError>;

/// Errors produced by the LSP bridge and client layer.
#[derive(Debug, Error)]
pub enum LspError {
    /// Could not spawn the language server subprocess.
    #[error("failed to spawn language server for {language}: {source}")]
    SpawnFailed {
        /// Language identifier.
        language: String,
        /// Underlying IO failure.
        source: std::io::Error,
    },

    /// The language server process exited unexpectedly.
    #[error("language server for {language} exited unexpectedly")]
    ProcessExited {
        /// Language identifier.
        language: String,
    },

    /// Content-Length framing failed while reading or writing.
    #[error("LSP framing error: {0}")]
    Framing(String),

    /// JSON serialization or deserialization of an LSP message failed.
    #[error("LSP JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The language server returned an error response.
    #[error("LSP server error {code}: {message}")]
    ServerError {
        /// JSON-RPC error code from the server.
        code: i64,
        /// Human-readable error message.
        message: String,
    },

    /// A pending request timed out waiting for a server response.
    #[error("LSP request timed out after {timeout_ms}ms")]
    Timeout {
        /// Configured timeout in milliseconds.
        timeout_ms: u64,
    },

    /// No language server is configured for the requested language.
    #[error("no LSP server configured for language: {language}")]
    UnsupportedLanguage {
        /// Language identifier.
        language: String,
    },

    /// A requested capability (e.g. hover) is not supported by this server.
    #[error("LSP capability {capability} not supported by server for {language}")]
    CapabilityUnsupported {
        /// Language identifier.
        language: String,
        /// Capability name.
        capability: String,
    },
}
