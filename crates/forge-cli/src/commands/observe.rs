//! # forge-cli: Live Observability TUI (`yantra observe`)
//!
//! Renders a live ratatui dashboard over the read-only OpenTelemetry trace
//! store at `.yantra/traces.sqlite`. Reloads the `spans` table roughly once per
//! second and aggregates cumulative cost, recent throughput, error rate, the
//! most expensive runs, and a scrollable recent-span list. Press `q` (or `Esc`)
//! to quit.
//!
//! ## Input
//! - `trace_database_path: PathBuf` — path to `.yantra/traces.sqlite` (opened read-only)
//! - `thresholds: yantra_obs::CostThresholds` — soft/hard/kill USD budget bounds
//!
//! ## Output
//! - `anyhow::Result<()>` — Ok after the user quits, or after an early friendly
//!   return when the database or `spans` table is absent
//! - Ratatui frames drawn to the alternate screen; terminal state always restored
//!
//! ## Related
//! - `forge-obs::cost::CostThresholds` — budget thresholds rendered in the gauge
//! - `forge-obs` traces schema — the `spans` table this reads from
//! - `forge-cli::tui` — the sibling Day-mode ratatui UI (shared crossterm/ratatui API)

use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph};
use ratatui::Terminal;
use rusqlite::{Connection, OpenFlags};
use yantra_obs::CostThresholds;

const RECENT_SPANS_WINDOW: usize = 20;
const TOP_EXPENSIVE_COUNT: usize = 5;
const THROUGHPUT_WINDOW_SECONDS: i64 = 60;
const ERROR_RATE_WINDOW_SECONDS: i64 = 300;

/// One row loaded from the `spans` table of `traces.sqlite`.
struct SpanRow {
    span_id: String,
    agent: Option<String>,
    model: Option<String>,
    cost_usd: Option<f64>,
    started_at: String,
    outcome: Option<String>,
}

