//! # forge-cli::commands::console::tasks: Background Refresh Tasks
//!
//! Spawns the two always-running background tasks that feed the Yantra Console
//! side panel and footer with fresh data:
//!
//! - **Graph builder**: rebuilds the `GraphCache` from `.yantra/crg.sqlite`
//!   every 5 seconds and whenever a refresh nudge is sent (e.g. after `index`
//!   or `run` complete). All sync rusqlite work runs in `spawn_blocking`.
//! - **Telemetry poller**: opens `.yantra/traces.sqlite` read-only once per
//!   second and aggregates cost/throughput/error metrics.
//!
//! ## Input
//! - `crg_database_path: PathBuf` — `.yantra/crg.sqlite`
//! - `trace_database_path: PathBuf` — `.yantra/traces.sqlite`
//! - `thresholds: CostThresholds` — soft/hard/kill USD budget bands
//! - `graph_refresh_receiver: mpsc::UnboundedReceiver<()>` — nudge channel
//!
//! ## Output
//! - `GraphSnapshot` and `TelemetrySnapshot` sent over unbounded channels to
//!   the Console draw loop
//!
//! ## Related
//! - `forge-cli::commands::metrics` — compute helpers used inside `spawn_blocking`
//! - `forge-cli::commands::console::mod` — creates the channels and consumes the snapshots

use std::path::PathBuf;

use ratatui::style::Color;
use tokio::sync::mpsc;
use yantra_obs::CostThresholds;

use crate::commands::metrics::{
    compute_communities, compute_cumulative_cost, compute_error_rate, compute_gauge_ratio,
    compute_hubs, compute_spans_per_minute, compute_stats, cost_color, cost_status_label,
    try_load_spans, GraphStats, HubEntry,
};

/// Number of hub symbols shown in the Console graph side panel.
const CONSOLE_HUB_LIMIT: usize = 12;

/// A snapshot of the CRG graph state for rendering in the side panel.
pub(crate) struct GraphSnapshot {
    /// `true` when CRG index is present and was loaded successfully.
    pub(crate) available: bool,
    /// Top-line statistics, `None` when unavailable.
    pub(crate) stats: Option<GraphStats>,
    /// Community list sorted by symbol count descending.
    pub(crate) communities: Vec<(String, usize)>,
    /// Top hub symbols by connectivity score, capped at `CONSOLE_HUB_LIMIT`.
    pub(crate) hubs: Vec<HubEntry>,
}

impl GraphSnapshot {
    /// Returns an unavailable snapshot for use before the first successful load.
    pub(crate) fn unavailable() -> Self {
        Self {
            available: false,
            stats: None,
            communities: Vec::new(),
            hubs: Vec::new(),
        }
    }
}

/// A snapshot of the telemetry state for rendering in the footer.
pub(crate) struct TelemetrySnapshot {
    /// `true` when `traces.sqlite` exists and has data.
    pub(crate) available: bool,
    /// Cumulative USD cost of all recorded spans.
    pub(crate) cumulative_cost_usd: f64,
    /// Span count in the last 60 seconds.
    pub(crate) spans_per_minute: usize,
    /// Percentage of spans in the last 5 minutes that errored.
    pub(crate) error_rate_pct: f64,
    /// Gauge fill ratio clamped to `[0, 1]`.
    pub(crate) gauge_ratio: f64,
    /// Ratatui colour matching the cost band.
    pub(crate) gauge_color: Color,
    /// Human-readable cost status label (`"Ok"`, `"Warn"`, `"Pause"`, `"Kill"`).
    pub(crate) status_label: String,
}

impl TelemetrySnapshot {
    /// Returns an empty/unavailable snapshot for use before any spans are recorded.
    pub(crate) fn unavailable() -> Self {
        Self {
            available: false,
            cumulative_cost_usd: 0.0,
            spans_per_minute: 0,
            error_rate_pct: 0.0,
            gauge_ratio: 0.0,
            gauge_color: Color::Gray,
            status_label: "—".to_owned(),
        }
    }
}

