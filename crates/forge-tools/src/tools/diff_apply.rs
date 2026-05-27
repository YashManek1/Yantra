//! # Unified Diff Application Engine
//!
//! Parses and applies unified diffs or creation payloads to files on disk,
//! with support for fuzzy line-matching and greenfield creation.
//!
//! ## Input
//! - `project_root: &Path` — root of the project
//! - `file_path: &str` — relative path to the target file
//! - `diff_text: &str` — unified diff or raw file content
//!
//! ## Output
//! - `Result<(), String>` — success or detailed error message

use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
enum DiffLine {
    Context(String),
    Delete(String),
    Add(String),
}

struct Hunk {
    old_start: usize,
    lines: Vec<DiffLine>,
}

/// Applies a unified diff or raw content to the specified file path.
///
/// If the diff text does not contain unified diff hunk headers (e.g. `@@`),
/// it is treated as a full file write/creation payload.
pub fn apply_diff_to_file(
    project_root: &Path,
    file_path: &str,
    diff_text: &str,
) -> Result<(), String> {
    let canonical_path = project_root.join(file_path);

    if !diff_text.contains("@@") {
        if let Some(parent_directory) = canonical_path.parent() {
            std::fs::create_dir_all(parent_directory)
                .map_err(|io_error| format!("failed to create parent directories: {io_error}"))?;
        }
        std::fs::write(&canonical_path, diff_text)
            .map_err(|io_error| format!("failed to write file: {io_error}"))?;
        return Ok(());
    }

    let existing_content = if canonical_path.exists() {
        std::fs::read_to_string(&canonical_path)
            .map_err(|io_error| format!("failed to read target file: {io_error}"))?
    } else {
        String::new()
    };

    let hunks = parse_diff(diff_text);
    let modified_content = apply_hunks(&existing_content, &hunks)?;

    if let Some(parent_directory) = canonical_path.parent() {
        std::fs::create_dir_all(parent_directory)
            .map_err(|io_error| format!("failed to create parent directories: {io_error}"))?;
    }
    std::fs::write(&canonical_path, modified_content)
        .map_err(|io_error| format!("failed to write modified file: {io_error}"))?;

    Ok(())
}

fn parse_diff(diff_text: &str) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut current_hunk = None;

    for line in diff_text.lines() {
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        if line.starts_with("@@") {
            if let Some(hunk) = current_hunk.take() {
                hunks.push(hunk);
            }
            let parts: Vec<&str> = line.split("@@").collect();
            if parts.len() >= 3 {
                let range_part = parts[1].trim();
                let ranges: Vec<&str> = range_part.split_whitespace().collect();
                if ranges.len() >= 2 {
                    let old_range = ranges[0].trim_start_matches('-');
                    let (old_start, _) = parse_range(old_range);

                    current_hunk = Some(Hunk {
                        old_start,
                        lines: Vec::new(),
                    });
                }
            }
            continue;
        }

        if let Some(ref mut hunk) = current_hunk {
            if let Some(stripped) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine::Delete(stripped.to_owned()));
            } else if let Some(stripped) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine::Add(stripped.to_owned()));
            } else if let Some(stripped) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine::Context(stripped.to_owned()));
            } else {
                hunk.lines.push(DiffLine::Context(line.to_owned()));
            }
        }
    }

    if let Some(hunk) = current_hunk {
        hunks.push(hunk);
    }

    hunks
}

fn parse_range(range_str: &str) -> (usize, usize) {
    if let Some((start_str, count_str)) = range_str.split_once(',') {
        let start = start_str.parse::<usize>().unwrap_or(0);
        let count = count_str.parse::<usize>().unwrap_or(0);
        (start, count)
    } else {
        let start = range_str.parse::<usize>().unwrap_or(0);
        (start, 1)
    }
}

fn apply_hunks(file_content: &str, hunks: &[Hunk]) -> Result<String, String> {
    let mut file_lines: Vec<String> = file_content.lines().map(String::from).collect();
    let mut line_offset: isize = 0;

    for hunk in hunks {
        let expected_index = (hunk.old_start as isize - 1 + line_offset).max(0) as usize;

        let mut before_lines = Vec::new();
        for diff_line in &hunk.lines {
            match diff_line {
                DiffLine::Context(line) | DiffLine::Delete(line) => {
                    before_lines.push(line.as_str());
                }
                DiffLine::Add(_) => {}
            }
        }

        let found_index = find_matching_index(&file_lines, &before_lines, expected_index);

        let actual_index = match found_index {
            Some(idx) => idx,
            None => {
                return Err(format!(
                    "hunk failed to match context around line {}",
                    hunk.old_start
                ));
            }
        };

        let mut replacement_lines = Vec::new();
        for diff_line in &hunk.lines {
            match diff_line {
                DiffLine::Context(line) | DiffLine::Add(line) => {
                    replacement_lines.push(line.clone());
                }
                DiffLine::Delete(_) => {}
            }
        }

        let range_len = before_lines.len();
        if actual_index + range_len <= file_lines.len() {
            file_lines.splice(
                actual_index..actual_index + range_len,
                replacement_lines.clone(),
            );
        } else {
            return Err("hunk range out of bounds during application".to_owned());
        }

        let old_len = before_lines.len() as isize;
        let new_len = replacement_lines.len() as isize;
        line_offset += new_len - old_len;
    }

    Ok(file_lines.join("\n"))
}

fn find_matching_index(
    file_lines: &[String],
    before_lines: &[&str],
    start_index: usize,
) -> Option<usize> {
    if before_lines.is_empty() {
        return Some(0);
    }

    if match_at(file_lines, before_lines, start_index) {
        return Some(start_index);
    }

    let max_search_radius = file_lines.len().max(100);
    for radius in 1..=max_search_radius {
        if start_index >= radius {
            let index = start_index - radius;
            if match_at(file_lines, before_lines, index) {
                return Some(index);
            }
        }
        let index = start_index + radius;
        if index <= file_lines.len() && match_at(file_lines, before_lines, index) {
            return Some(index);
        }
    }

    None
}

fn match_at(file_lines: &[String], before_lines: &[&str], index: usize) -> bool {
    if index + before_lines.len() > file_lines.len() {
        return false;
    }
    for (i, before_line) in before_lines.iter().enumerate() {
        if file_lines[index + i].trim() != before_line.trim() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_diff_hunks() {
        let diff_text = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn add(a: i32, b: i32) -> i32 {
-    a + b
+    let result = a + b;
+    result
 }";
        let hunks = parse_diff(diff_text);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
    }

    #[test]
    fn test_apply_hunks_exact() {
        let content = "\
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}";
        let diff_text = "\
@@ -1,3 +1,4 @@
 pub fn add(a: i32, b: i32) -> i32 {
-    a + b
+    let result = a + b;
+    result
 }";
        let hunks = parse_diff(diff_text);
        let result = apply_hunks(content, &hunks).unwrap();
        let expected = "\
pub fn add(a: i32, b: i32) -> i32 {
    let result = a + b;
    result
}";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_apply_hunks_fuzzy() {
        let content = "\
// Some leading comments
// More comments

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}";
        let diff_text = "\
@@ -1,3 +1,4 @@
 pub fn add(a: i32, b: i32) -> i32 {
-    a + b
+    let result = a + b;
+    result
 }";
        let hunks = parse_diff(diff_text);
        let result = apply_hunks(content, &hunks).unwrap();
        let expected = "\
// Some leading comments
// More comments

pub fn add(a: i32, b: i32) -> i32 {
    let result = a + b;
    result
}";
        assert_eq!(result, expected);
    }
}
