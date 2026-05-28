//! # forge-night: Error Types
//!
//! Typed error variants covering every Night Mode operation: STVP validation
//! during Twilight, truth-token issuance, decision-rule collection, night-plan
//! generation, and user-confirmation handling.
//!
//! ## Input
//! - Lower-level errors from `forge-stvp` and UI interactions
//!
//! ## Output
//! - `NightError` returned by all public forge-night APIs
//!
//! ## Related
//! - `forge-night::twilight` — primary producer of `NightError` values
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
}
