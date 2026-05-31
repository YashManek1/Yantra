//! # forge-cli::commands::console::render: Yantra Console Renderer
//!
//! Pure rendering functions for the Yantra Console TUI. All functions take
//! an immutable `&ConsoleApp` and a mutable `&mut ratatui::Frame` and produce
//! no side-effects, making the layout easy to test with a `TestBackend`.
//!
//! ## Layout
//! ```text
//! ┌─────────────────────────────────┬──────────────────┐
//! │  Yantra Console (conversation)  │  Graph (side)    │
//! │  [output scrollback]            │  Stats           │
//! │  [output scrollback]            │  Communities     │
//! │                                 │  Hubs            │
//! ├─────────────────────────────────┴──────────────────┤
//! │  input > _                                         │
//! ├────────────────────────────────────────────────────┤
//! │  Telemetry footer (cost gauge + metrics line)      │
//! └────────────────────────────────────────────────────┘
//! ```
//!
//! ## Input
//! - `&ConsoleApp` — full application state
//! - `&mut ratatui::Frame` — ratatui rendering target
//!
//! ## Output
//! - Rendered terminal frames (side-effect on `Frame`)
//!
//! ## Related
//! - `forge-cli::commands::console::app` — `ConsoleApp` state source
//! - `forge-cli::commands::console::mod` — calls `render_console` each loop tick

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::commands::console::app::{ConsoleApp, ConsoleMode, OutputKind};

/// Renders the full Yantra Console for one frame.
///
/// This is the single entry point called by the draw loop each iteration.
pub(crate) fn render_console(frame: &mut Frame, app: &ConsoleApp) {
    let total_area = frame.size();

    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(total_area);

    let body_area = vertical_chunks[0];
    let input_area = vertical_chunks[1];
    let footer_area = vertical_chunks[2];

    let horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(body_area);

    let conversation_area = horizontal_chunks[0];
    let graph_panel_area = horizontal_chunks[1];

    render_conversation(frame, app, conversation_area);
    render_graph_panel(frame, app, graph_panel_area);
    render_input_box(frame, app, input_area);
    render_telemetry_footer(frame, app, footer_area);
}

/// Renders the conversation scrollback pane.
fn render_conversation(frame: &mut Frame, app: &ConsoleApp, area: ratatui::layout::Rect) {
    let output_lines: Vec<Line> = app
        .output_lines()
        .iter()
        .map(|output_line| {
            let line_style = match output_line.kind {
                OutputKind::UserInput => Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                OutputKind::AskToken => Style::default().fg(Color::White),
                OutputKind::System => Style::default().fg(Color::DarkGray),
                OutputKind::Warning => Style::default().fg(Color::Yellow),
                OutputKind::Error => Style::default().fg(Color::Red),
                OutputKind::Subgraph => Style::default().fg(Color::Blue),
            };
            Line::from(Span::styled(output_line.text.clone(), line_style))
        })
        .collect();

    let total_line_count = output_lines.len();
    let pane_height = area.height.saturating_sub(2) as usize;
    let scroll_offset = app
        .scroll_offset()
        .min(total_line_count.saturating_sub(pane_height));
    let display_start = total_line_count
        .saturating_sub(pane_height)
        .saturating_sub(scroll_offset);

    let visible_lines: Vec<Line> = output_lines.into_iter().skip(display_start).collect();

    let title = match app.mode {
        ConsoleMode::AskInFlight => " Yantra Console  [thinking…] ",
        ConsoleMode::Suspended => " Yantra Console  [suspended] ",
        ConsoleMode::Idle => " Yantra Console ",
    };

    let conversation_paragraph = Paragraph::new(visible_lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(conversation_paragraph, area);
}

/// Renders the input box below the conversation pane.
fn render_input_box(frame: &mut Frame, app: &ConsoleApp, area: ratatui::layout::Rect) {
    let prompt_prefix = "› ";
    let input_text = format!("{}{}", prompt_prefix, app.input_buffer());
    let input_style = if app.mode == ConsoleMode::Idle {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let input_paragraph = Paragraph::new(input_text.as_str())
        .block(Block::default().borders(Borders::ALL))
        .style(input_style);
    frame.render_widget(input_paragraph, area);

    if app.mode == ConsoleMode::Idle {
        let cursor_column = area.x + 1 + prompt_prefix.len() as u16 + app.cursor_position() as u16;
        let cursor_row = area.y + 1;
        if cursor_column < area.x + area.width && cursor_row < area.y + area.height {
            frame.set_cursor(cursor_column, cursor_row);
        }
    }
}

/// Renders the CRG graph side panel.
fn render_graph_panel(frame: &mut Frame, app: &ConsoleApp, area: ratatui::layout::Rect) {
    let graph_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(45),
            Constraint::Percentage(55),
        ])
        .split(area);

    let stats_area = graph_vertical_chunks[0];
    let communities_area = graph_vertical_chunks[1];
    let hubs_area = graph_vertical_chunks[2];

    if !app.graph_snapshot.available {
        let placeholder_paragraph = Paragraph::new("No CRG index\ntype `index` to build")
            .block(Block::default().borders(Borders::ALL).title(" Graph "))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(placeholder_paragraph, area);
        return;
    }

    let stats_text = if let Some(graph_stats) = &app.graph_snapshot.stats {
        format!(
            " S:{} E:{} C:{} F:{}",
            graph_stats.total_symbols,
            graph_stats.total_edges,
            graph_stats.community_count,
            graph_stats.file_count,
        )
    } else {
        " loading…".to_owned()
    };

    let stats_paragraph = Paragraph::new(stats_text)
        .block(Block::default().borders(Borders::ALL).title(" Graph "))
        .style(Style::default().fg(Color::Cyan));
    frame.render_widget(stats_paragraph, stats_area);

    let community_items: Vec<ListItem> = app
        .graph_snapshot
        .communities
        .iter()
        .map(|(community_name, symbol_count)| {
            ListItem::new(format!("{symbol_count:>4} {community_name}"))
        })
        .collect();
    let communities_list = List::new(community_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Communities "),
        )
        .style(Style::default().fg(Color::Green));
    frame.render_widget(communities_list, communities_area);

    let hub_items: Vec<ListItem> = app
        .graph_snapshot
        .hubs
        .iter()
        .map(|hub_entry| {
            ListItem::new(format!(
                "{:>4} {}",
                hub_entry.connectivity_score, hub_entry.name
            ))
        })
        .collect();
    let hubs_list = List::new(hub_items)
        .block(Block::default().borders(Borders::ALL).title(" Hubs "))
        .style(Style::default().fg(Color::Gray));
    frame.render_widget(hubs_list, hubs_area);
}

