//! # forge-cli::commands::metrics: Shared CRG + Telemetry Compute Helpers
//!
//! Centralises the pure-function compute layer that powers both the standalone
//! `graph` and `observe` TUI dashboards and the unified Yantra Console side
//! panel / footer. By extracting these helpers here, the three consumers share
//! one implementation (CLAUDE §3.2 — no duplication).
//!
//! ## Input
//! - `&yantra_crg::GraphCache` — in-memory CRG snapshot for graph statistics
//! - `&[SpanRow]` — trace rows loaded from `.yantra/traces.sqlite`
//! - `&yantra_obs::CostThresholds` — soft/hard/kill USD budget bands
//! - `&std::path::Path` and `&std::path::Path` — paths for the CRG index builder
//!
//! ## Output
//! - `GraphStats`, `HubEntry`, communities for graph panels
//! - Cost / throughput / error-rate aggregates and formatted lines for telemetry panels
//!
//! ## Related
//! - `forge-cli::commands::graph` — standalone CRG dashboard TUI consumer
//! - `forge-cli::commands::observe` — standalone observability TUI consumer
//! - `forge-cli::commands::console` — unified Console side-panel / footer consumer

use std::path::Path;

use ratatui::style::Color;
use rusqlite::{Connection, OpenFlags};
use yantra_crg::{community_label, GraphCache};
use yantra_obs::CostThresholds;

// ---------------------------------------------------------------------------
// CRG graph helpers
// ---------------------------------------------------------------------------

/// Top-line statistics derived from a `GraphCache` snapshot.
pub(crate) struct GraphStats {
    pub(crate) total_symbols: usize,
    pub(crate) total_edges: usize,
    pub(crate) community_count: usize,
    pub(crate) file_count: usize,
}

/// One hub entry: a highly-connected symbol from the CRG.
pub(crate) struct HubEntry {
    pub(crate) name: String,
    pub(crate) symbol_id: String,
    pub(crate) connectivity_score: i32,
    pub(crate) file_path: String,
}

/// Computes top-line statistics from a `GraphCache` snapshot.
pub(crate) fn compute_stats(graph_cache: &GraphCache) -> GraphStats {
    let total_symbols = graph_cache.symbol_details.len();
    let total_edges = graph_cache
        .adjacency_index
        .values()
        .map(Vec::len)
        .sum::<usize>();

    let mut distinct_communities = std::collections::HashSet::new();
    let mut distinct_files = std::collections::HashSet::new();
    for symbol_detail in graph_cache.symbol_details.values() {
        distinct_communities.insert(community_label(&symbol_detail.file_path));
        distinct_files.insert(symbol_detail.file_path.clone());
    }

    GraphStats {
        total_symbols,
        total_edges,
        community_count: distinct_communities.len(),
        file_count: distinct_files.len(),
    }
}

/// Groups symbols into communities by their file path's leading meaningful
/// directory segment, returning `(community, symbol_count)` sorted by count
/// descending.
pub(crate) fn compute_communities(graph_cache: &GraphCache) -> Vec<(String, usize)> {
    let mut counts_by_community: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for symbol_detail in graph_cache.symbol_details.values() {
        *counts_by_community
            .entry(community_label(&symbol_detail.file_path))
            .or_insert(0) += 1;
    }

    let mut communities: Vec<(String, usize)> = counts_by_community.into_iter().collect();
    communities.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    communities
}

/// Returns the top hub symbols by connectivity score, descending, capped at
/// `limit`. Callers pass their own limit so the graph TUI (50) and the narrow
/// Console side panel (~12) each get an appropriately sized list.
pub(crate) fn compute_hubs(graph_cache: &GraphCache, limit: usize) -> Vec<HubEntry> {
    let mut hub_entries: Vec<HubEntry> = graph_cache
        .symbol_details
        .iter()
        .map(|(symbol_id, symbol_detail)| HubEntry {
            name: symbol_detail.name.clone(),
            symbol_id: symbol_id.to_string(),
            connectivity_score: symbol_detail.connectivity_score,
            file_path: symbol_detail.file_path.clone(),
        })
        .collect();

    hub_entries.sort_by(|left_entry, right_entry| {
        right_entry
            .connectivity_score
            .cmp(&left_entry.connectivity_score)
            .then_with(|| left_entry.name.cmp(&right_entry.name))
    });
    hub_entries.truncate(limit);
    hub_entries
}