/// Restores terminal state on drop so a `?` early-return still leaves cooked mode.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout_handle = std::io::stdout();
        stdout_handle.execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    fn restore(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        self.terminal.backend_mut().execute(LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Runs the live observability dashboard until the user quits.
///
/// Opens `trace_database_path` read-only to avoid contending with the live
/// trace writer, then renders a refreshing ratatui dashboard. Returns early
/// with a friendly message if the database file or the `spans` table is missing.
///
/// # Errors
///
/// Returns `anyhow::Error` if the database open fails, a query fails, or the
/// terminal backend fails to render.
pub async fn observe_command(
    trace_database_path: PathBuf,
    thresholds: CostThresholds,
) -> anyhow::Result<()> {
    if !trace_database_path.exists() {
        println!(
            "No telemetry found at {}. Run 'yantra run <task>' first.",
            trace_database_path.display()
        );
        return Ok(());
    }

    let connection =
        Connection::open_with_flags(&trace_database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    if !spans_table_exists(&connection)? {
        println!(
            "No telemetry spans recorded yet at {}. Run 'yantra run <task>' first.",
            trace_database_path.display()
        );
        return Ok(());
    }

    let mut terminal_guard = TerminalGuard::new()?;
    let mut all_spans = load_spans(&connection)?;
    let mut scroll_offset: usize = 0;
    let mut last_reload = Instant::now();

    loop {
        let cumulative_cost_usd = compute_cumulative_cost(&all_spans);
        let now = chrono::Utc::now();
        let spans_per_minute = compute_spans_per_minute(&all_spans, now);
        let error_rate = compute_error_rate(&all_spans, now);
        let top_expensive = compute_top_expensive(&all_spans);
        let recent_spans = compute_recent_lines(&all_spans);

        let max_scroll_offset = recent_spans.len().saturating_sub(1);
        if scroll_offset > max_scroll_offset {
            scroll_offset = max_scroll_offset;
        }

        let gauge_ratio = compute_gauge_ratio(cumulative_cost_usd, &thresholds);
        let gauge_color = cost_color(cumulative_cost_usd, &thresholds);
        let status_label = cost_status_label(cumulative_cost_usd, &thresholds);

        terminal_guard.terminal.draw(|frame| {
            let terminal_area = frame.size();

            let vertical_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Min(4),
                    Constraint::Length(3),
                ])
                .split(terminal_area);

            let top_bar_text = format!(
                " Live Observability   │   Spans/min: {spans_per_minute}   │   Error rate (5m): {error_rate:.1}%"
            );
            let top_bar_paragraph = Paragraph::new(top_bar_text)
                .block(Block::default().borders(Borders::ALL).title("Yantra observe"))
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(top_bar_paragraph, vertical_chunks[0]);

            let gauge_label = format!(
                "${cumulative_cost_usd:.4} / ${kill:.4} ({status_label})",
                kill = thresholds.kill
            );
            let cost_gauge = Gauge::default()
                .block(Block::default().borders(Borders::ALL).title("Cumulative Cost"))
                .gauge_style(Style::default().fg(gauge_color))
                .ratio(gauge_ratio)
                .label(gauge_label);
            frame.render_widget(cost_gauge, vertical_chunks[1]);

            let body_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(vertical_chunks[2]);

            let expensive_items: Vec<ListItem> = top_expensive
                .iter()
                .map(|expensive_line| ListItem::new(expensive_line.as_str()))
                .collect();
            let expensive_list = List::new(expensive_items)
                .block(Block::default().borders(Borders::ALL).title("Top 5 Expensive"))
                .style(Style::default().fg(Color::Magenta));
            frame.render_widget(expensive_list, body_chunks[0]);

            let recent_items: Vec<ListItem> = recent_spans
                .iter()
                .skip(scroll_offset)
                .map(|recent_line| ListItem::new(recent_line.as_str()))
                .collect();
            let recent_list = List::new(recent_items)
                .block(Block::default().borders(Borders::ALL).title("Recent Spans"))
                .style(Style::default().fg(Color::Gray));
            frame.render_widget(recent_list, body_chunks[1]);

            let help_paragraph = Paragraph::new(" ↑/↓ scroll · q quit")
                .block(Block::default().borders(Borders::ALL))
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(help_paragraph, vertical_chunks[3]);
        })?;

        if crossterm::event::poll(Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key_event) = crossterm::event::read()? {
                use crossterm::event::KeyCode;
                match key_event.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Down | KeyCode::PageDown => {
                        if scroll_offset < max_scroll_offset {
                            scroll_offset += 1;
                        }
                    }
                    KeyCode::Up | KeyCode::PageUp => {
                        scroll_offset = scroll_offset.saturating_sub(1);
                    }
                    _ => {}
                }
            }
        }

        if last_reload.elapsed() >= Duration::from_secs(1) {
            all_spans = load_spans(&connection)?;
            last_reload = Instant::now();
        }
    }

    terminal_guard.restore()?;
    Ok(())
}

/// Returns true when the `spans` table exists in the connected database.
fn spans_table_exists(connection: &Connection) -> anyhow::Result<bool> {
    let table_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='spans'",
        [],
        |sqlite_row| sqlite_row.get(0),
    )?;
    Ok(table_count > 0)
}

