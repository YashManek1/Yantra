//! # forge-orchestrator: Error Types
//!
//! All typed errors produced by the orchestrator subsystems: DAG persistence,
//! scheduling, circuit breaker, CSP planner, event bus, dependency inference,
//! and the speculation engine.
//!
//! ## Input
//! - Underlying errors from rusqlite, forge-router, forge-agents, forge-obs
//!
//! ## Output
//! - `SchedulingError` — token gate errors (unchanged for backward compatibility)
//! - `OrchestratorError` — all other orchestrator subsystem errors
//!
//! ## Related
//! - `forge-orchestrator::dag` — produces `OrchestratorError::Database`
//! - `forge-orchestrator::scheduler` — produces `OrchestratorError::AgentFailed`
//! - `forge-stvp` — `StvpError` wrapped in `SchedulingError::TokenVerificationFailed`

use thiserror::Error;
use yantra_core::AgentKind;
use yantra_stvp::StvpError;

/// Errors returned when submitting a task through the STVP token gate.
///
/// These variants are preserved unchanged so existing callers that match on
/// `SchedulingError` continue to compile without modification.
#[derive(Debug, Error)]
pub enum SchedulingError {
    /// The task carries no truth token.
    ///
    /// Every task must pass through STVP before it can be scheduled.
    #[error("task {task_id} cannot be scheduled: no truth token present — run STVP first")]
    MissingTruthToken {
        /// Identifier of the task that was rejected.
        task_id: String,
    },

    /// The token's Ed25519 signature does not match the session public key.
    #[error("task {task_id} has an invalid truth token signature")]
    InvalidTruthToken {
        /// Identifier of the task whose token failed signature verification.
        task_id: String,
    },

    /// A filesystem or key-parse error occurred while verifying the token.
    #[error("token verification for task {task_id} failed: {source}")]
    TokenVerificationFailed {
        /// Identifier of the task whose token could not be verified.
        task_id: String,
        /// Underlying STVP error (typically a missing `session.pub` file).
        #[source]
        source: StvpError,
    },
}

/// All errors produced by orchestrator subsystems other than the token gate.
#[derive(Debug, Error)]
pub enum OrchestratorError {
    /// A SQLite database operation failed.
    #[error("database error: {0}")]
    Database(String),

    /// No agent is registered in the scheduler for the requested kind.
    #[error("no agent registered for kind {kind:?}")]
    NoAgentRegistered {
        /// The agent kind for which no registration exists.
        kind: AgentKind,
    },

    /// The model router failed to select a provider.
    #[error("router error: {0}")]
    Router(String),

    /// A model provider returned an error response.
    #[error("provider error: {0}")]
    Provider(String),

    /// An agent returned an error while executing a task.
    #[error("agent execution failed for task {task_id}: {reason}")]
    AgentFailed {
        /// Task that the agent was executing.
        task_id: String,
        /// Human-readable failure description.
        reason: String,
    },

    /// A JSON parse or serialization operation failed.
    #[error("JSON error: {0}")]
    JsonParsing(String),

    /// An observability persistence operation failed.
    #[error("observability error: {0}")]
    Obs(String),

    /// The circuit breaker is open and dispatch is paused.
    #[error("circuit breaker open: {calls_per_minute} calls/min exceeds limit")]
    CircuitBreakerOpen {
        /// Measured LLM calls per minute that triggered the open transition.
        calls_per_minute: u32,
    },

    /// The CSP planner detected a contradiction among hard constraints.
    #[error("CSP contradiction: {0}")]
    CspContradiction(String),

    /// A task ID was referenced in the DAG but the row does not exist.
    #[error("task {task_id} not found in DAG")]
    TaskNotFound {
        /// String representation of the missing task identifier.
        task_id: String,
    },

    /// The task dependency graph contains a cycle.
    #[error("cyclic dependency detected in task graph")]
    CyclicDependency,

    /// A task failed permanently (exhausted retries).
    #[error("task {task_id} failed permanently")]
    TaskFailedPermanent {
        /// String representation of the failed task identifier.
        task_id: String,
    },

    /// A task finished successfully but its result was missing from the store.
    #[error("task result for {task_id} is missing")]
    ResultMissing {
        /// String representation of the task identifier.
        task_id: String,
    },
}

impl From<rusqlite::Error> for OrchestratorError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

impl From<serde_json::Error> for OrchestratorError {
    fn from(error: serde_json::Error) -> Self {
        Self::JsonParsing(error.to_string())
    }
}

impl From<yantra_obs::ObsError> for OrchestratorError {
    fn from(error: yantra_obs::ObsError) -> Self {
        Self::Obs(error.to_string())
    }
}
