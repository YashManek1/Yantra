//! # forge-serve: Axum `HTTP` Server and `SSE` Live Canvas
//!
//! Exposes the Yantra runtime state over `HTTP` using Axum. Server-Sent Events
//! stream real-time agent progress, cost gauge updates, and `DAG` snapshots to
//! the browser-based Live Canvas. Also serves the `REST` `API` consumed by the
//! `IDE` plugin.
//!
//! ## Input
//! - Live `DAG` state and agent events from `forge-orchestrator`
//! - Cost gauge updates from `forge-obs`
//! - Night Mode progress events from `forge-night`
//!
//! ## Output
//! - `SSE` event stream on `GET /api/stream`
//! - `REST` endpoints for task submission and status polling
//! - Static assets for the Live Canvas UI
//!
//! ## Related
//! - `forge-orchestrator` — primary source of `DAG` state events
//! - `forge-night` — streams Night Run progress and the Dawn Digest
//! - `forge-obs` — cost gauge `SSE` updates originate here

pub mod error;
pub mod redirect;
pub mod server;
pub mod session;
pub mod sse;

pub use error::{ServeError, ServeResult};
pub use redirect::{redirect_channel, RedirectReceiver, RedirectSender, RedirectSignal};
pub use server::{build_router, serve, AppState};
pub use session::{
    empty_redirect_channels, empty_registry, RedirectChannels, SessionInfo, SessionRegistry,
};