/// Renders the telemetry footer.
fn render_telemetry_footer(frame: &mut Frame, app: &ConsoleApp, area: ratatui::layout::Rect) {
    if !app.telemetry_snapshot.available {
        let no_telemetry_paragraph = Paragraph::new(
            " no traces yet · run a task to see telemetry  │  PageUp/Down scroll  │  Esc quit",
        )
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(no_telemetry_paragraph, area);
        return;
    }

    let metrics_line = format!(
        " ${:.4}  │  {}/min  │  err {:.1}%  │  {}  │  PageUp/Down scroll  │  Esc quit",
        app.telemetry_snapshot.cumulative_cost_usd,
        app.telemetry_snapshot.spans_per_minute,
        app.telemetry_snapshot.error_rate_pct,
        app.telemetry_snapshot.status_label,
    );

    let footer_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(Block::default().borders(Borders::ALL).inner(area));

    let gauge_widget = Gauge::default()
        .gauge_style(Style::default().fg(app.telemetry_snapshot.gauge_color))
        .ratio(app.telemetry_snapshot.gauge_ratio);
    let metrics_paragraph =
        Paragraph::new(metrics_line.as_str()).style(Style::default().fg(Color::DarkGray));

    let outer_block = Block::default().borders(Borders::ALL).title(" Telemetry ");
    let inner_area = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    let inner_vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner_area);

    if inner_vertical.len() >= 2 {
        frame.render_widget(gauge_widget, inner_vertical[0]);
        frame.render_widget(metrics_paragraph, inner_vertical[1]);
    }
    let _ = footer_vertical_chunks;
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::commands::console::app::ConsoleApp;

    #[test]
    fn render_console_contains_yantra_console_title() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = ConsoleApp::new();
        terminal.draw(|frame| render_console(frame, &app)).unwrap();
        let buffer_content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect();
        assert!(
            buffer_content.contains("Yantra Console"),
            "buffer should contain 'Yantra Console'"
        );
    }

    #[test]
    fn render_console_shows_no_crg_index_placeholder_when_unavailable() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = ConsoleApp::new();
        terminal.draw(|frame| render_console(frame, &app)).unwrap();
        let buffer_content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect();
        assert!(
            buffer_content.contains("No CRG"),
            "buffer should mention 'No CRG' when graph is unavailable"
        );
    }

    #[test]
    fn render_console_shows_no_traces_yet_when_telemetry_unavailable() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = ConsoleApp::new();
        terminal.draw(|frame| render_console(frame, &app)).unwrap();
        let buffer_content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect();
        assert!(
            buffer_content.contains("no traces"),
            "buffer should mention 'no traces' when telemetry is unavailable"
        );
    }
}
