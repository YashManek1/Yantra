//! # forge-cli::commands::console::app: Yantra Console Application State
//!
//! Pure, terminal-free application state for the Yantra Console TUI. All
//! methods are synchronous and have no I/O side-effects, making them easy to
//! unit-test. The draw loop in `mod.rs` calls these methods in response to
//! events and then passes the state to the renderer.
//!
//! ## Input
//! - Key events (character insertion, backspace, Enter, scroll, Esc)
//! - `GraphSnapshot` and `TelemetrySnapshot` from background tasks
//! - `AskEvent` tokens from the streaming ask pipeline
//!
//! ## Output
//! - Updated `ConsoleApp` fields that the renderer reads each frame
//!
//! ## Related
//! - `forge-cli::commands::console::render` — reads state and draws ratatui frames
//! - `forge-cli::commands::console::tasks` — produces `GraphSnapshot` / `TelemetrySnapshot`
//! - `forge-cli::commands::ask` — produces `AskEvent` tokens

use std::collections::VecDeque;

use crate::commands::console::tasks::{GraphSnapshot, TelemetrySnapshot};

/// Maximum number of output lines retained in the scrollback buffer.
const SCROLLBACK_CAPACITY: usize = 1000;

/// Visual category of a line in the conversation scrollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutputKind {
    /// A prompt echo of what the user typed.
    UserInput,
    /// A streamed model output token or assembled answer line.
    AskToken,
    /// A system message (boot notices, index progress, command output).
    System,
    /// A grounding or cross-role warning from the verifier.
    Warning,
    /// An error that occurred during a command.
    Error,
    /// The CRG subgraph preamble preceding the model answer.
    Subgraph,
}

/// A single line in the conversation scrollback.
#[derive(Debug, Clone)]
pub(crate) struct OutputLine {
    pub(crate) text: String,
    pub(crate) kind: OutputKind,
}

/// The current operational mode of the Console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConsoleMode {
    /// Ready for input.
    Idle,
    /// An ask is streaming tokens; input is disabled.
    AskInFlight,
    /// A blocking subcommand (run/night/canvas/observe/doctor) has suspended
    /// the TUI and is running in the foreground.
    Suspended,
}

/// A parsed console command derived from the user's input line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConsoleCommand {
    /// Send a natural-language question through the ask pipeline.
    Ask(String),
    /// Run the STVP + multi-agent DAG pipeline for a task description.
    Run(String),
    /// Build or rebuild the CRG index for an optional path (defaults to `.`).
    Index(Option<String>),
    /// Start Night Mode with optional comma-separated task descriptions.
    Night(Vec<String>),
    /// Open the visual canvas editor, optionally cloning a URL.
    Canvas(Option<String>),
    /// Open the live observability TUI.
    Observe,
    /// Run preflight health checks.
    Doctor,
    /// Print the help text into the scrollback.
    Help,
    /// Exit the Console.
    Quit,
    /// The user typed nothing (blank line).
    Empty,
}

/// The full mutable state of the Yantra Console.
pub(crate) struct ConsoleApp {
    /// Current content of the input box.
    input_buffer: String,
    /// Cursor position within `input_buffer` (character index).
    cursor_position: usize,
    /// Scrollback output buffer, capped at `SCROLLBACK_CAPACITY`.
    output_lines: VecDeque<OutputLine>,
    /// Number of lines scrolled up from the bottom.
    scroll_offset: usize,
    /// Latest CRG graph snapshot for the side panel.
    pub(crate) graph_snapshot: GraphSnapshot,
    /// Latest telemetry snapshot for the footer.
    pub(crate) telemetry_snapshot: TelemetrySnapshot,
    /// Current operational mode.
    pub(crate) mode: ConsoleMode,
    /// Whether the Console should exit on the next loop iteration.
    pub(crate) should_quit: bool,
}

impl ConsoleApp {
    /// Creates a new `ConsoleApp` with empty state and unavailable snapshots.
    pub(crate) fn new() -> Self {
        Self {
            input_buffer: String::new(),
            cursor_position: 0,
            output_lines: VecDeque::new(),
            scroll_offset: 0,
            graph_snapshot: GraphSnapshot::unavailable(),
            telemetry_snapshot: TelemetrySnapshot::unavailable(),
            mode: ConsoleMode::Idle,
            should_quit: false,
        }
    }

