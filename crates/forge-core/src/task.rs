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

impl TaskClass {
    /// Returns the stable lowercase string representation of the task class.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewFeature => "new_feature",
            Self::BugFix => "bug_fix",
            Self::Refactor => "refactor",
            Self::Migration => "migration",
            Self::Integration => "integration",
            Self::Exploration => "exploration",
            Self::Chore => "chore",
            Self::Docstring => "docstring",
            Self::Style => "style",
        }
    }
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

/// A generated question definition representing (id, text, required, suggested_answer, help_text).
pub type AugmentedQuestionTuple = (String, String, bool, Option<String>, Option<String>);

/// Future type returned by LLM question augmentation.
pub type AugmentFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<AugmentedQuestionTuple>> + Send>>;

/// Port/trait used to dynamically generate questionnaire questions via LLM.
/// Implemented by downstream crates (e.g. `yantra-cli` using `Router`) and
/// injected into `STVP` interrogator.
pub trait AugmenterPort: Send + Sync {
    /// Generates context-specific questions for a task using an LLM.
    fn augment(
        &self,
        task_description: &str,
        task_class: TaskClass,
        existing_question_ids: Vec<String>,
        max_count: usize,
    ) -> AugmentFuture;
}
