//! # forge-sidecar: Always-On Background Daemon
//!
//! Runs as a separate process connected to the Yantra runtime via Unix socket.
//! Watches the filesystem for saves, triggers incremental CRG updates, polls
//! LSP for new diagnostics, and notifies the Live Canvas of regressions.
//!
//! ## Input
//! - Filesystem events from the OS file-watch subsystem
//! - Heartbeat pings from the main Yantra orchestrator process
//!
//! ## Output
//! - Incremental CRG update commands forwarded to `forge-crg`
//! - LSP diagnostic push events forwarded to `forge-lsp`
//! - SSE notifications posted to the Live Canvas via `forge-serve`
//!
//! ## Related
//! - `forge-crg` — receives incremental update commands
//! - `forge-lsp` — receives diagnostic-refresh commands
//! - `forge-memory` — flushes working memory checkpoints

fn main() {}
