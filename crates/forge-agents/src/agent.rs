//! # forge-agents: Agent Trait and Shared Context Types
//!
//! Defines the `Agent` trait that every specialist agent must implement, the
//! `AgentContext` carrying shared runtime services, and the `TaskResult` and
//! `Diff` types that agents return to the orchestrator.
//!
//! ## Input
//! - `TaskNode` dispatched by the orchestrator (must carry a `TruthToken`)
//! - `AgentContext` assembled by the orchestrator before dispatch
//!
//! ## Output
//! - `TaskResult` containing the outcome, an optional unified diff, a one-line
//!   summary, and a fresh `DecisionId` for the archaeology log
//!
//! ## Related
//! - `forge-agents::coder` — first concrete `Agent` implementation
//! - `forge-router` — provides the `Router` held in `AgentContext`
//! - `forge-tools` — provides the `McpRouter` held in `AgentContext`

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use yantra_core::{AgentCapability, AgentKind, DecisionId, Outcome, SessionId, TaskId, TaskNode};
use yantra_router::Router;
use yantra_tools::McpRouter;

use crate::error::AgentError;

/// One file's worth of unified diff produced by the Coder agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    /// Relative path of the file the diff applies to.
    pub file_path: String,
    /// Raw unified-diff text (lines starting with `---`, `+++`, `@@`, `+`, `-`, ` `).
    pub unified_diff: String,
}

/// Complete set of file diffs produced by one agent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    /// One entry per changed file.
    pub files: Vec<FileDiff>,
}

/// Result returned by an agent to the orchestrator after executing a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Identifier of the task that was executed.
    pub task_id: TaskId,
    /// Whether the agent succeeded or failed.
    pub outcome: Outcome,
    /// Proposed code change, or `None` if the agent produced no diff.
    pub diff: Option<Diff>,
    /// One-line summary of what was done or why it failed.
    pub summary: String,
    /// Fresh decision identifier written to the archaeology log.
    pub decision_id: DecisionId,
}

/// Runtime services and identifiers available to every agent during a run.
pub struct AgentContext {
    /// Model router for all LLM calls.
    pub router: Arc<Router>,
    /// MCP tool router for CRG, FS, Git, and LSP calls.
    pub tools: McpRouter,
    /// Session identifier for span recording and archaeology.
    pub session_id: SessionId,
    /// Results from upstream tasks in the orchestrator DAG.
    pub upstream_results: Vec<TaskResult>,
    /// Memory service facade for recall and mistake prevention rules.
    pub memory: Arc<yantra_memory::MemoryService>,
}

/// Trait implemented by every specialist agent.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Returns the agent's kind for routing and audit.
    fn kind(&self) -> AgentKind;

    /// Returns the capabilities this agent requires from the tool layer.
    fn capabilities(&self) -> Vec<AgentCapability>;

    /// Executes the task and returns a result for the orchestrator.
    ///
    /// # Errors
    ///
    /// Returns an `AgentError` if the task has no truth token, a tool call
    /// fails, or the model provider returns an error.
    async fn run(&self, task: TaskNode, context: AgentContext) -> Result<TaskResult, AgentError>;
}
