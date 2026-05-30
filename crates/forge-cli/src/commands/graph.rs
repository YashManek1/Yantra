//! # Graph Command: CRG Dashboard TUI
//!
//! Renders a ratatui dashboard summarizing the Code-Review Graph (CRG): top-line
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
//! - `forge-cli::tui` — the ratatui 0.27 widget/layout pattern this mirrors

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
use yantra_crg::{community_label, GraphCache};

/// Top-line CRG statistics shown in the dashboard header bar.
struct GraphStats {
    total_symbols: usize,
    total_edges: usize,
    community_count: usize,
    file_count: usize,
}

/// One hub entry: the most-connected symbols in the graph.
struct HubEntry {
    name: String,
    symbol_id: String,
    connectivity_score: i32,
    file_path: String,
}

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
    let hubs = compute_hubs(&graph_cache);

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
    graph_stats: &GraphStats,
    communities: &[(String, usize)],
    hubs: &[HubEntry],
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
    graph_stats: &GraphStats,
    communities: &[(String, usize)],
    hubs: &[HubEntry],
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

/// Computes top-line graph statistics from the cache.
fn compute_stats(graph_cache: &GraphCache) -> GraphStats {
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
fn compute_communities(graph_cache: &GraphCache) -> Vec<(String, usize)> {
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
/// `HUB_LIMIT`.
fn compute_hubs(graph_cache: &GraphCache) -> Vec<HubEntry> {
    let mut hubs: Vec<HubEntry> = graph_cache
        .symbol_details
        .iter()
        .map(|(symbol_id, symbol_detail)| HubEntry {
            name: symbol_detail.name.clone(),
            symbol_id: symbol_id.to_string(),
            connectivity_score: symbol_detail.connectivity_score,
            file_path: symbol_detail.file_path.clone(),
        })
        .collect();

    hubs.sort_by(|left, right| {
        right
            .connectivity_score
            .cmp(&left.connectivity_score)
            .then_with(|| left.name.cmp(&right.name))
    });
    hubs.truncate(HUB_LIMIT);
    hubs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_hubs_sorts_by_connectivity_descending() {
        use std::str::FromStr;
        use yantra_core::SymbolId;
        use yantra_crg::SymbolDetails;

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

        let graph_cache = GraphCache {
            symbol_details,
            adjacency_index: std::collections::HashMap::new(),
            symbols_by_name: std::collections::HashMap::new(),
            symbols_by_file_id: std::collections::HashMap::new(),
            symbol_to_file_id: std::collections::HashMap::new(),
        };

        let hubs = compute_hubs(&graph_cache);
        assert_eq!(hubs.len(), 2);
        assert_eq!(hubs[0].name, "high");
        assert_eq!(hubs[0].connectivity_score, 42);
        assert_eq!(hubs[1].name, "low");
    }
}
