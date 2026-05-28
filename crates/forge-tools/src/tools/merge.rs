//! # forge-tools: Semantic Merge MCP Server
//!
//! Exposes the `forge-core::diff` semantic merge algorithm as an MCP tool so
//! agents can request an additive union merge of two unified diffs without
//! shelling out or implementing their own parser.
//!
//! ## Input
//! - `merge.semantic_merge { diff_a: "...", diff_b: "..." }` — two unified diffs
//!
//! ## Output
//! - `{ merged_files: [...], deferred_paths: [...] }` — merged per-file patches
//!   and paths that must be manually resolved
//!
//! ## Related
//! - `forge-core::diff` — underlying merge algorithm and `MergeResult` type
//! - `forge-night::merge` — re-exports the same algorithm for Night Mode usage

use async_trait::async_trait;
use serde_json::{json, Value};
use yantra_core::diff::merge_additive_patches;

use crate::error::McpError;
use crate::mcp::{McpServer, ToolCapability};

/// MCP server that wraps the semantic additive-diff merge algorithm.
#[derive(Debug, Clone, Default)]
pub struct MergeMcpServer {
    server_name: String,
}

impl MergeMcpServer {
    /// Creates a new merge MCP server.
    pub fn new() -> Self {
        Self {
            server_name: "merge".to_owned(),
        }
    }

    /// Merges two unified diffs and returns the result as JSON.
    ///
    /// For files present in both `diff_a` and `diff_b`: if both are purely
    /// additive, their added function blocks are unioned and sorted
    /// alphabetically. Files with removed-line conflicts are added to
    /// `deferred_paths`.
    ///
    /// # Errors
    ///
    /// Returns `McpError::invalid_params` when either `diff_a` or `diff_b` is
    /// missing from the parameters.
    pub fn semantic_merge(&self, params: &Value) -> Result<Value, McpError> {
        let diff_a = params
            .get("diff_a")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::invalid_params("missing required field: diff_a"))?;
        let diff_b = params
            .get("diff_b")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::invalid_params("missing required field: diff_b"))?;

        let merge_result = merge_additive_patches(diff_a, diff_b);

        let merged_files: Vec<Value> = merge_result
            .merged_patches
            .iter()
            .map(|file_patch| {
                let added_lines: Vec<&str> = file_patch
                    .hunks
                    .iter()
                    .flat_map(|hunk| hunk.lines.iter())
                    .filter(|line| matches!(line.kind, yantra_core::diff::HunkLineKind::Added))
                    .map(|line| line.content.as_str())
                    .collect();

                json!({
                    "path": file_patch.path,
                    "hunk_count": file_patch.hunks.len(),
                    "added_line_count": added_lines.len(),
                })
            })
            .collect();

        Ok(json!({
            "merged_files": merged_files,
            "deferred_paths": merge_result.deferred_paths,
        }))
    }
}

#[async_trait]
impl McpServer for MergeMcpServer {
    fn name(&self) -> &str {
        &self.server_name
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError> {
        match method {
            "merge.semantic_merge" => self.semantic_merge(&params),
            _ => Err(McpError::method_not_found(method.to_owned())),
        }
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn semantic_merge_tool_returns_merged_file_list() {
        let server = MergeMcpServer::new();
        let params = serde_json::json!({
            "diff_a": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,5 @@\n pub fn a() {}\n+\n+pub fn c() {}\n",
            "diff_b": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,5 @@\n pub fn a() {}\n+\n+pub fn b() {}\n",
        });

        let result = server.handle("merge.semantic_merge", params).await.unwrap();

        let merged_files = result["merged_files"].as_array().unwrap();
        assert_eq!(merged_files.len(), 1);
        assert_eq!(merged_files[0]["path"], "src/lib.rs");

        let deferred = result["deferred_paths"].as_array().unwrap();
        assert!(deferred.is_empty(), "no conflicts expected");
    }

    #[tokio::test]
    async fn semantic_merge_tool_reports_deferred_path_for_conflict() {
        let server = MergeMcpServer::new();
        let params = serde_json::json!({
            "diff_a": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n-pub fn old() {}\n+pub fn new() {}\n",
            "diff_b": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,3 @@\n+pub fn extra() {}\n pub fn old() {}\n",
        });

        let result = server.handle("merge.semantic_merge", params).await.unwrap();

        let deferred = result["deferred_paths"].as_array().unwrap();
        assert!(
            deferred
                .iter()
                .any(|path| path.as_str() == Some("src/lib.rs")),
            "conflicting file must appear in deferred_paths"
        );
    }

    #[tokio::test]
    async fn semantic_merge_tool_returns_error_for_missing_diff_a() {
        let server = MergeMcpServer::new();
        let params = serde_json::json!({ "diff_b": "--- a/x\n+++ b/x\n" });
        let result = server.handle("merge.semantic_merge", params).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, -32602);
    }
}
