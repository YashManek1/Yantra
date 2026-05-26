//! # forge-tools: Error Types
//!
//! Defines typed failures for MCP routing, filesystem tools, and gitoxide
//! repository operations. Callers receive serializable MCP errors or internal
//! tool errors that can be converted into JSON-RPC responses.
//!
//! ## Input
//! - Filesystem, JSON, path safety, and repository failures
//! - Tool authorization decisions
//!
//! ## Output
//! - `McpError`, `ToolsError`, and `ToolsResult<T>`
//!
//! ## Related
//! - `forge-tools::mcp` — converts errors into JSON-RPC error objects
//! - `forge-tools::tools::fs` — maps path safety failures
//! - `forge-tools::tools::git` — maps gitoxide failures

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Result alias for the tool implementation layer.
pub type ToolsResult<T> = Result<T, ToolsError>;

/// Errors produced by MCP servers and repository helpers.
#[derive(Debug, Error)]
pub enum ToolsError {
    /// A required parameter was absent or had the wrong shape.
    #[error("invalid parameters: {0}")]
    InvalidParams(String),

    /// The requested operation is not permitted for this agent or path.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// A target path failed project-root validation.
    #[error("path safety error: {0}")]
    PathSafety(#[from] yantra_core::CoreError),

    /// A filesystem operation failed.
    #[error("filesystem operation failed for {path}: {source}")]
    Io {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying IO failure.
        source: std::io::Error,
    },

    /// JSON serialization or deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The repository could not be opened by gitoxide.
    #[error("repository error: {0}")]
    Repository(String),

    /// The requested JSON-RPC method does not exist.
    #[error("method not found: {0}")]
    MethodNotFound(String),
}

/// MCP-level error that maps directly to JSON-RPC failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpError {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}

impl McpError {
    /// Creates an invalid-params error.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    /// Creates a forbidden-operation error.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            code: -32003,
            message: message.into(),
        }
    }

    /// Creates a method-not-found error.
    pub fn method_not_found(message: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: message.into(),
        }
    }

    /// Creates an internal server error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
        }
    }
}

impl From<ToolsError> for McpError {
    fn from(error: ToolsError) -> Self {
        match error {
            ToolsError::InvalidParams(message) => Self::invalid_params(message),
            ToolsError::Forbidden(message) => Self::forbidden(message),
            ToolsError::MethodNotFound(message) => Self::method_not_found(message),
            ToolsError::PathSafety(path_error) => Self::forbidden(path_error.to_string()),
            ToolsError::Io { path, source } => {
                Self::internal(format!("{}: {source}", path.display()))
            }
            ToolsError::Json(json_error) => Self::invalid_params(json_error.to_string()),
            ToolsError::Repository(message) => Self::internal(message),
        }
    }
}

impl From<yantra_core::CoreError> for McpError {
    fn from(error: yantra_core::CoreError) -> Self {
        Self::from(ToolsError::PathSafety(error))
    }
}
