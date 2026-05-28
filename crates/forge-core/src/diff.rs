//! # forge-core: Unified Diff Utilities
//!
//! Parses unified diff text and performs semantic additive merges. Two patches
//! that each add different symbols to the same file are unioned and sorted
//! alphabetically by symbol name. Patches with removed lines are deferred.
//!
//! ## Input
//! - Unified diff strings in standard format (`---`/`+++`/`@@` headers)
//!
//! ## Output
//! - `MergeResult` — merged `FilePatch` objects plus paths that need human review
//!
//! ## Related
//! - `forge-night::merge` — re-exports this module for Night Mode usage
//! - `forge-tools::tools::merge` — exposes merge as an MCP tool

use std::collections::HashMap;

/// A single line within a unified diff hunk.
#[derive(Debug, Clone)]
pub struct HunkLine {
    /// Classification of this line.
    pub kind: HunkLineKind,
    /// Line content without the diff prefix character.
    pub content: String,
}

/// Classification of a diff hunk line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLineKind {
    /// Unmodified context line (prefix ` `).
    Context,
    /// Added line (prefix `+`).
    Added,
    /// Removed line (prefix `-`).
    Removed,
}

/// One contiguous region of changes within a file.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// First line of the pre-patch region.
    pub old_start: u32,
    /// Number of lines in the pre-patch region.
    pub old_count: u32,
    /// First line of the post-patch region.
    pub new_start: u32,
    /// Number of lines in the post-patch region.
    pub new_count: u32,
    /// Lines in this hunk.
    pub lines: Vec<HunkLine>,
}

/// One file's worth of changes in a unified diff.
#[derive(Debug, Clone)]
pub struct FilePatch {
    /// Relative path of the file being patched.
    pub path: String,
    /// Ordered list of hunks.
    pub hunks: Vec<DiffHunk>,
}

/// Result of attempting to merge two unified diffs.
#[derive(Debug)]
pub struct MergeResult {
    /// Files where both diffs added non-conflicting content, merged and sorted.
    pub merged_patches: Vec<FilePatch>,
    /// Files where a removed-line collision was detected — deferred to human review.
    pub deferred_paths: Vec<String>,
}

struct AddedBlock {
    fn_name: String,
    lines: Vec<String>,
}

/// Parses unified diff text into a list of per-file patches.
pub fn parse_unified_diff(diff_text: &str) -> Vec<FilePatch> {
    let mut patches: Vec<FilePatch> = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_hunks: Vec<DiffHunk> = Vec::new();
    let mut current_hunk: Option<DiffHunk> = None;

    for line in diff_text.lines() {
        if line.starts_with("--- ") {
            flush_patch(
                current_path.take(),
                &mut current_hunk,
                &mut current_hunks,
                &mut patches,
            );
        } else if line.starts_with("+++ ") {
            let raw_path = line.trim_start_matches("+++ b/").trim_start_matches("+++ ");
            current_path = Some(raw_path.to_owned());
        } else if line.starts_with("@@ ") {
            if let Some(finished_hunk) = current_hunk.take() {
                current_hunks.push(finished_hunk);
            }
            if let Some(parsed_hunk) = parse_hunk_header(line) {
                current_hunk = Some(parsed_hunk);
            }
        } else if let Some(hunk) = current_hunk.as_mut() {
            let kind = if line.starts_with('+') {
                HunkLineKind::Added
            } else if line.starts_with('-') {
                HunkLineKind::Removed
            } else if line.starts_with(' ') {
                HunkLineKind::Context
            } else {
                continue;
            };
            hunk.lines.push(HunkLine {
                kind,
                content: line[1..].to_owned(),
            });
        }
    }

    flush_patch(
        current_path,
        &mut current_hunk,
        &mut current_hunks,
        &mut patches,
    );
    patches
}

fn flush_patch(
    path: Option<String>,
    current_hunk: &mut Option<DiffHunk>,
    current_hunks: &mut Vec<DiffHunk>,
    patches: &mut Vec<FilePatch>,
) {
    if let Some(finished_hunk) = current_hunk.take() {
        current_hunks.push(finished_hunk);
    }
    if let Some(file_path) = path {
        let collected_hunks = std::mem::take(current_hunks);
        if !collected_hunks.is_empty() {
            patches.push(FilePatch {
                path: file_path,
                hunks: collected_hunks,
            });
        }
    }
}

fn parse_hunk_header(header_line: &str) -> Option<DiffHunk> {
    let inner = header_line.trim_start_matches("@@ ").split(" @@").next()?;
    let mut parts = inner.split_whitespace();
    let old_part = parts.next()?.trim_start_matches('-');
    let new_part = parts.next()?.trim_start_matches('+');

    let (old_start, old_count) = parse_range(old_part)?;
    let (new_start, new_count) = parse_range(new_part)?;

    Some(DiffHunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: Vec::new(),
    })
}