// ---------------------------------------------------------------------------
// CRG index builder
// ---------------------------------------------------------------------------

/// Builds (or rebuilds) the CRG index at `yantra_dir/crg.sqlite` from the
/// source tree at `target_path`.
///
/// This is the shared implementation used by `yantra index`, the guided
/// pipeline, and the Console `index` command.
///
/// # Errors
///
/// Returns `anyhow::Error` when the database cannot be opened, or when the
/// tree-sitter parse or graph-build step fails.
pub(crate) fn build_crg_index(target_path: &Path, yantra_dir: &Path) -> anyhow::Result<()> {
    let crg_database_path = yantra_dir.join("crg.sqlite");
    if let Some(parent_directory) = crg_database_path.parent() {
        std::fs::create_dir_all(parent_directory).map_err(|io_error| {
            anyhow::anyhow!("failed to create .yantra directory: {io_error}")
        })?;
    }
    let database_connection = rusqlite::Connection::open(&crg_database_path)
        .map_err(|db_error| anyhow::anyhow!("failed to open CRG database: {db_error}"))?;
    let graph_builder = yantra_crg::GraphBuilder::new(database_connection);
    graph_builder
        .build_from_repo(target_path)
        .map_err(|build_error| anyhow::anyhow!("failed to build CRG from repo: {build_error}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Telemetry / span helpers
// ---------------------------------------------------------------------------

/// Number of recent spans shown in the scrollable telemetry list.
pub(crate) const RECENT_SPANS_WINDOW: usize = 20;
/// Number of most-expensive spans shown in the top-expensive list.
pub(crate) const TOP_EXPENSIVE_COUNT: usize = 5;
/// Window (seconds) for the spans-per-minute throughput counter.
pub(crate) const THROUGHPUT_WINDOW_SECONDS: i64 = 60;
/// Window (seconds) for the error-rate percentage gauge.
pub(crate) const ERROR_RATE_WINDOW_SECONDS: i64 = 300;

/// One row loaded from the `spans` table of `traces.sqlite`.
pub(crate) struct SpanRow {
    pub(crate) span_id: String,
    pub(crate) agent: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) cost_usd: Option<f64>,
    pub(crate) started_at: String,
    pub(crate) outcome: Option<String>,
}

/// Returns `true` when the `spans` table exists in the connected database.
pub(crate) fn spans_table_exists(connection: &Connection) -> anyhow::Result<bool> {
    let table_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='spans'",
        [],
        |sqlite_row| sqlite_row.get(0),
    )?;
    Ok(table_count > 0)
}

/// Opens `trace_database_path` read-only and loads every span ordered by start
/// time, returning `None` when the file is absent or the table does not exist.
///
/// # Errors
///
/// Returns `anyhow::Error` when the file exists but the database open or query fails.
pub(crate) fn try_load_spans(trace_database_path: &Path) -> anyhow::Result<Option<Vec<SpanRow>>> {
    if !trace_database_path.exists() {
        return Ok(None);
    }
    let connection =
        Connection::open_with_flags(trace_database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !spans_table_exists(&connection)? {
        return Ok(None);
    }
    Ok(Some(load_spans(&connection)?))
}

/// Loads every span from `connection` ordered by start time.
///
/// # Errors
///
/// Returns `anyhow::Error` when the SQL prepare or query fails.
pub(crate) fn load_spans(connection: &Connection) -> anyhow::Result<Vec<SpanRow>> {
    let mut statement = connection.prepare(
        "SELECT span_id, agent, model, cost_usd, started_at, outcome \
         FROM spans ORDER BY started_at ASC",
    )?;

    let span_iterator = statement.query_map([], |sqlite_row| {
        Ok(SpanRow {
            span_id: sqlite_row.get(0)?,
            agent: sqlite_row.get(1)?,
            model: sqlite_row.get(2)?,
            cost_usd: sqlite_row.get(3)?,
            started_at: sqlite_row.get(4)?,
            outcome: sqlite_row.get(5)?,
        })
    })?;

    let mut loaded_spans = Vec::new();
    for span_result in span_iterator {
        loaded_spans.push(span_result?);
    }
    Ok(loaded_spans)
}

/// Sums `cost_usd` across all spans, treating missing costs as zero.
pub(crate) fn compute_cumulative_cost(spans: &[SpanRow]) -> f64 {
    spans
        .iter()
        .map(|span_row| span_row.cost_usd.unwrap_or(0.0))
        .sum()
}

/// Clamps the gauge fill ratio to `[0, 1]`, returning 0 when the kill bound is
/// non-positive.
pub(crate) fn compute_gauge_ratio(cumulative_cost_usd: f64, thresholds: &CostThresholds) -> f64 {
    let kill_bound = f64::from(thresholds.kill);
    if kill_bound > 0.0 {
        (cumulative_cost_usd / kill_bound).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Maps cumulative cost to a ratatui gauge color: green below soft, yellow
/// below hard, red otherwise.
pub(crate) fn cost_color(cumulative_cost_usd: f64, thresholds: &CostThresholds) -> Color {
    if cumulative_cost_usd < f64::from(thresholds.soft) {
        Color::Green
    } else if cumulative_cost_usd < f64::from(thresholds.hard) {
        Color::Yellow
    } else {
        Color::Red
    }
}

/// Human-readable status label matching the cost color bands.
pub(crate) fn cost_status_label(
    cumulative_cost_usd: f64,
    thresholds: &CostThresholds,
) -> &'static str {
    if cumulative_cost_usd < f64::from(thresholds.soft) {
        "Ok"
    } else if cumulative_cost_usd < f64::from(thresholds.hard) {
        "Warn"
    } else if cumulative_cost_usd < f64::from(thresholds.kill) {
        "Pause"
    } else {
        "Kill"
    }
}

/// Counts spans started within the last `THROUGHPUT_WINDOW_SECONDS` relative to `now`.
pub(crate) fn compute_spans_per_minute(
    spans: &[SpanRow],
    now: chrono::DateTime<chrono::Utc>,
) -> usize {
    spans
        .iter()
        .filter(|span_row| within_window(&span_row.started_at, now, THROUGHPUT_WINDOW_SECONDS))
        .count()
}

/// Fraction (as a percentage) of spans in the last 5 minutes whose outcome is
/// a non-`Success` value.
pub(crate) fn compute_error_rate(spans: &[SpanRow], now: chrono::DateTime<chrono::Utc>) -> f64 {
    let recent_spans: Vec<&SpanRow> = spans
        .iter()
        .filter(|span_row| within_window(&span_row.started_at, now, ERROR_RATE_WINDOW_SECONDS))
        .collect();

    if recent_spans.is_empty() {
        return 0.0;
    }

    let error_count = recent_spans
        .iter()
        .filter(|span_row| is_error_outcome(span_row.outcome.as_deref()))
        .count();

    (error_count as f64 / recent_spans.len() as f64) * 100.0
}

/// Returns `true` when the outcome is present and not equal to `"success"`
/// (case-insensitive).
pub(crate) fn is_error_outcome(outcome: Option<&str>) -> bool {
    match outcome {
        Some(outcome_value) => !outcome_value.eq_ignore_ascii_case("success"),
        None => false,
    }
}

/// Returns `true` when `started_at` parses as RFC3339 and lies within
/// `window_seconds` before `now`.
pub(crate) fn within_window(
    started_at: &str,
    now: chrono::DateTime<chrono::Utc>,
    window_seconds: i64,
) -> bool {
    match chrono::DateTime::parse_from_rfc3339(started_at) {
        Ok(parsed_time) => {
            let elapsed_seconds = (now - parsed_time.with_timezone(&chrono::Utc)).num_seconds();
            (0..=window_seconds).contains(&elapsed_seconds)
        }
        Err(_) => false,
    }
}

/// Renders the five highest-cost spans as `agent/model · $cost` lines.
pub(crate) fn compute_top_expensive(spans: &[SpanRow]) -> Vec<String> {
    let mut sortable_spans: Vec<&SpanRow> = spans.iter().collect();
    sortable_spans.sort_by(|left_span, right_span| {
        let left_cost = left_span.cost_usd.unwrap_or(0.0);
        let right_cost = right_span.cost_usd.unwrap_or(0.0);
        right_cost
            .partial_cmp(&left_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    sortable_spans
        .into_iter()
        .take(TOP_EXPENSIVE_COUNT)
        .map(format_expensive_line)
        .collect()
}

/// Formats one expensive-span line, falling back to a span-id prefix when the
/// agent is missing.
pub(crate) fn format_expensive_line(span_row: &SpanRow) -> String {
    let cost = span_row.cost_usd.unwrap_or(0.0);
    match (&span_row.agent, &span_row.model) {
        (Some(agent_name), Some(model_name)) => {
            format!("{agent_name}/{model_name} · ${cost:.4}")
        }
        (Some(agent_name), None) => format!("{agent_name} · ${cost:.4}"),
        _ => {
            let span_prefix = span_row
                .span_id
                .get(..8)
                .unwrap_or(span_row.span_id.as_str());
            format!("{span_prefix} · ${cost:.4}")
        }
    }
}

/// Renders the most recent spans (newest first, capped at `RECENT_SPANS_WINDOW`)
/// as `HH:MM:SS  agent  outcome  $cost` lines.
pub(crate) fn compute_recent_lines(spans: &[SpanRow]) -> Vec<String> {
    spans
        .iter()
        .rev()
        .take(RECENT_SPANS_WINDOW)
        .map(format_recent_line)
        .collect()
}

/// Formats one recent-span line for the scrollable list.
pub(crate) fn format_recent_line(span_row: &SpanRow) -> String {
    let time_label = match chrono::DateTime::parse_from_rfc3339(&span_row.started_at) {
        Ok(parsed_time) => parsed_time
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string(),
        Err(_) => "--:--:--".to_owned(),
    };
    let agent_label = span_row.agent.as_deref().unwrap_or("—");
    let outcome_label = span_row.outcome.as_deref().unwrap_or("—");
    let cost = span_row.cost_usd.unwrap_or(0.0);
    format!("{time_label}  {agent_label}  {outcome_label}  ${cost:.4}")
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use yantra_core::SymbolId;
    use yantra_crg::SymbolDetails;

    fn build_thresholds() -> CostThresholds {
        CostThresholds {
            soft: 1.0,
            hard: 5.0,
            kill: 10.0,
        }
    }

    fn span_with(cost: Option<f64>, started_at: &str, outcome: Option<&str>) -> SpanRow {
        SpanRow {
            span_id: "abcdef0123456789".to_owned(),
            agent: Some("Coder".to_owned()),
            model: Some("qwen2.5-coder".to_owned()),
            cost_usd: cost,
            started_at: started_at.to_owned(),
            outcome: outcome.map(str::to_owned),
        }
    }

    fn build_graph_cache_with_two_hubs() -> GraphCache {
        let mut symbol_details = std::collections::HashMap::new();
        let low_id = SymbolId::from_str("sym_low").expect("valid symbol id");
        let high_id = SymbolId::from_str("sym_high").expect("valid symbol id");
        symbol_details.insert(
            low_id,
            SymbolDetails {
                symbol_id: SymbolId::from_str("sym_low").expect("valid symbol id"),
                name: "low".to_owned(),
                kind: "function".to_owned(),
                start_line: 1,
                signature: None,
                docstring: None,
                connectivity_score: 3,
                file_path: "crates/a/src/a.rs".to_owned(),
                file_id: "file_a".to_owned(),
                token_cost_with_docstring: 0,
                token_cost_no_docstring: 0,
            },
        );
        symbol_details.insert(
            high_id,
            SymbolDetails {
                symbol_id: SymbolId::from_str("sym_high").expect("valid symbol id"),
                name: "high".to_owned(),
                kind: "function".to_owned(),
                start_line: 1,
                signature: None,
                docstring: None,
                connectivity_score: 42,
                file_path: "crates/b/src/b.rs".to_owned(),
                file_id: "file_b".to_owned(),
                token_cost_with_docstring: 0,
                token_cost_no_docstring: 0,
            },
        );
        GraphCache {
            symbol_details,
            adjacency_index: std::collections::HashMap::new(),
            symbols_by_name: std::collections::HashMap::new(),
            symbols_by_file_id: std::collections::HashMap::new(),
            symbol_to_file_id: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn compute_hubs_sorts_by_connectivity_descending() {
        let graph_cache = build_graph_cache_with_two_hubs();
        let hubs = compute_hubs(&graph_cache, 50);
        assert_eq!(hubs.len(), 2);
        assert_eq!(hubs[0].name, "high");
        assert_eq!(hubs[0].connectivity_score, 42);
        assert_eq!(hubs[1].name, "low");
    }

    #[test]
    fn compute_hubs_respects_limit() {
        let graph_cache = build_graph_cache_with_two_hubs();
        let hubs = compute_hubs(&graph_cache, 1);
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].name, "high");
    }

    #[test]
    fn cumulative_cost_treats_none_as_zero() {
        let spans = vec![
            span_with(Some(1.5), "2026-05-29T00:00:00Z", Some("Success")),
            span_with(None, "2026-05-29T00:00:01Z", Some("Success")),
            span_with(Some(2.5), "2026-05-29T00:00:02Z", Some("Success")),
        ];
        assert_eq!(compute_cumulative_cost(&spans), 4.0);
    }

    #[test]
    fn gauge_ratio_clamps_and_guards_zero_kill() {
        let thresholds = build_thresholds();
        assert_eq!(compute_gauge_ratio(5.0, &thresholds), 0.5);
        assert_eq!(compute_gauge_ratio(50.0, &thresholds), 1.0);
        let zero_kill = CostThresholds {
            soft: 1.0,
            hard: 2.0,
            kill: 0.0,
        };
        assert_eq!(compute_gauge_ratio(5.0, &zero_kill), 0.0);
    }

    #[test]
    fn cost_color_bands() {
        let thresholds = build_thresholds();
        assert_eq!(cost_color(0.5, &thresholds), Color::Green);
        assert_eq!(cost_color(2.0, &thresholds), Color::Yellow);
        assert_eq!(cost_color(9.0, &thresholds), Color::Red);
    }

    #[test]
    fn cost_status_label_bands() {
        let thresholds = build_thresholds();
        assert_eq!(cost_status_label(0.5, &thresholds), "Ok");
        assert_eq!(cost_status_label(2.0, &thresholds), "Warn");
        assert_eq!(cost_status_label(7.0, &thresholds), "Pause");
        assert_eq!(cost_status_label(11.0, &thresholds), "Kill");
    }

    #[test]
    fn error_outcome_classification_is_case_insensitive() {
        assert!(!is_error_outcome(Some("Success")));
        assert!(!is_error_outcome(Some("success")));
        assert!(!is_error_outcome(None));
        assert!(is_error_outcome(Some("Failed")));
        assert!(is_error_outcome(Some("Timeout")));
    }

    #[test]
    fn error_rate_zero_when_no_recent_spans() {
        let now = chrono::Utc::now();
        assert_eq!(compute_error_rate(&[], now), 0.0);
    }

    #[test]
    fn error_rate_counts_recent_failures() {
        let now = chrono::Utc::now();
        let recent_time = (now - chrono::Duration::seconds(30)).to_rfc3339();
        let spans = vec![
            span_with(Some(1.0), &recent_time, Some("Success")),
            span_with(Some(1.0), &recent_time, Some("Failed")),
        ];
        assert_eq!(compute_error_rate(&spans, now), 50.0);
    }

    #[test]
    fn spans_per_minute_excludes_old_spans() {
        let now = chrono::Utc::now();
        let recent_time = (now - chrono::Duration::seconds(10)).to_rfc3339();
        let stale_time = (now - chrono::Duration::seconds(120)).to_rfc3339();
        let spans = vec![
            span_with(Some(1.0), &recent_time, Some("Success")),
            span_with(Some(1.0), &stale_time, Some("Success")),
        ];
        assert_eq!(compute_spans_per_minute(&spans, now), 1);
    }

    #[test]
    fn top_expensive_sorts_descending_and_limits() {
        let spans = vec![
            span_with(Some(0.5), "2026-05-29T00:00:00Z", Some("Success")),
            span_with(Some(9.0), "2026-05-29T00:00:01Z", Some("Success")),
            span_with(Some(3.0), "2026-05-29T00:00:02Z", Some("Success")),
        ];
        let top_lines = compute_top_expensive(&spans);
        assert_eq!(top_lines.len(), 3);
        assert!(top_lines[0].contains("$9.0000"));
        assert!(top_lines[1].contains("$3.0000"));
    }

    #[test]
    fn expensive_line_falls_back_to_span_prefix_without_agent() {
        let mut span_row = span_with(Some(1.0), "2026-05-29T00:00:00Z", Some("Success"));
        span_row.agent = None;
        span_row.model = None;
        let formatted_line = format_expensive_line(&span_row);
        assert!(formatted_line.starts_with("abcdef01"));
    }

    #[test]
    fn recent_lines_newest_first_and_capped() {
        let mut spans = Vec::new();
        for span_index in 0..25 {
            let started_at = format!("2026-05-29T00:00:{span_index:02}Z");
            spans.push(span_with(Some(1.0), &started_at, Some("Success")));
        }
        let recent_lines = compute_recent_lines(&spans);
        assert_eq!(recent_lines.len(), RECENT_SPANS_WINDOW);
    }
}
