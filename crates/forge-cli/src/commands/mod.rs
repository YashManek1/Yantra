//! # forge-cli: Command Implementations
//!
//! Each submodule holds the implementation of one `yantra` subcommand.
//! The `main.rs` entry point routes `clap` matches here.
//!
//! ## Modules
//! - `agent_runtime` — shared multi-agent scheduler constructor (no-duplication helper)
//! - `canvas` — `yantra canvas` visual editor server
//! - `context` — `yantra context` token-ledger view (Context Lens)
//! - `doctor` — `yantra doctor` preflight health checks
//! - `graph` — `yantra graph` CRG graph dashboard TUI
//! - `night` — `yantra night` autonomous Night Mode pipeline
//! - `observe` — `yantra observe` observability TUI
//! - `run` — `yantra run` full STVP + multi-agent DAG pipeline
//! - `start` — `yantra start` unified interactive shell + guided pipeline
//!
//! ## Related
//! - `forge-cli::main` — parses CLI args and delegates to these modules

pub mod agent_runtime;
pub mod canvas;
pub mod context;
pub mod doctor;
pub mod graph;
pub mod night;
pub mod observe;
pub mod run;
pub mod start;