fn parse_range(range_str: &str) -> Option<(u32, u32)> {
    if let Some((start_str, count_str)) = range_str.split_once(',') {
        Some((start_str.parse().ok()?, count_str.parse().ok()?))
    } else {
        Some((range_str.parse().ok()?, 1))
    }
}

/// Merges two unified diffs semantically.
///
/// For each file present in both diffs: if both are purely additive (no removed
/// lines), added function blocks are unioned and sorted alphabetically by symbol
/// name. If either diff has removed lines, the file path is added to
/// `deferred_paths`. Files appearing in only one diff pass through unmodified.
pub fn merge_additive_patches(diff_a: &str, diff_b: &str) -> MergeResult {
    let patches_a = parse_unified_diff(diff_a);
    let patches_b = parse_unified_diff(diff_b);

    let mut map_a: HashMap<String, FilePatch> = patches_a
        .into_iter()
        .map(|file_patch| (file_patch.path.clone(), file_patch))
        .collect();
    let mut map_b: HashMap<String, FilePatch> = patches_b
        .into_iter()
        .map(|file_patch| (file_patch.path.clone(), file_patch))
        .collect();

    let mut all_paths: Vec<String> = map_a.keys().chain(map_b.keys()).cloned().collect();
    all_paths.sort();
    all_paths.dedup();

    let mut merged_patches: Vec<FilePatch> = Vec::new();
    let mut deferred_paths: Vec<String> = Vec::new();

    for path in all_paths {
        match (map_a.remove(&path), map_b.remove(&path)) {
            (Some(patch_only), None) | (None, Some(patch_only)) => {
                merged_patches.push(patch_only);
            }
            (Some(patch_a), Some(patch_b)) => {
                if patch_has_removals(&patch_a) || patch_has_removals(&patch_b) {
                    deferred_paths.push(path);
                } else {
                    merged_patches.push(merge_two_additive_file_patches(patch_a, &patch_b));
                }
            }
            (None, None) => {}
        }
    }

    MergeResult {
        merged_patches,
        deferred_paths,
    }
}

fn patch_has_removals(patch: &FilePatch) -> bool {
    patch.hunks.iter().any(|hunk| {
        hunk.lines
            .iter()
            .any(|line| matches!(line.kind, HunkLineKind::Removed))
    })
}

fn merge_two_additive_file_patches(patch_a: FilePatch, patch_b: &FilePatch) -> FilePatch {
    let mut blocks_a = collect_added_blocks(&patch_a);
    let mut blocks_b = collect_added_blocks(patch_b);

    let mut all_blocks: Vec<AddedBlock> = Vec::new();
    all_blocks.append(&mut blocks_a);
    all_blocks.append(&mut blocks_b);
    all_blocks.sort_by(|a_block, b_block| a_block.fn_name.cmp(&b_block.fn_name));

    build_merged_file_patch(patch_a, all_blocks)
}

fn collect_added_blocks(patch: &FilePatch) -> Vec<AddedBlock> {
    let mut blocks: Vec<AddedBlock> = Vec::new();

    for hunk in &patch.hunks {
        let mut current_block_lines: Vec<String> = Vec::new();

        for hunk_line in &hunk.lines {
            if matches!(hunk_line.kind, HunkLineKind::Added) {
                current_block_lines.push(hunk_line.content.clone());
            } else if !current_block_lines.is_empty() {
                let fn_name = extract_fn_name_from_block(&current_block_lines);
                blocks.push(AddedBlock {
                    fn_name,
                    lines: std::mem::take(&mut current_block_lines),
                });
            }
        }

        if !current_block_lines.is_empty() {
            let fn_name = extract_fn_name_from_block(&current_block_lines);
            blocks.push(AddedBlock {
                fn_name,
                lines: current_block_lines,
            });
        }
    }

    blocks
}

fn extract_fn_name_from_block(lines: &[String]) -> String {
    for line in lines {
        let trimmed = line.trim();
        for fn_prefix in &["pub async fn ", "pub fn ", "async fn ", "fn "] {
            if let Some(after_prefix) = trimmed.strip_prefix(fn_prefix) {
                let fn_name: String = after_prefix
                    .chars()
                    .take_while(|&character| character.is_alphanumeric() || character == '_')
                    .collect();
                if !fn_name.is_empty() {
                    return fn_name;
                }
            }
        }
    }
    lines
        .iter()
        .find(|line| !line.trim().is_empty())
        .cloned()
        .unwrap_or_default()
}