/// Spawns the background graph-builder task.
///
/// The task wakes on a 5-second tick and on every `()` message received from
/// `graph_refresh_receiver`, rebuilds the `GraphCache` in `spawn_blocking`, and
/// publishes a new `GraphSnapshot`. A fresh read-only `Connection` is opened
/// inside each `spawn_blocking` closure to avoid holding a non-`Send`
/// `Connection` across `.await` points.
///
/// Both `interval.tick()` and `graph_refresh_receiver.recv()` are cancel-safe.
pub(crate) fn spawn_graph_builder(
    crg_database_path: PathBuf,
    mut graph_refresh_receiver: mpsc::UnboundedReceiver<()>,
    graph_snapshot_sender: mpsc::UnboundedSender<GraphSnapshot>,
) {
    tokio::spawn(async move {
        let mut rebuild_interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = rebuild_interval.tick() => {},
                nudge = graph_refresh_receiver.recv() => {
                    if nudge.is_none() {
                        break;
                    }
                }
            }

            let crg_path_clone = crg_database_path.clone();
            let snapshot = tokio::task::spawn_blocking(move || {
                if !crg_path_clone.exists() {
                    return GraphSnapshot::unavailable();
                }
                let connection = match rusqlite::Connection::open_with_flags(
                    &crg_path_clone,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                ) {
                    Ok(conn) => conn,
                    Err(db_error) => {
                        tracing::warn!(error = %db_error, "graph builder: could not open crg.sqlite");
                        return GraphSnapshot::unavailable();
                    }
                };
                let graph_cache = match yantra_crg::GraphCache::build(&connection) {
                    Ok(cache) => cache,
                    Err(build_error) => {
                        tracing::warn!(error = %build_error, "graph builder: GraphCache::build failed");
                        return GraphSnapshot::unavailable();
                    }
                };
                let graph_stats = compute_stats(&graph_cache);
                let communities = compute_communities(&graph_cache);
                let hubs = compute_hubs(&graph_cache, CONSOLE_HUB_LIMIT);
                GraphSnapshot {
                    available: true,
                    stats: Some(graph_stats),
                    communities,
                    hubs,
                }
            })
            .await
            .unwrap_or_else(|join_error| {
                tracing::warn!(error = %join_error, "graph builder: spawn_blocking panicked");
                GraphSnapshot::unavailable()
            });

            if graph_snapshot_sender.send(snapshot).is_err() {
                break;
            }
        }
    });
}

/// Spawns the background telemetry-polling task.
///
/// The task wakes every second, opens `trace_database_path` read-only in
/// `spawn_blocking`, loads spans, and publishes a new `TelemetrySnapshot`. A
/// fresh `Connection` per tick avoids the non-`Send` `Connection` issue.
pub(crate) fn spawn_telemetry_poller(
    trace_database_path: PathBuf,
    thresholds: CostThresholds,
    telemetry_snapshot_sender: mpsc::UnboundedSender<TelemetrySnapshot>,
) {
    tokio::spawn(async move {
        let mut poll_interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            poll_interval.tick().await;

            let trace_path_clone = trace_database_path.clone();
            let thresholds_clone = thresholds;
            let snapshot = tokio::task::spawn_blocking(move || {
                let now = chrono::Utc::now();
                let spans = match try_load_spans(&trace_path_clone) {
                    Ok(Some(loaded_spans)) => loaded_spans,
                    Ok(None) => return TelemetrySnapshot::unavailable(),
                    Err(load_error) => {
                        tracing::warn!(error = %load_error, "telemetry poller: span load failed");
                        return TelemetrySnapshot::unavailable();
                    }
                };
                let cumulative_cost_usd = compute_cumulative_cost(&spans);
                let spans_per_minute = compute_spans_per_minute(&spans, now);
                let error_rate_pct = compute_error_rate(&spans, now);
                let gauge_ratio = compute_gauge_ratio(cumulative_cost_usd, &thresholds_clone);
                let gauge_color = cost_color(cumulative_cost_usd, &thresholds_clone);
                let status_label =
                    cost_status_label(cumulative_cost_usd, &thresholds_clone).to_owned();
                TelemetrySnapshot {
                    available: true,
                    cumulative_cost_usd,
                    spans_per_minute,
                    error_rate_pct,
                    gauge_ratio,
                    gauge_color,
                    status_label,
                }
            })
            .await
            .unwrap_or_else(|join_error| {
                tracing::warn!(error = %join_error, "telemetry poller: spawn_blocking panicked");
                TelemetrySnapshot::unavailable()
            });

            if telemetry_snapshot_sender.send(snapshot).is_err() {
                break;
            }
        }
    });
}
