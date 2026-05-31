//! # yantra-canvas: Visual Feature Host Entry Point
//!
//! Standalone binary that boots the canvas server with no project loaded.
//! Most users invoke the server indirectly through `yantra canvas` /
//! `yantra graph`, which embed this same library entry point via
//! `yantra_canvas::serve`. This binary is useful for manual testing and as
//! a long-lived server outside the CLI lifecycle.
//!
//! ## Input
//! - `--port <PORT>` — TCP port to bind (default: 8088)
//! - `--allow-remote` — bind to 0.0.0.0 instead of 127.0.0.1
//!
//! ## Output
//! - HTTP server listening; prints the canvas + graph URLs to stdout
//!
//! ## Related
//! - `yantra_canvas::server` — Axum router and `serve()` function
//! - `forge-cli::commands::canvas` — primary embedded caller

use std::net::SocketAddr;
use std::str::FromStr;

use clap::Parser;
use yantra_canvas::{serve, AppState};

#[derive(Parser)]
#[command(
    name = "yantra-canvas",
    about = "Visual canvas editor + CRG graph viewer host"
)]
struct CliArgs {
    /// TCP port to bind (default: 8088).
    #[arg(long, default_value_t = 8088)]
    port: u16,

    /// Bind on all network interfaces instead of localhost only.
    #[arg(long)]
    allow_remote: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli_arguments = CliArgs::parse();

    let bind_host = if cli_arguments.allow_remote {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    let bind_address_string = format!("{bind_host}:{}", cli_arguments.port);
    let bind_address = SocketAddr::from_str(&bind_address_string)?;

    let application_state = AppState::empty();

    println!("Yantra Canvas: http://{bind_address}/");
    println!("  Editor: http://{bind_address}/editor/<project>");
    println!("  Graph:  http://{bind_address}/graph");

    serve(application_state, bind_address)
        .await
        .map_err(|canvas_error| anyhow::anyhow!("{canvas_error}"))?;

    Ok(())
}