/// Loads every span ordered by start time, mapping each SQLite row to a `SpanRow`.
fn load_spans(connection: &Connection) -> anyhow::Result<Vec<SpanRow>> {
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
fn compute_cumulative_cost(spans: &[SpanRow]) -> f64 {
    spans
        .iter()
        .map(|span_row| span_row.cost_usd.unwrap_or(0.0))
        .sum()
}

/// Clamps the gauge fill ratio to `[0, 1]`, returning 0 when the kill bound is non-positive.
fn compute_gauge_ratio(cumulative_cost_usd: f64, thresholds: &CostThresholds) -> f64 {
    let kill_bound = f64::from(thresholds.kill);
    if kill_bound > 0.0 {
        (cumulative_cost_usd / kill_bound).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Maps cumulative cost to a gauge color: green below soft, yellow below hard, red otherwise.
fn cost_color(cumulative: f64, thresholds: &CostThresholds) -> Color {
    if cumulative < f64::from(thresholds.soft) {
        Color::Green
    } else if cumulative < f64::from(thresholds.hard) {
        Color::Yellow
    } else {
        Color::Red
    }
}

/// Human-readable status label matching the cost color bands.
fn cost_status_label(cumulative: f64, thresholds: &CostThresholds) -> &'static str {
    if cumulative < f64::from(thresholds.soft) {
        "Ok"
    } else if cumulative < f64::from(thresholds.hard) {
        "Warn"
    } else if cumulative < f64::from(thresholds.kill) {
        "Pause"
    } else {
        "Kill"
    }
}

/// Counts spans started within the last `THROUGHPUT_WINDOW_SECONDS` relative to `now`.
fn compute_spans_per_minute(spans: &[SpanRow], now: chrono::DateTime<chrono::Utc>) -> usize {
    spans
        .iter()
        .filter(|span_row| within_window(&span_row.started_at, now, THROUGHPUT_WINDOW_SECONDS))
        .count()
}

/// Fraction (as a percentage) of spans in the last 5 minutes whose outcome is a non-`Success` value.
fn compute_error_rate(spans: &[SpanRow], now: chrono::DateTime<chrono::Utc>) -> f64 {
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

/// Returns true when the outcome is present and not equal to "success" (case-insensitive).
fn is_error_outcome(outcome: Option<&str>) -> bool {
    match outcome {
        Some(outcome_value) => !outcome_value.eq_ignore_ascii_case("success"),
        None => false,
    }
}

/// True when `started_at` parses as RFC3339 and lies within `window_seconds` before `now`.
fn within_window(
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
fn compute_top_expensive(spans: &[SpanRow]) -> Vec<String> {
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

/// Formats one expensive-span line, falling back to a span-id prefix when the agent is missing.
fn format_expensive_line(span_row: &SpanRow) -> String {
    let cost = span_row.cost_usd.unwrap_or(0.0);
    match (&span_row.agent, &span_row.model) {
        (Some(agent_name), Some(model_name)) => format!("{agent_name}/{model_name} · ${cost:.4}"),
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

/// Renders the most recent spans (newest first) as `HH:MM:SS  agent  outcome  $cost` lines.
fn compute_recent_lines(spans: &[SpanRow]) -> Vec<String> {
    spans
        .iter()
        .rev()
        .take(RECENT_SPANS_WINDOW)
        .map(format_recent_line)
        .collect()
}

/// Formats one recent-span line for the scrollable list.
fn format_recent_line(span_row: &SpanRow) -> String {
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

    fn thresholds() -> CostThresholds {
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
        assert_eq!(compute_gauge_ratio(5.0, &thresholds()), 0.5);
        assert_eq!(compute_gauge_ratio(50.0, &thresholds()), 1.0);
        let zero_kill = CostThresholds {
            soft: 1.0,
            hard: 2.0,
            kill: 0.0,
        };
        assert_eq!(compute_gauge_ratio(5.0, &zero_kill), 0.0);
    }

    #[test]
    fn cost_color_bands() {
        assert_eq!(cost_color(0.5, &thresholds()), Color::Green);
        assert_eq!(cost_color(2.0, &thresholds()), Color::Yellow);
        assert_eq!(cost_color(9.0, &thresholds()), Color::Red);
    }

    #[test]
    fn cost_status_label_bands() {
        assert_eq!(cost_status_label(0.5, &thresholds()), "Ok");
        assert_eq!(cost_status_label(2.0, &thresholds()), "Warn");
        assert_eq!(cost_status_label(7.0, &thresholds()), "Pause");
        assert_eq!(cost_status_label(11.0, &thresholds()), "Kill");
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
        let recent = (now - chrono::Duration::seconds(30)).to_rfc3339();
        let spans = vec![
            span_with(Some(1.0), &recent, Some("Success")),
            span_with(Some(1.0), &recent, Some("Failed")),
        ];
        assert_eq!(compute_error_rate(&spans, now), 50.0);
    }

    #[test]
    fn spans_per_minute_excludes_old_spans() {
        let now = chrono::Utc::now();
        let recent = (now - chrono::Duration::seconds(10)).to_rfc3339();
        let stale = (now - chrono::Duration::seconds(120)).to_rfc3339();
        let spans = vec![
            span_with(Some(1.0), &recent, Some("Success")),
            span_with(Some(1.0), &stale, Some("Success")),
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
        let formatted = format_expensive_line(&span_row);
        assert!(formatted.starts_with("abcdef01"));
    }

    #[test]
    fn recent_lines_newest_first_and_capped() {
        let mut spans = Vec::new();
        for span_index in 0..25 {
            let started_at = format!("2026-05-29T00:00:{span_index:02}Z");
            spans.push(span_with(Some(1.0), &started_at, Some("Success")));
        }
        let recent = compute_recent_lines(&spans);
        assert_eq!(recent.len(), RECENT_SPANS_WINDOW);
    }
}
