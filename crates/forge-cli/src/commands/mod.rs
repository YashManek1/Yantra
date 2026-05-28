//! # forge-cli: Command Implementations
//!
//! Each submodule holds the implementation of one `yantra` subcommand.
//! The `main.rs` entry point routes `clap` matches here.
//!
//! ## Related
//! - `forge-cli::main` — parses CLI args and delegates to these modules

pub mod canvas;
pub mod graph;
pub mod observe;
pub mod run;
