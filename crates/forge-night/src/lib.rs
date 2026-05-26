//! # forge-night: Night Mode Runtime
//!
//! Implements the three-phase autonomous execution loop: Twilight (front-load
//! STVP for all planned tasks, build decision rules with the user), Night Run
//! (execute the approved DAG without approval gates, checkpoint every 30s),
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
//! - `forge-orchestrator` — Night Run drives the DAG scheduler
//! - `forge-serve` — streams progress to the Live Canvas during the run
