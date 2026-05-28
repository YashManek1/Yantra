//! # forge-cli: Unified Diff Preview and Per-Hunk Approval
//!
//! Parses a unified diff into discrete hunks, renders each hunk to the terminal
//! with syntax highlighting via `syntect`, and collects an accept/reject decision
//! from the user for every hunk via `crossterm` raw-mode key input.
//!
//! ## Input
//! - `file_path: String` — the path label shown in the preview header
//! - `unified_diff: String` — a standard unified diff (`--- / +++ / @@ …` format)
//!
//! ## Output
//! - `Vec<HunkDecision>` — one decision per hunk processed before the user quits
//!
//! ## Related
//! - `forge-verifier` — consumes `HunkDecision` results to apply selective patches
//! - `forge-stvp` — gates diff application behind a valid `TruthToken`

use std::io::{self, Write};

use crossterm::event::{read, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

/// A single `@@` hunk extracted from a unified diff.
#[derive(Debug, Clone)]
pub struct Hunk {
    /// The raw `@@ -a,b +c,d @@` header line.
    pub header: String,
    /// All content lines belonging to this hunk.
    pub lines: Vec<HunkLine>,
}

/// A line within a diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    /// An unchanged context line (space-prefixed in the diff).
    Context(String),
    /// A line added in the new version (`+`-prefixed in the diff).
    Added(String),
    /// A line removed from the old version (`-`-prefixed in the diff).
    Removed(String),
}

/// Per-hunk decision made by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkDecision {
    /// Zero-based index of the hunk within the parsed hunk list.
    pub hunk_index: usize,
    /// `true` when the user accepted or chose to edit this hunk; `false` when rejected.
    pub accepted: bool,
}

/// Wrapper around a unified diff for interactive terminal display and approval.
pub struct DiffPreview {
    /// The file path label displayed in the preview header.
    pub file_path: String,
    /// The raw unified diff text to parse and display.
    pub unified_diff: String,
}

impl DiffPreview {
    /// Creates a new `DiffPreview` from a file path label and raw unified diff text.
    pub fn new(file_path: String, unified_diff: String) -> Self {
        Self {
            file_path,
            unified_diff,
        }
    }

    /// Parses the unified diff into a list of hunks.
    ///
    /// Lines beginning with `"--- "` or `"+++ "` (file headers) are skipped.
    /// Lines beginning with `"@@ "` open a new hunk. Each subsequent line is
    /// classified as `Added`, `Removed`, or `Context` by its first character.
    pub fn parse_hunks(&self) -> Vec<Hunk> {
        let mut completed_hunks: Vec<Hunk> = Vec::new();
        let mut current_hunk: Option<Hunk> = None;

        for raw_line in self.unified_diff.lines() {
            if raw_line.starts_with("--- ") || raw_line.starts_with("+++ ") {
                continue;
            }

            if raw_line.starts_with("@@ ") {
                if let Some(finished_hunk) = current_hunk.take() {
                    completed_hunks.push(finished_hunk);
                }
                current_hunk = Some(Hunk {
                    header: raw_line.to_owned(),
                    lines: Vec::new(),
                });
                continue;
            }

            if let Some(open_hunk) = current_hunk.as_mut() {
                let hunk_line = if let Some(added_content) = raw_line.strip_prefix('+') {
                    HunkLine::Added(added_content.to_owned())
                } else if let Some(removed_content) = raw_line.strip_prefix('-') {
                    HunkLine::Removed(removed_content.to_owned())
                } else {
                    HunkLine::Context(raw_line.get(1..).unwrap_or(raw_line).to_owned())
                };
                open_hunk.lines.push(hunk_line);
            }
        }

        if let Some(final_hunk) = current_hunk {
            completed_hunks.push(final_hunk);
        }

        completed_hunks
    }

