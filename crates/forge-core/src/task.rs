//! # forge-core: Task Types
//!
//! Defines the canonical task classification, lifecycle status, and DAG node
//! shape used by the orchestrator. A `TaskNode` can only carry shared core
//! types, keeping the task graph independent of implementation crates.
//!
//! ## Input
//! - User task descriptions
//! - Dependency identifiers and assignment metadata from planning
//!
//! ## Output
//! - `TaskClass`, `TaskStatus`, and `TaskNode`
//!
//! ## Related
//! - `forge-core::truth` — provides `TruthToken`
//! - `forge-core::agent` — provides `AgentKind`

use serde::{Deserialize, Serialize};

use crate::agent::AgentKind;
use crate::id::{DecisionId, TaskId};
use crate::truth::TruthToken;

/// Classification of a user task for policy and validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskClass {
    /// Adds user-visible behavior.
    NewFeature,
    /// Fixes broken behavior.
    BugFix,
    /// Preserves behavior while changing structure.
    Refactor,
    /// Moves data or changes schema.
    Migration,
    /// Connects Yantra to another system.
    Integration,
    /// Investigates without necessarily changing code.
    Exploration,
    /// Performs maintenance.
    Chore,
    /// Adds or changes documentation comments.
    Docstring,
    /// Changes formatting or style only.
    Style,
}

/// Lifecycle state of a task node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task is ready but not started.
    Pending,
    /// Task is currently executing.
    Running,
    /// Task completed successfully.
    Complete,
    /// Task failed and cannot continue automatically.
    Failed,
    /// Task was postponed by policy or confidence threshold.
    Deferred,
    /// Task was stopped by a safety condition.
    Halted,
}

/// Node in the orchestrator task DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNode {
    /// Unique task identifier.
    pub id: TaskId,
    /// Natural language task description.
    pub description: String,
    /// Current lifecycle state.
    pub status: TaskStatus,
    /// Task classification used for `STVP` strictness.
    pub class: TaskClass,
    /// Task dependencies that must complete first.
    pub dependencies: Vec<TaskId>,
    /// Agent assigned to execute the task.
    pub assigned_agent: Option<AgentKind>,
    /// Source-truth token authorizing the task.
    pub truth_token: Option<TruthToken>,
    /// Decision that produced this task node.
    pub parent_decision_id: Option<DecisionId>,
}
