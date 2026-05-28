//! # yantra-serve: HTTP Server Entry Point
//!
//! Starts the Axum HTTP server that exposes the Yantra Live Canvas and REST API.
//! Binds to localhost by default; use `--allow-remote` to expose on all interfaces.
//!
//! ## Input
//! - `--port <PORT>` — TCP port to bind (default: 8088)
//! - `--allow-remote` — bind to 0.0.0.0 instead of 127.0.0.1
//!
//! ## Output
//! - HTTP server listening, serving the Live Canvas dashboard and SSE stream
//!
//! ## Related
//! - `yantra_serve::server` — Axum router and serve function
//! - `yantra_serve::session` — session registry

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use clap::Parser;
use tokio::sync::broadcast;
use yantra_orchestrator::AgentMessage;
use yantra_serve::server::{serve, AppState};
use yantra_serve::session::{empty_redirect_channels, empty_registry};

/// CLI arguments for the `yantra-serve` binary.
#[derive(Parser)]
#[command(name = "yantra-serve", about = "Yantra Live Canvas HTTP server")]
struct CliArgs {
    /// TCP port to bind (default: 8088)
    #[arg(long, default_value_t = 8088)]
    port: u16,

    /// Bind on all interfaces instead of localhost only
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

    let (event_sender, _initial_receiver) = broadcast::channel::<AgentMessage>(1024);

    let application_state = AppState {
        event_sender: event_sender.clone(),
        sessions: empty_registry(),
        redirect_channels: empty_redirect_channels(),
    };

    tokio::spawn(async move {
        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            heartbeat_interval.tick().await;
            let heartbeat_message = AgentMessage::DagUpdated { ready_count: 0 };
            if event_sender.send(heartbeat_message).is_err() {
                break;
            }
        }
    });

    println!("Yantra Live Canvas: http://{bind_address}");

    serve(application_state, bind_address)
        .await
        .map_err(|serve_error| anyhow::anyhow!("{serve_error}"))?;

    Ok(())
}
