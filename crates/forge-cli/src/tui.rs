//! # forge-cli: Ratatui Day-Mode Terminal UI
//!
//! Drives the `RunTui` terminal interface used by `yantra run` to display
//! session information, streaming agent progress, a scrollable event log, and
//! interactive approval prompts.
//!
//! ## Input
//! - `SessionId` — identifies the active session shown in the header bar
//! - Incremental state updates via `update_*` / `push_log` methods
//! - `set_approval_prompt` / `set_error` for the bottom action bar
//!
//! ## Output
//! - Ratatui frames rendered to the alternate screen via `CrosstermBackend`
//! - Raw-mode terminal state restored on drop via `restore()`
//!
//! ## Related
//! - `forge-cli::commands::run` — calls `RunTui` methods during the pipeline
//! - `forge-cli::approval` — renders the inline approval gate prompt

use std::collections::VecDeque;
use std::io::Stdout;

use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Terminal;
use yantra_core::{AgentKind, ModelTier, SessionId};

/// Maximum number of log lines retained in the scrolling pane.
const LOG_CAPACITY: usize = 200;

/// Full ratatui-based Day-mode UI for `yantra run`.
///
/// Call `render()` after each state mutation to push a new frame. Call
/// `restore()` (or rely on `Drop`) to leave the alternate screen cleanly.
pub struct RunTui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    session_id: SessionId,
    active_agent: Option<AgentKind>,
    current_task: String,
    current_step: String,
    cost_usd: f64,
    active_tier: ModelTier,
    log_lines: VecDeque<String>,
    approval_prompt: Option<String>,
    error_message: Option<String>,
}

impl RunTui {
    /// Initialises raw mode, enters the alternate screen, and returns a
    /// fully-wired `RunTui` ready for rendering.
    ///
    /// # Errors
    ///
    /// Returns `anyhow::Error` if crossterm or ratatui setup fails.
    pub fn new(session_id: SessionId) -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout_handle = std::io::stdout();
        stdout_handle.execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let terminal = Terminal::new(backend)?;

