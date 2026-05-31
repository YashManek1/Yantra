//! # forge-cli: `yantra start` — Unified Console Launcher
//!
//! Single command that delivers the complete Yantra product through the
//! Yantra Console TUI — a unified ratatui application with:
//!
//! - A **conversation pane**: input box + scrollback for `ask` (CRG-grounded
//!   streaming Q&A) and `run` (STVP multi-agent pipeline), plus inline commands
//!   (`index`, `night`, `canvas`, `observe`, `doctor`, `help`, `quit`).
//! - A **persistent graph side panel** that auto-refreshes every 5 s and
//!   whenever `index` or `run` complete.
//! - A **live telemetry footer** updated every second from
//!   `.yantra/traces.sqlite`.
//!
//! When `task` is `Some`, the Console auto-submits `run <task>` on startup.
//! When `task` is `None`, the Console starts idle.
//!
//! ## Input
//! - `task: Option<String>` — auto-submitted task when provided
//! - `router: Arc<Router>` — pre-built model router
//! - `project_root: ProjectRoot` — workspace root
//! - `thresholds: CostThresholds` — soft/hard/kill budget bands for the footer
//! - `session_id: SessionId` — current session for span recording
//!
//! ## Output
//! - `anyhow::Result<()>` — returns when the user quits; terminal always restored
//!
//! ## Related
//! - `forge-cli::commands::console` — the Yantra Console TUI implementation
//! - `forge-cli::commands::doctor` — doctor check run at boot

use std::sync::Arc;

use yantra_core::{ProjectRoot, SessionId};
use yantra_obs::CostThresholds;
use yantra_router::Router;

use crate::commands::console::console_command;

/// Launches the Yantra Console TUI.
///
/// Ensures `.yantra/` exists, runs a quick doctor preflight check (results are
/// pushed into the Console's initial scrollback), then hands control to the
/// Console event loop.
///
/// When `task` is `Some`, the Console auto-submits `run <task>` on startup so
/// `yantra start "add unit tests"` goes straight into the pipeline.
///
/// # Errors
///
/// Returns `anyhow::Error` if the `.yantra/` directory cannot be created or if
/// the Console event loop encounters an unrecoverable terminal failure.
pub async fn start_command(
    task: Option<String>,
    router: Arc<Router>,
    project_root: ProjectRoot,
    thresholds: CostThresholds,
    session_id: SessionId,
) -> anyhow::Result<()> {
    let yantra_dir = project_root.as_path().join(".yantra");
    std::fs::create_dir_all(&yantra_dir)?;

    console_command(router, project_root, thresholds, session_id, task).await
}