    /// Pushes a new line onto the scrollback, evicting the oldest line when the
    /// capacity cap is reached.
    pub(crate) fn push_output(&mut self, text: impl Into<String>, kind: OutputKind) {
        if self.output_lines.len() >= SCROLLBACK_CAPACITY {
            self.output_lines.pop_front();
        }
        self.output_lines.push_back(OutputLine {
            text: text.into(),
            kind,
        });
    }

    /// Appends a streamed ask token to the scrollback. If the last line is an
    /// `AskToken` line and the token contains no newline, it is appended in
    /// place. Embedded `\n` characters produce new `AskToken` lines.
    pub(crate) fn append_ask_token(&mut self, token: &str) {
        for (split_index, fragment) in token.split('\n').enumerate() {
            if split_index == 0 {
                if let Some(last_line) = self.output_lines.back_mut() {
                    if last_line.kind == OutputKind::AskToken {
                        last_line.text.push_str(fragment);
                        continue;
                    }
                }
                self.push_output(fragment.to_owned(), OutputKind::AskToken);
            } else {
                self.push_output(fragment.to_owned(), OutputKind::AskToken);
            }
        }
    }

    /// Inserts `character` at the current cursor position in the input buffer.
    pub(crate) fn insert_char(&mut self, character: char) {
        self.input_buffer.insert(self.cursor_position, character);
        self.cursor_position += 1;
    }

    /// Removes the character immediately before the cursor (backspace behaviour).
    pub(crate) fn backspace(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            self.input_buffer.remove(self.cursor_position);
        }
    }

    /// Moves the cursor one position to the left (bounded at zero).
    pub(crate) fn move_cursor_left(&mut self) {
        self.cursor_position = self.cursor_position.saturating_sub(1);
    }

    /// Moves the cursor one position to the right (bounded at buffer length).
    pub(crate) fn move_cursor_right(&mut self) {
        self.cursor_position = (self.cursor_position + 1).min(self.input_buffer.len());
    }

    /// Takes the current input buffer contents, leaving it empty, and resets
    /// the cursor. Returns the previous contents.
    pub(crate) fn take_input(&mut self) -> String {
        self.cursor_position = 0;
        std::mem::take(&mut self.input_buffer)
    }

    /// Returns the current input buffer as a string slice.
    pub(crate) fn input_buffer(&self) -> &str {
        &self.input_buffer
    }

    /// Returns the current cursor position within the input buffer.
    pub(crate) fn cursor_position(&self) -> usize {
        self.cursor_position
    }

    /// Returns all output lines in order (oldest first).
    pub(crate) fn output_lines(&self) -> &VecDeque<OutputLine> {
        &self.output_lines
    }

    /// Returns the current scroll offset (lines from the bottom).
    pub(crate) fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Scrolls the conversation pane up by one line.
    pub(crate) fn scroll_up(&mut self) {
        let max_offset = self.output_lines.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + 1).min(max_offset);
    }

    /// Scrolls the conversation pane down by one line.
    pub(crate) fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Replaces the current graph snapshot with the given one.
    pub(crate) fn apply_graph_snapshot(&mut self, snapshot: GraphSnapshot) {
        self.graph_snapshot = snapshot;
    }

    /// Replaces the current telemetry snapshot with the given one.
    pub(crate) fn apply_telemetry_snapshot(&mut self, snapshot: TelemetrySnapshot) {
        self.telemetry_snapshot = snapshot;
    }

    /// Parses a raw input string into a `ConsoleCommand`.
    ///
    /// Rules (in order):
    /// - Empty or whitespace-only → `Empty`.
    /// - `exit` / `quit` / `q` → `Quit`.
    /// - `help` / `?` → `Help`.
    /// - `index [path]` → `Index(Option<String>)`.
    /// - `run <task>` → `Run(task)`.
    /// - `ask <question>` → `Ask(question)`.
    /// - `night [task,task,...]` → `Night(Vec<String>)`.
    /// - `canvas [url]` → `Canvas(Option<String>)`.
    /// - `observe` → `Observe`.
    /// - `doctor` → `Doctor`.
    /// - Anything else → `Ask(raw_input)` (bare prose asks the codebase).
    pub(crate) fn parse_command(raw_input: &str) -> ConsoleCommand {
        let trimmed = raw_input.trim();
        if trimmed.is_empty() {
            return ConsoleCommand::Empty;
        }

        let (leading_word, rest_of_input) = match trimmed.split_once(' ') {
            Some((word, remainder)) => (word, remainder.trim()),
            None => (trimmed, ""),
        };

        match leading_word.to_ascii_lowercase().as_str() {
            "exit" | "quit" | "q" => ConsoleCommand::Quit,
            "help" | "?" => ConsoleCommand::Help,
            "index" => {
                if rest_of_input.is_empty() {
                    ConsoleCommand::Index(None)
                } else {
                    ConsoleCommand::Index(Some(rest_of_input.to_owned()))
                }
            }
            "run" => {
                if rest_of_input.is_empty() {
                    ConsoleCommand::Ask("run".to_owned())
                } else {
                    ConsoleCommand::Run(rest_of_input.to_owned())
                }
            }
            "ask" => {
                if rest_of_input.is_empty() {
                    ConsoleCommand::Empty
                } else {
                    ConsoleCommand::Ask(rest_of_input.to_owned())
                }
            }
            "night" => {
                let task_list: Vec<String> = if rest_of_input.is_empty() {
                    Vec::new()
                } else {
                    rest_of_input
                        .split(',')
                        .map(|task_item| task_item.trim().to_owned())
                        .filter(|task_item| !task_item.is_empty())
                        .collect()
                };
                ConsoleCommand::Night(task_list)
            }
            "canvas" => {
                if rest_of_input.is_empty() {
                    ConsoleCommand::Canvas(None)
                } else {
                    ConsoleCommand::Canvas(Some(rest_of_input.to_owned()))
                }
            }
            "observe" => ConsoleCommand::Observe,
            "doctor" => ConsoleCommand::Doctor,
            _ => ConsoleCommand::Ask(trimmed.to_owned()),
        }
    }
}

