//! # forge-cli: Interactive Approval Gate
//!
//! Provides `ApprovalGate`, which presents a formatted approval box for
//! high-consequence actions (file writes, git commits, and sacred file writes)
//! and blocks on a single keypress in crossterm raw mode.
//!
//! ## Input
//! - `ApprovalMode` — `Interactive` blocks on a keypress; `Auto` accepts immediately
//! - `ApprovalKind` — describes the action requiring approval
//! - Optional estimated cost in USD via `with_cost`
//!
//! ## Output
//! - `ApprovalDecision` — one of `Accept`, `Reject`, `Edit`, or `Quit`
//!
//! ## Related
//! - `forge-cli::tui` — the ratatui Day-mode UI that wraps this gate
//! - `forge-cli::commands::run` — calls the gate before applying diffs

use std::io::{self, Write};

use crossterm::event::{read, Event, KeyCode, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

/// Describes the category of action requiring human approval.
#[derive(Debug, Clone)]
pub enum ApprovalKind {
    /// Write to a regular workspace file.
    FileWrite {
        /// Workspace-relative path of the file to be written.
        path: String,
    },
    /// Create a git commit.
    GitCommit {
        /// The commit message subject line.
        message: String,
    },
    /// Write to a file protected by the Sacred File Guard.
    SacredWrite {
        /// Workspace-relative path of the sacred file to be written.
        path: String,
    },
}

/// Determines whether `ApprovalGate::render_and_wait` blocks on input or auto-accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Print the approval box and block until the user presses a key.
    Interactive,
    /// Skip the prompt and immediately return `ApprovalDecision::Accept`.
    Auto,
}

/// The user's response to an approval prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The user accepted the action.
    Accept,
    /// The user rejected the action.
    Reject,
    /// The user wants to edit the proposed change before proceeding.
    Edit,
    /// The user wants to abort the current task entirely.
    Quit,
}

/// Gate that renders an approval box and returns the user's decision.
pub struct ApprovalGate {
    /// Determines interactive vs. automatic approval.
    pub mode: ApprovalMode,
    /// The action category requiring approval.
    pub kind: ApprovalKind,
    /// Optional pre-computed estimated cost of the action in USD.
    pub estimated_cost_usd: Option<f64>,
}

impl ApprovalGate {
    /// Creates a new `ApprovalGate` with no estimated cost.
    pub fn new(mode: ApprovalMode, kind: ApprovalKind) -> Self {
        Self {
            mode,
            kind,
            estimated_cost_usd: None,
        }
    }

    /// Attaches an estimated cost in USD to this gate, enabling it to be
    /// shown in the approval box.
    pub fn with_cost(mut self, cost_usd: f64) -> Self {
        self.estimated_cost_usd = Some(cost_usd);
        self
    }

    /// Renders the approval box and returns the user's decision.
    ///
    /// In `Auto` mode this returns `ApprovalDecision::Accept` immediately
    /// without printing anything or touching the terminal.
    ///
    /// # Errors
    ///
    /// Returns `anyhow::Error` if enabling/disabling raw mode or reading a
    /// crossterm event fails.
    pub fn render_and_wait(&self) -> anyhow::Result<ApprovalDecision> {
        if self.mode == ApprovalMode::Auto {
            return Ok(ApprovalDecision::Accept);
        }

        self.print_approval_box();

        io::stdout().flush()?;

        enable_raw_mode()?;

        let decision = loop {
            let terminal_event = read()?;
            if let Event::Key(KeyEvent { code, .. }) = terminal_event {
                match code {
                    KeyCode::Char('y' | 'Y') => break ApprovalDecision::Accept,
                    KeyCode::Char('n' | 'N') => break ApprovalDecision::Reject,
                    KeyCode::Char('e' | 'E') => break ApprovalDecision::Edit,
                    KeyCode::Char('q' | 'Q') | KeyCode::Esc => break ApprovalDecision::Quit,
                    _ => {}
                }
            }
        };

        disable_raw_mode()?;
        println!();

        Ok(decision)
    }

    /// Prints the formatted approval box to stdout.
    fn print_approval_box(&self) {
        let separator = "─".repeat(48);

        println!("{separator}");

        match &self.kind {
            ApprovalKind::SacredWrite { path } => {
                println!("⚠ SACRED FILE WRITE — APPROVAL REQUIRED");
                println!("  Action : Sacred file write");
                println!("  Path   : {path}");
            }
            ApprovalKind::FileWrite { path } => {
                println!("APPROVAL REQUIRED");
                println!("  Action : Write file");
                println!("  Path   : {path}");
            }
            ApprovalKind::GitCommit { message } => {
                println!("APPROVAL REQUIRED");
                println!("  Action : Git commit");
                println!("  Msg    : {message}");
            }
        }

        if let Some(cost_value) = self.estimated_cost_usd {
            println!("  Est. $ : ${cost_value:.6}");
        }

        println!("{separator}");
        print!("  [y] Accept  [n] Reject  [e] Edit  [q] Quit  ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_always_accepts() {
        let approval_gate = ApprovalGate::new(
            ApprovalMode::Auto,
            ApprovalKind::FileWrite {
                path: "src/lib.rs".to_owned(),
            },
        );
        let decision = approval_gate.render_and_wait().unwrap();
        assert_eq!(decision, ApprovalDecision::Accept);
    }

    #[test]
    fn with_cost_sets_estimated_cost() {
        let approval_gate = ApprovalGate::new(
            ApprovalMode::Auto,
            ApprovalKind::GitCommit {
                message: "feat: add auth".to_owned(),
            },
        )
        .with_cost(0.001);
        assert!(approval_gate.estimated_cost_usd.is_some());
    }
}
