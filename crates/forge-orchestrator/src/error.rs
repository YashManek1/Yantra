//! # forge-orchestrator: Scheduling Error Types
//!
//! Typed errors returned by `Orchestrator::schedule_task`. Every variant
//! includes the `task_id` that triggered the failure so callers can log or
//! surface it without re-extracting it from the original `TaskNode`.
//!
//! ## Related
//! - `forge-orchestrator::Orchestrator` — the only producer of these errors
//! - `forge-stvp::StvpError` — wrapped in `TokenVerificationFailed`

use thiserror::Error;

use yantra_stvp::StvpError;

/// All errors that can occur when submitting a task to the orchestrator.
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
