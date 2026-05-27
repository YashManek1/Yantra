//! # forge-agents: Agent Error Types
//!
//! Typed errors for the agent layer. Library crate errors use `thiserror` so
//! callers (orchestrator, CLI) can match on specific failure modes.
//!
//! ## Input
//! - None
//!
//! ## Output
//! - `AgentError` variants covering truth-token, MCP, router, and assembly failures
//!
//! ## Related
//! - `forge-agents::agent` — `Agent` trait returns `Result<TaskResult, AgentError>`

use thiserror::Error;

/// Errors produced by specialist agent implementations.
#[derive(Debug, Error)]
pub enum AgentError {
    /// Task dispatched without the required STVP truth token.
    #[error("task {task_id} has no truth token — STVP must authorize before dispatch")]
    MissingTruthToken {
        /// String representation of the task identifier.
        task_id: String,
    },

    /// An MCP tool call returned an error response.
    #[error("MCP tool call failed: {0}")]
    McpToolCall(String),

    /// The model router rejected the request or the provider returned an error.
    #[error("model provider error: {0}")]
    ModelProvider(String),

    /// Prompt assembly failed before the model call.
    #[error("prompt assembly failed: {0}")]
    PromptAssembly(String),

    /// Subprocess execution failed inside an agent.
    #[error("subprocess error: {0}")]
    Subprocess(String),

    /// Ed25519 commit signing failed.
    #[error("commit signing failed: {0}")]
    CommitSigning(String),
}
