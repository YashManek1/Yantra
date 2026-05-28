//! # forge-night: Night Mode Runtime
//!
//! Implements the three-phase autonomous execution loop: Twilight (front-load
//! `STVP` for all planned tasks, build decision rules with the user), Night Run
//! (execute the approved `DAG` without approval gates, checkpoint every 30s),
//! and Dawn Digest (generate a markdown report and trigger the Postmortem).
//!
//! ## Input
//! - A set of planned tasks with associated `TruthToken`s from Twilight
//! - Decision rules agreed with the user that resolve ambiguity autonomously
//! - An optional user-set wake time or token budget ceiling
//!
//! ## Output
//! - Completed, Deferred, or Halted task disposition in the Dawn Digest
//! - A `dawn_digest.md` report written to the project directory
//! - Postmortem snippets forwarded to `forge-memory` Mistake Library
//!
//! ## Related
//! - `forge-orchestrator` — Night Run drives the `DAG` scheduler
//! - `forge-serve` — streams progress to the Live Canvas during the run

pub mod checkpointing;
pub mod decision_rules;
pub mod error;
pub mod mode_policy;
pub mod night_branch;
pub mod night_run;
pub mod twilight;

pub use checkpointing::{Checkpointer, TaskCheckpoint};
pub use decision_rules::{DecisionRule, NightEvent, RuleAction, RuleCondition, RuleEngine};
pub use error::NightError;
pub use mode_policy::{
    ApprovalMode, ModePolicy, SacredPolicy, DAY_POLICY, NIGHT_POLICY, STRICT_POLICY, TRUST_POLICY,
};
pub use night_branch::{
    create_night_branch, format_dawn_digest_branch_section, record_verified_commit, NightBranchRef,
};
pub use night_run::{
    run_night, HaltReason, NightReport, TaskDisposition, TaskExecutor, TaskOutcome,
};
pub use twilight::{run_twilight, NightSession, TaskSpec, TwilightUi, ValidatedGoal};