        Ok(Self {
            terminal,
            session_id,
            active_agent: None,
            current_task: String::new(),
            current_step: String::new(),
            cost_usd: 0.0,
            active_tier: ModelTier::Tier0,
            log_lines: VecDeque::with_capacity(LOG_CAPACITY),
            approval_prompt: None,
            error_message: None,
        })
    }

    /// Updates the currently active agent shown in the header.
    pub fn update_agent(&mut self, agent: AgentKind) {
        self.active_agent = Some(agent);
    }

    /// Updates the task description shown in the header (truncated to 50 chars in render).
    pub fn update_task(&mut self, description: &str) {
        description.clone_into(&mut self.current_task);
    }

    /// Updates the current step label shown beneath the task line.
    pub fn update_step(&mut self, step: &str) {
        step.clone_into(&mut self.current_step);
    }

    /// Updates the cumulative cost and active model tier shown in the top bar.
    pub fn update_cost(&mut self, cost_usd: f64, tier: ModelTier) {
        self.cost_usd = cost_usd;
        self.active_tier = tier;
    }

    /// Prepends a timestamped line to the log pane, evicting the oldest entry
    /// when the buffer exceeds `LOG_CAPACITY`.
    pub fn push_log(&mut self, line: impl Into<String>) {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        let timestamped_line = format!("{timestamp} {}", line.into());
        if self.log_lines.len() >= LOG_CAPACITY {
            self.log_lines.pop_front();
        }
        self.log_lines.push_back(timestamped_line);
    }

    /// Sets (or clears) the error message displayed in the bottom action bar.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error_message = Some(message.into());
    }

    /// Sets or clears the approval prompt shown in the bottom action bar.
    ///
    /// Pass `None` to revert to the default "Press Ctrl-C to abort" hint.
    pub fn set_approval_prompt(&mut self, prompt: Option<String>) {
        self.approval_prompt = prompt;
    }

    /// Draws one frame to the terminal using the current internal state.
    ///
    /// # Errors
    ///
    /// Returns `anyhow::Error` if the underlying `Terminal::draw` call fails.
    pub fn render(&mut self) -> anyhow::Result<()> {
        let session_id_display = self.session_id.to_string();
        let session_id_prefix = &session_id_display[..session_id_display.len().min(8)];
        let cost_snapshot = self.cost_usd;
        let tier_snapshot = self.active_tier;
        let agent_snapshot = self.active_agent;
        let task_snapshot = self.current_task.clone();
        let step_snapshot = self.current_step.clone();
        let log_snapshot: Vec<String> = self.log_lines.iter().cloned().collect();
        let approval_snapshot = self.approval_prompt.clone();
        let error_snapshot = self.error_message.clone();

        let top_bar_text = format!(
            " Session: {session_id_prefix}  │  Cost: ${cost_snapshot:.4}  │  Model: {}",
            tier_label(tier_snapshot),
        );

        self.terminal.draw(|frame| {
            let terminal_area = frame.size();

            let vertical_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(2),
                    Constraint::Min(4),
                    Constraint::Length(3),
                ])
                .split(terminal_area);

            let top_bar_paragraph = Paragraph::new(top_bar_text.as_str())
                .block(Block::default().borders(Borders::ALL).title("Yantra"))
                .style(Style::default().fg(Color::Cyan));
            frame.render_widget(top_bar_paragraph, vertical_chunks[0]);

            let agent_label = agent_snapshot.map_or_else(|| "—".to_owned(), |agent_kind| format!("{agent_kind:?}"));
            let task_truncated: String = task_snapshot.chars().take(50).collect();
            let step_truncated: String = step_snapshot.chars().take(60).collect();

            let header_lines = vec![
                Line::from(format!(" Agent: {agent_label}   Task: {task_truncated}")),
                Line::from(format!(" Step:  {step_truncated}")),
            ];
            let header_paragraph =
                Paragraph::new(header_lines).style(Style::default().fg(Color::Yellow));
            frame.render_widget(header_paragraph, vertical_chunks[1]);

            let log_items: Vec<ListItem> = log_snapshot
                .iter()
                .rev()
                .map(|log_entry| ListItem::new(log_entry.as_str()))
                .collect();
            let log_list = List::new(log_items)
                .block(Block::default().borders(Borders::TOP))
                .style(Style::default().fg(Color::Gray));
            frame.render_widget(log_list, vertical_chunks[2]);

            let (bottom_bar_text, bottom_bar_style) = if let Some(ref error_text) = error_snapshot {
                (format!("✗ {error_text}"), Style::default().fg(Color::Red))
            } else if let Some(ref prompt_text) = approval_snapshot {
                (prompt_text.clone(), Style::default().fg(Color::Green))
            } else {
                (
                    " Press Ctrl-C to abort".to_owned(),
                    Style::default().fg(Color::DarkGray),
                )
            };

            let bottom_bar_paragraph = Paragraph::new(bottom_bar_text.as_str())
                .block(Block::default().borders(Borders::ALL))
                .style(bottom_bar_style);
            frame.render_widget(bottom_bar_paragraph, vertical_chunks[3]);
        })?;

        Ok(())
    }

    /// Leaves the alternate screen, disables raw mode, and shows the cursor.
    ///
    /// # Errors
    ///
    /// Returns `anyhow::Error` if any crossterm teardown step fails.
    pub fn restore(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        self.terminal.backend_mut().execute(LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for RunTui {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Maps a `ModelTier` to its short human-readable label shown in the header bar.
fn tier_label(tier: ModelTier) -> &'static str {
    match tier {
        ModelTier::Tier0 => "T0/Local",
        ModelTier::Tier1 => "T1/Free",
        ModelTier::Tier2 => "T2/Cloud",
        ModelTier::Tier3 => "T3/Frontier",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_label_covers_all_variants() {
        assert_eq!(tier_label(ModelTier::Tier0), "T0/Local");
        assert_eq!(tier_label(ModelTier::Tier1), "T1/Free");
        assert_eq!(tier_label(ModelTier::Tier2), "T2/Cloud");
        assert_eq!(tier_label(ModelTier::Tier3), "T3/Frontier");
    }

    #[test]
    fn push_log_prepends_timestamp_and_caps_at_capacity() {
        let session_id = SessionId::new();
        let mut run_tui = RunTui {
            terminal: {
                let backend = CrosstermBackend::new(std::io::stdout());
                Terminal::new(backend).expect("terminal construction should succeed in test")
            },
            session_id,
            active_agent: None,
            current_task: String::new(),
            current_step: String::new(),
            cost_usd: 0.0,
            active_tier: ModelTier::Tier0,
            log_lines: VecDeque::with_capacity(LOG_CAPACITY),
            approval_prompt: None,
            error_message: None,
        };

        for push_index in 0..=LOG_CAPACITY {
            run_tui.push_log(format!("entry {push_index}"));
        }

        assert_eq!(run_tui.log_lines.len(), LOG_CAPACITY);
        let most_recent_entry = run_tui.log_lines.back().expect("log must not be empty");
        assert!(most_recent_entry.contains(&format!("entry {LOG_CAPACITY}")));
    }
}