    /// Displays each hunk with syntax highlighting and waits for a user key decision.
    ///
    /// Prints to stdout using ANSI escape codes produced by `syntect` and reads
    /// single keystrokes via `crossterm` raw mode. Returns one `HunkDecision` per
    /// hunk. If the user presses `q` or `Esc`, remaining hunks are implicitly
    /// rejected and the method returns early with decisions collected so far.
    pub fn preview_and_approve(&self) -> anyhow::Result<Vec<HunkDecision>> {
        let parsed_hunks = self.parse_hunks();
        let total_hunk_count = parsed_hunks.len();
        let mut collected_decisions: Vec<HunkDecision> = Vec::new();

        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let rust_syntax = syntax_set
            .find_syntax_by_extension("rs")
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
        let dark_theme = &theme_set.themes["base16-ocean.dark"];

        for (hunk_index, current_hunk) in parsed_hunks.iter().enumerate() {
            let hunk_number = hunk_index + 1;
            println!(
                "\n\x1b[1;36m══ {} — Hunk {hunk_number}/{total_hunk_count} ══\x1b[0m",
                self.file_path
            );
            println!("\x1b[1;33m{}\x1b[0m", current_hunk.header);

            let mut highlighter = HighlightLines::new(rust_syntax, dark_theme);

            for hunk_line in &current_hunk.lines {
                let (line_prefix, content_text) = match hunk_line {
                    HunkLine::Added(content_text) => ("\x1b[42m+ \x1b[0m", content_text.as_str()),
                    HunkLine::Removed(content_text) => ("\x1b[41m- \x1b[0m", content_text.as_str()),
                    HunkLine::Context(content_text) => ("  ", content_text.as_str()),
                };

                let highlighted_ranges = highlighter
                    .highlight_line(content_text, &syntax_set)
                    .unwrap_or_default();
                let highlighted_text = as_24_bit_terminal_escaped(&highlighted_ranges, false);

                println!("{line_prefix}{highlighted_text}\x1b[0m");
            }

            println!("\x1b[90m─────────────────────────────────────────────────\x1b[0m");
            println!("  \x1b[1m[y]\x1b[0m Accept hunk  \x1b[1m[n]\x1b[0m Reject  \x1b[1m[e]\x1b[0m Edit  \x1b[1m[q]\x1b[0m Quit");

            io::stdout().flush()?;
            enable_raw_mode()?;

            let user_accepted = loop {
                if let Event::Key(key_event) = read()? {
                    match key_event.code {
                        KeyCode::Char('y' | 'Y') => break true,
                        KeyCode::Char('n' | 'N') => break false,
                        KeyCode::Char('e' | 'E') => break true,
                        KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                            disable_raw_mode()?;
                            return Ok(collected_decisions);
                        }
                        _ => {}
                    }
                }
            };

            disable_raw_mode()?;

            collected_decisions.push(HunkDecision {
                hunk_index,
                accepted: user_accepted,
            });
        }

        Ok(collected_decisions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_DIFF: &str = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 fn foo() {}
+fn bar() {}
 fn baz() {}
 fn qux() {}
@@ -10,2 +11,3 @@
 fn alpha() {}
+fn beta() {}
";

    #[test]
    fn parse_hunks_finds_two_hunks() {
        let preview = DiffPreview::new("src/lib.rs".to_owned(), FIXTURE_DIFF.to_owned());
        let hunks = preview.parse_hunks();
        assert_eq!(hunks.len(), 2);
    }

    #[test]
    fn parse_hunks_first_hunk_has_correct_lines() {
        let preview = DiffPreview::new("src/lib.rs".to_owned(), FIXTURE_DIFF.to_owned());
        let hunks = preview.parse_hunks();
        let first_hunk = &hunks[0];
        assert!(first_hunk.header.contains("@@"));
        let added_count = first_hunk
            .lines
            .iter()
            .filter(|hunk_line| matches!(hunk_line, HunkLine::Added(_)))
            .count();
        assert_eq!(added_count, 1);
    }

    #[test]
    fn parse_hunks_skips_file_headers() {
        let preview = DiffPreview::new("src/lib.rs".to_owned(), FIXTURE_DIFF.to_owned());
        let hunks = preview.parse_hunks();
        for hunk in &hunks {
            assert!(!hunk.header.starts_with("---"));
            assert!(!hunk.header.starts_with("+++"));
        }
    }

    #[test]
    fn hunk_decision_default_accepted_false() {
        let decision = HunkDecision {
            hunk_index: 0,
            accepted: false,
        };
        assert!(!decision.accepted);
    }
}
