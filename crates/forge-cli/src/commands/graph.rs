//! # Graph Command: CRG Dashboard TUI
//!
//! Renders a ratatui dashboard summarising the Code-Review Graph (CRG): top-line
//! statistics, the largest communities (by symbol count), and the most-connected
//! hub symbols. Arrow keys navigate the hub list; `g` boots the canvas graph
//! viewer in a browser focused on the selected (or CLI-supplied) symbol; `q`
//! quits.
//!
//! ## Input
//! - `focus: Option<String>` — optional symbol id to focus the browser viewer on
//! - `crg_database_path: std::path::PathBuf` — path to `.yantra/crg.sqlite`
//! - `port: u16` — local TCP port the canvas graph viewer binds to on 127.0.0.1
//!
//! ## Output
//! - `anyhow::Result<()>` — runs the TUI until the user quits, restoring the
//!   terminal on every return path
//!
//! ## Related
//! - `forge-crg::GraphCache` — in-memory CRG snapshot read read-only from SQLite
//! - `forge-canvas::serve` — the long-running Axum graph viewer server
//! - `forge-cli::commands::metrics` — shared CRG compute helpers (compute_stats,
//!   compute_communities, compute_hubs)

use std::io::Stdout;

use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Terminal;
use rusqlite::{Connection, OpenFlags};
use yantra_crg::GraphCache;

use crate::commands::metrics::{compute_communities, compute_hubs, compute_stats};

/// Maximum number of hub symbols listed in the dashboard.
const HUB_LIMIT: usize = 50;

/// Renders the CRG dashboard TUI and, on `g`, opens the browser graph viewer.
///
/// Reads the graph read-only from `crg_database_path`. If the database is
/// missing, prints an actionable hint and returns. The terminal is always
/// restored before returning, including on error paths, via a `Drop` guard.
///
/// # Errors
///
/// Returns `anyhow::Error` if opening the database, building the `GraphCache`,
/// or any ratatui/crossterm operation fails.
pub async fn graph_command(
    focus: Option<String>,
    crg_database_path: std::path::PathBuf,
    port: u16,
) -> anyhow::Result<()> {
    if !crg_database_path.exists() {
        println!(
            "No CRG index found at {}. Run 'yantra index .' first.",
            crg_database_path.display()
        );
        return Ok(());
    }

    let connection =
        Connection::open_with_flags(&crg_database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let graph_cache = GraphCache::build(&connection)?;

    let graph_stats = compute_stats(&graph_cache);
    let communities = compute_communities(&graph_cache);
    let hubs = compute_hubs(&graph_cache, HUB_LIMIT);

    let mut terminal_guard = TerminalGuard::enter()?;
    run_event_loop(
        &mut terminal_guard.terminal,
        focus,
        port,
        &graph_stats,
        &communities,
        &hubs,
    )
}

/// RAII guard that enters raw mode + the alternate screen on construction and
/// restores the terminal on `Drop`, so every return path leaves the terminal
/// clean.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout_handle = std::io::stdout();
        stdout_handle.execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = self.terminal.backend_mut().execute(LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

/// Drives the draw/poll loop until the user quits.
fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    focus: Option<String>,
    port: u16,
    graph_stats: &crate::commands::metrics::GraphStats,
    communities: &[(String, usize)],
    hubs: &[crate::commands::metrics::HubEntry],
) -> anyhow::Result<()> {
    let cli_focus_id = focus;
    let mut selected_index: usize = 0;
    let mut server_started = false;

    loop {
        terminal.draw(|frame| {
            render_dashboard(frame, graph_stats, communities, hubs, selected_index);
        })?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key_event) = crossterm::event::read()? {
                use crossterm::event::KeyCode;
                match key_event.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Down => {
                        if !hubs.is_empty() {
                            selected_index = (selected_index + 1).min(hubs.len() - 1);
                        }
                    }
                    KeyCode::Up => {
                        selected_index = selected_index.saturating_sub(1);
                    }
                    KeyCode::Char('g') => {
                        let selected_focus_id = cli_focus_id.clone().or_else(|| {
                            hubs.get(selected_index)
                                .map(|hub_entry| hub_entry.symbol_id.clone())
                        });
                        open_graph_viewer(port, selected_focus_id, &mut server_started);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

/// Spawns the canvas graph viewer server (once) and opens the browser focused
/// on `selected_focus_id` when present.
fn open_graph_viewer(port: u16, selected_focus_id: Option<String>, server_started: &mut bool) {
    if !*server_started {
        let bind_address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let state = yantra_canvas::AppState::empty();
        tokio::spawn(async move {
            if let Err(serve_error) = yantra_canvas::serve(state, bind_address).await {
                tracing::error!(error = %serve_error, "graph viewer server exited");
            }
        });
        *server_started = true;
    }

    let mut browser_url = format!("http://127.0.0.1:{port}/graph");
    if let Some(focus_id) = selected_focus_id {
        browser_url.push_str(&format!("?focus={focus_id}"));
    }
    let _ = webbrowser::open(&browser_url);
}

/// Renders the four dashboard panes for the current frame.
fn render_dashboard(
    frame: &mut ratatui::Frame,
    graph_stats: &crate::commands::metrics::GraphStats,
    communities: &[(String, usize)],
    hubs: &[crate::commands::metrics::HubEntry],
    selected_index: usize,
) {
    let terminal_area = frame.size();

    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(terminal_area);

    let stats_text = format!(
        " Symbols: {}  │  Edges: {}  │  Communities: {}  │  Files: {}",
        graph_stats.total_symbols,
        graph_stats.total_edges,
        graph_stats.community_count,
        graph_stats.file_count,
    );
    let stats_paragraph = Paragraph::new(stats_text)
        .block(Block::default().borders(Borders::ALL).title("CRG Overview"))
        .style(Style::default().fg(Color::Cyan));
    frame.render_widget(stats_paragraph, vertical_chunks[0]);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(vertical_chunks[1]);

    let community_items: Vec<ListItem> = communities
        .iter()
        .map(|(community_name, symbol_count)| {
            ListItem::new(format!("{symbol_count:>5}  {community_name}"))
        })
        .collect();
    let community_list = List::new(community_items)
        .block(Block::default().borders(Borders::ALL).title("Communities"))
        .style(Style::default().fg(Color::Green));
    frame.render_widget(community_list, body_chunks[0]);

    let hub_items: Vec<ListItem> = hubs
        .iter()
        .map(|hub_entry| {
            ListItem::new(format!(
                "{:>5}  {}  ({})",
                hub_entry.connectivity_score, hub_entry.name, hub_entry.file_path
            ))
        })
        .collect();
    let hub_list = List::new(hub_items)
        .block(Block::default().borders(Borders::ALL).title("Hubs"))
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    let mut hub_list_state = ListState::default();
    if !hubs.is_empty() {
        hub_list_state.select(Some(selected_index.min(hubs.len() - 1)));
    }
    frame.render_stateful_widget(hub_list, body_chunks[1], &mut hub_list_state);

    let help_paragraph = Paragraph::new(" ↑/↓ navigate · g open browser graph · q quit")
        .block(Block::default().borders(Borders::ALL))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help_paragraph, vertical_chunks[2]);
}