fn build_merged_file_patch(base_patch: FilePatch, sorted_blocks: Vec<AddedBlock>) -> FilePatch {
    if base_patch.hunks.is_empty() {
        return base_patch;
    }

    let first_hunk = &base_patch.hunks[0];
    let old_start = first_hunk.old_start;
    let old_count = first_hunk.old_count;

    let mut merged_lines: Vec<HunkLine> = first_hunk
        .lines
        .iter()
        .filter(|line| matches!(line.kind, HunkLineKind::Context))
        .map(|line| HunkLine {
            kind: HunkLineKind::Context,
            content: line.content.clone(),
        })
        .collect();

    for block in sorted_blocks {
        for line_content in block.lines {
            merged_lines.push(HunkLine {
                kind: HunkLineKind::Added,
                content: line_content,
            });
        }
    }

    let added_count = merged_lines
        .iter()
        .filter(|line| matches!(line.kind, HunkLineKind::Added))
        .count() as u32;

    FilePatch {
        path: base_patch.path,
        hunks: vec![DiffHunk {
            old_start,
            old_count,
            new_start: old_start,
            new_count: old_count + added_count,
            lines: merged_lines,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_merge_of_two_non_conflicting_additive_diffs_produces_correct_union() {
        let diff_a = "--- a/src/greeter.rs
+++ b/src/greeter.rs
@@ -5,6 +5,11 @@ impl Greeter {
     pub fn greet(&self) -> String {
         \"hello\".to_owned()
     }
+
+    pub fn zebra_method(&self) -> String {
+        \"zebra\".to_owned()
+    }
 }
";
        let diff_b = "--- a/src/greeter.rs
+++ b/src/greeter.rs
@@ -5,6 +5,11 @@ impl Greeter {
     pub fn greet(&self) -> String {
         \"hello\".to_owned()
     }
+
+    pub fn alpha_method(&self) -> String {
+        \"alpha\".to_owned()
+    }
 }
";
        let merge_result = merge_additive_patches(diff_a, diff_b);

        assert!(
            merge_result.deferred_paths.is_empty(),
            "no conflicts expected for purely additive diffs"
        );
        assert_eq!(
            merge_result.merged_patches.len(),
            1,
            "one file should be in merged output"
        );

        let file_patch = &merge_result.merged_patches[0];
        assert_eq!(file_patch.path, "src/greeter.rs");

        let added_lines: Vec<&str> = file_patch
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .filter(|line| matches!(line.kind, HunkLineKind::Added))
            .map(|line| line.content.as_str())
            .collect();

        let alpha_pos = added_lines
            .iter()
            .position(|line| line.contains("alpha_method"))
            .expect("alpha_method must appear in merged patch");
        let zebra_pos = added_lines
            .iter()
            .position(|line| line.contains("zebra_method"))
            .expect("zebra_method must appear in merged patch");

        assert!(
            alpha_pos < zebra_pos,
            "alpha_method must sort before zebra_method alphabetically"
        );
    }

    #[test]
    fn merge_defers_conflicting_diffs_with_removed_lines() {
        let diff_a = "--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,4 @@
-pub fn old_function() {}
+pub fn new_function() {}
";
        let diff_b = "--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,5 @@
+pub fn another_function() {}
 pub fn old_function() {}
";
        let merge_result = merge_additive_patches(diff_a, diff_b);

        assert!(
            merge_result
                .deferred_paths
                .contains(&"src/lib.rs".to_owned()),
            "file with removed lines must be deferred"
        );
        assert!(
            merge_result.merged_patches.is_empty(),
            "no merged patches when the only file has a conflict"
        );
    }

    #[test]
    fn parse_unified_diff_extracts_path_and_added_lines() {
        let diff = "--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!(\"added\");
 }
";
        let patches = parse_unified_diff(diff);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].path, "src/main.rs");

        let added: Vec<&str> = patches[0].hunks[0]
            .lines
            .iter()
            .filter(|line| matches!(line.kind, HunkLineKind::Added))
            .map(|line| line.content.as_str())
            .collect();
        assert_eq!(added, vec!["    println!(\"added\");"]);
    }

    #[test]
    fn single_patch_passes_through_unchanged_when_other_is_empty() {
        let diff = "--- a/src/utils.rs
+++ b/src/utils.rs
@@ -1,3 +1,5 @@
 pub fn existing() {}
+
+pub fn new_util() {}
";
        let merge_result = merge_additive_patches(diff, "");

        assert_eq!(merge_result.merged_patches.len(), 1);
        assert!(merge_result.deferred_paths.is_empty());
    }
}
