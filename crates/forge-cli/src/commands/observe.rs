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
//! - `forge-cli::commands::metrics` — shared span compute helpers
//! - `forge-obs` traces schema — the `spans` table this reads from

use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph};
use ratatui::Terminal;
use rusqlite::{Connection, OpenFlags};
use yantra_obs::CostThresholds;

use crate::commands::metrics::{
    compute_cumulative_cost, compute_error_rate, compute_gauge_ratio, compute_recent_lines,
    compute_spans_per_minute, compute_top_expensive, cost_color, cost_status_label, load_spans,
    spans_table_exists,
};

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
                .style(Style::default().fg(ratatui::style::Color::Cyan));
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
                .style(Style::default().fg(ratatui::style::Color::Magenta));
            frame.render_widget(expensive_list, body_chunks[0]);

            let recent_items: Vec<ListItem> = recent_spans
                .iter()
                .skip(scroll_offset)
                .map(|recent_line| ListItem::new(recent_line.as_str()))
                .collect();
            let recent_list = List::new(recent_items)
                .block(Block::default().borders(Borders::ALL).title("Recent Spans"))
                .style(Style::default().fg(ratatui::style::Color::Gray));
            frame.render_widget(recent_list, body_chunks[1]);

            let help_paragraph = Paragraph::new(" ↑/↓ scroll · q quit")
                .block(Block::default().borders(Borders::ALL))
                .style(Style::default().fg(ratatui::style::Color::DarkGray));
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