/// Returns the help text that is pushed into the scrollback on `help`.
pub(crate) fn help_text() -> &'static str {
    "Commands:
  <question>           Ask the codebase anything (default — just type prose)
  ask <question>       Same as above, explicit form
  run <task>           Run the full STVP + multi-agent pipeline
  index [path]         Build/refresh the CRG symbol index
  night [task,...]     Start autonomous Night Mode (Dawn Digest on completion)
  canvas [url]         Open the visual canvas editor (optionally cloning a URL)
  observe              Open the live telemetry TUI
  doctor               Run preflight health checks
  help / ?             Show this help
  quit / exit / q      Exit the Yantra Console

  Graph panel (right): auto-refreshes every 5 s + after index/run.
  Telemetry footer:    updates every 1 s from .yantra/traces.sqlite.
  PageUp/PageDown:     scroll conversation."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_empty_input_returns_empty() {
        assert_eq!(ConsoleApp::parse_command(""), ConsoleCommand::Empty);
        assert_eq!(ConsoleApp::parse_command("   "), ConsoleCommand::Empty);
    }

    #[test]
    fn parse_command_quit_variants() {
        assert_eq!(ConsoleApp::parse_command("quit"), ConsoleCommand::Quit);
        assert_eq!(ConsoleApp::parse_command("exit"), ConsoleCommand::Quit);
        assert_eq!(ConsoleApp::parse_command("q"), ConsoleCommand::Quit);
    }

    #[test]
    fn parse_command_help_variants() {
        assert_eq!(ConsoleApp::parse_command("help"), ConsoleCommand::Help);
        assert_eq!(ConsoleApp::parse_command("?"), ConsoleCommand::Help);
    }

    #[test]
    fn parse_command_index_no_path() {
        assert_eq!(
            ConsoleApp::parse_command("index"),
            ConsoleCommand::Index(None)
        );
    }

    #[test]
    fn parse_command_index_with_path() {
        assert_eq!(
            ConsoleApp::parse_command("index /some/path"),
            ConsoleCommand::Index(Some("/some/path".to_owned()))
        );
    }

    #[test]
    fn parse_command_run_with_task() {
        assert_eq!(
            ConsoleApp::parse_command("run add unit tests"),
            ConsoleCommand::Run("add unit tests".to_owned())
        );
    }

    #[test]
    fn parse_command_ask_with_question() {
        assert_eq!(
            ConsoleApp::parse_command("ask what does run_ask do?"),
            ConsoleCommand::Ask("what does run_ask do?".to_owned())
        );
    }

    #[test]
    fn parse_command_bare_prose_falls_through_to_ask() {
        assert_eq!(
            ConsoleApp::parse_command("explain the CRG subgraph extraction"),
            ConsoleCommand::Ask("explain the CRG subgraph extraction".to_owned())
        );
    }

    #[test]
    fn parse_command_night_with_tasks() {
        assert_eq!(
            ConsoleApp::parse_command("night fix bug A, add tests"),
            ConsoleCommand::Night(vec!["fix bug A".to_owned(), "add tests".to_owned()])
        );
    }

    #[test]
    fn parse_command_canvas_with_url() {
        assert_eq!(
            ConsoleApp::parse_command("canvas https://example.com"),
            ConsoleCommand::Canvas(Some("https://example.com".to_owned()))
        );
    }

    #[test]
    fn parse_command_canvas_no_url() {
        assert_eq!(
            ConsoleApp::parse_command("canvas"),
            ConsoleCommand::Canvas(None)
        );
    }

    #[test]
    fn parse_command_observe() {
        assert_eq!(
            ConsoleApp::parse_command("observe"),
            ConsoleCommand::Observe
        );
    }

    #[test]
    fn parse_command_doctor() {
        assert_eq!(ConsoleApp::parse_command("doctor"), ConsoleCommand::Doctor);
    }

    #[test]
    fn push_output_evicts_oldest_when_capacity_reached() {
        let mut app = ConsoleApp::new();
        for line_index in 0..=SCROLLBACK_CAPACITY {
            app.push_output(format!("line {line_index}"), OutputKind::System);
        }
        assert_eq!(app.output_lines.len(), SCROLLBACK_CAPACITY);
        assert_eq!(app.output_lines.front().unwrap().text, "line 1");
    }

    #[test]
    fn append_ask_token_accumulates_into_last_line() {
        let mut app = ConsoleApp::new();
        app.push_output(String::new(), OutputKind::AskToken);
        app.append_ask_token("Hello");
        app.append_ask_token(", world");
        assert_eq!(app.output_lines.back().unwrap().text, "Hello, world");
        assert_eq!(app.output_lines.len(), 1);
    }

    #[test]
    fn append_ask_token_splits_on_newline() {
        let mut app = ConsoleApp::new();
        app.append_ask_token("line1\nline2");
        assert_eq!(app.output_lines.len(), 2);
        assert_eq!(app.output_lines[0].text, "line1");
        assert_eq!(app.output_lines[1].text, "line2");
    }

    #[test]
    fn scroll_up_and_down_are_bounded() {
        let mut app = ConsoleApp::new();
        for line_index in 0..5 {
            app.push_output(format!("line {line_index}"), OutputKind::System);
        }
        app.scroll_up();
        app.scroll_up();
        assert_eq!(app.scroll_offset, 2);
        app.scroll_down();
        assert_eq!(app.scroll_offset, 1);
        app.scroll_down();
        app.scroll_down();
        assert_eq!(app.scroll_offset, 0);
    }

    #[test]
    fn insert_char_and_backspace_edit_buffer() {
        let mut app = ConsoleApp::new();
        app.insert_char('h');
        app.insert_char('i');
        assert_eq!(app.input_buffer(), "hi");
        app.backspace();
        assert_eq!(app.input_buffer(), "h");
    }

    #[test]
    fn take_input_clears_buffer_and_resets_cursor() {
        let mut app = ConsoleApp::new();
        app.insert_char('x');
        app.insert_char('y');
        let taken_text = app.take_input();
        assert_eq!(taken_text, "xy");
        assert_eq!(app.input_buffer(), "");
        assert_eq!(app.cursor_position(), 0);
    }
}
