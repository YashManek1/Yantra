//! # forge-cli: Command Implementations
//!
//! Each submodule holds the implementation of one `yantra` subcommand or a
//! shared helper module used across multiple commands.
//!
//! ## Modules
//! - `agent_runtime` — shared multi-agent scheduler constructor (no-duplication helper)
//! - `ask` — reusable streaming CRG-grounded ask pipeline (used by `Commands::Ask` and
//!   the Console TUI)
//! - `canvas` — `yantra canvas` visual editor server
//! - `console` — Yantra Console unified TUI (conversation + live graph + telemetry)
//! - `context` — `yantra context` token-ledger view (Context Lens)
//! - `doctor` — `yantra doctor` preflight health checks
//! - `graph` — `yantra graph` CRG graph dashboard TUI
//! - `metrics` — shared CRG + telemetry compute helpers (no-duplication per §3.2)
//! - `night` — `yantra night` autonomous Night Mode pipeline
//! - `observe` — `yantra observe` observability TUI
//! - `run` — `yantra run` full STVP + multi-agent DAG pipeline
//! - `start` — `yantra start` unified Console launcher
//!
//! ## Related
//! - `forge-cli::main` — parses CLI args and delegates to these modules

pub mod agent_runtime;
pub mod ask;
pub mod canvas;
pub mod console;
pub mod context;
pub mod doctor;
pub mod graph;
pub mod metrics;
pub mod night;
pub mod observe;
pub mod run;
pub mod start;
