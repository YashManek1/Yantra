//! # forge-night: Error Types
//!
//! Typed error variants covering every Night Mode operation: STVP validation
//! during Twilight, truth-token issuance, decision-rule collection, night-plan
//! generation, checkpoint I/O, git branch management, and watchdog signalling.
//!
//! ## Input
//! - Lower-level errors from `forge-stvp`, `forge-orchestrator`, and I/O layers
//!
//! ## Output
//! - `NightError` returned by all public forge-night APIs
//!
//! ## Related
//! - `forge-night::twilight` — primary producer of `NightError` values during Twilight
//! - `forge-night::night_run` — produces `DagError`, `CheckpointIo`, `WatchdogDied`
//! - `forge-stvp::error` — source of `StvpError` wrapped here

use thiserror::Error;
use yantra_stvp::StvpError;

/// All errors that can arise during Night Mode execution.
#[derive(Debug, Error)]
pub enum NightError {
    /// STVP validation failed for a goal during the Twilight phase.
    #[error("STVP validation failed for task {description:?}: {source}")]
    StvpValidationFailed {
        /// Natural-language description of the goal that failed.
        description: String,
        /// Underlying STVP error.
        #[source]
        source: StvpError,
    },

    /// Truth-token issuance failed after successful STVP validation.
    #[error("token issuance failed for task {description:?}: {source}")]
    TokenIssuanceFailed {
        /// Natural-language description of the task.
        description: String,
        /// Underlying STVP error.
        #[source]
        source: StvpError,
    },

    /// The user aborted the Twilight session before confirming the plan.
    #[error("twilight session was aborted by the user")]
    TwilightAborted,

    /// Decision-rule collection failed or the user aborted it.
    #[error("decision rule collection failed: {reason}")]
    RuleCollectionFailed {
        /// Human-readable explanation.
        reason: String,
    },

    /// Night plan markdown rendering encountered an unexpected condition.
    #[error("night plan generation failed: {reason}")]
    PlanGenerationFailed {
        /// Human-readable explanation.
        reason: String,
    },

    /// A SQLite DAG operation failed during the Night Run.
    #[error("task DAG error: {0}")]
    DagError(String),

    /// Checkpoint file could not be read or written.
    #[error("checkpoint I/O failed at {path}: {reason}")]
    CheckpointIo {
        /// Path of the checkpoint file that could not be accessed.
        path: String,
        /// Human-readable I/O failure reason.
        reason: String,
    },

    /// The watchdog channel closed or sent a kill command.
    #[error("watchdog died: channel closed or kill command received")]
    WatchdogDied,

    /// A git operation via `gitoxide` failed.
    #[error("git operation failed: {0}")]
    GitOperationFailed(String),
}
