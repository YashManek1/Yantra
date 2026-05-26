//! # forge-cli: Yantra Command-Line Interface
//!
//! Entry point for the `yantra` binary. Uses clap for argument parsing and
//! ratatui for the interactive terminal UI. Delegates all runtime logic to
//! `forge-orchestrator`, `forge-night`, and `forge-serve`.
//!
//! ## Input
//! - CLI arguments: subcommand (index, ask, run, night), flags, and options
//! - Interactive terminal input during `STVP` questionnaires
//!
//! ## Output
//! - Terminal UI rendered via ratatui
//! - Exit code 0 on success, non-zero on task failure or user abort
//!
//! ## Related
//! - `forge-orchestrator` — receives task submissions
//! - `forge-night` — starts Night Mode on `yantra night`
//! - `forge-serve` — optionally launched for the Live Canvas UI

fn main() {}
