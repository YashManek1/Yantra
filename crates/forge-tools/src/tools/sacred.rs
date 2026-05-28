//! # forge-tools: Sacred File Guard MCP Server
//!
//! Dedicated pre-write check that lets agents query whether a path is protected
//! before attempting a write. Defense-in-depth companion to the check already
//! embedded in `forge-tools::tools::fs::write_file`.
//!
//! ## Input
//! - `sacred.check_write { path, sacred_authorization? }` — path to validate
//! - `sacred.list_patterns {}` — list all active patterns
//!
//! ## Output
//! - `{ allowed: true }` or `McpError::forbidden` for `check_write`
//! - `{ patterns: [...] }` for `list_patterns`
//!
//! ## Related
//! - `forge-core::path` — `is_sacred` and `load_sacred_patterns` implementations
//! - `forge-tools::tools::fs` — enforces the same check at write time

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};
use yantra_core::path::{is_sacred, load_sacred_patterns, ProjectRoot};

use crate::error::McpError;
use crate::mcp::{McpServer, ToolCapability};

/// MCP server that exposes sacred-file pattern checks as explicit tool calls.
#[derive(Debug, Clone)]
pub struct SacredGuardServer {
    project_root: ProjectRoot,
    server_name: String,
}

impl SacredGuardServer {
    /// Creates a guard server for the given project root.
    pub fn new(project_root: ProjectRoot) -> Self {
        Self {
            project_root,
            server_name: "sacred".to_owned(),
        }
    }

    /// Returns `{ allowed: true }` when the path may be written, or a
    /// `McpError::forbidden` response when it is sacred and the caller has not
    /// provided `sacred_authorization: true` in the params.
    ///
    /// # Errors
    ///
    /// Returns `McpError` on invalid parameters or IO failure reading the
    /// sacred pattern file.
    pub fn check_write(&self, params: &Value) -> Result<Value, McpError> {
        let path_string = params
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::invalid_params("missing required field: path"))?;

        let path = PathBuf::from(path_string);
        let path_is_sacred = is_sacred(&self.project_root, &path)?;

        if path_is_sacred && !has_sacred_authorization(params) {
            return Err(McpError::forbidden(format!(
                "path {path_string:?} is protected by sacred-file policy; \
                 supply metadata.sacred_authorization=true after STVP approval"
            )));
        }

        Ok(json!({ "allowed": true }))
    }

    /// Returns all glob patterns currently loaded from `.yantra/sacred.txt`.
    ///
    /// # Errors
    ///
    /// Returns `McpError::internal` when the sacred pattern file cannot be read.
    pub fn list_patterns(&self, _params: &Value) -> Result<Value, McpError> {
        let yantra_dir = self.project_root.as_path().join(".yantra");
        let patterns = load_sacred_patterns(&yantra_dir)?;
        Ok(json!({ "patterns": patterns }))
    }
}

fn has_sacred_authorization(params: &Value) -> bool {
    params
        .get("sacred_authorization")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || params
            .get("metadata")
            .and_then(|metadata_value| metadata_value.get("sacred_authorization"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

#[async_trait]
impl McpServer for SacredGuardServer {
    fn name(&self) -> &str {
        &self.server_name
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError> {
        match method {
            "sacred.check_write" => self.check_write(&params),
            "sacred.list_patterns" => self.list_patterns(&params),
            _ => Err(McpError::method_not_found(method.to_owned())),
        }
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::SacredWrite]
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use yantra_core::path::ProjectRoot;

    use super::*;

    fn temp_root_with_sacred(patterns: &[&str]) -> (PathBuf, ProjectRoot) {
        let root_dir =
            std::env::temp_dir().join(format!("yantra-sacred-{}", yantra_core::TaskId::new()));
        let yantra_dir = root_dir.join(".yantra");
        std::fs::create_dir_all(&yantra_dir).unwrap();
        std::fs::write(yantra_dir.join("sacred.txt"), patterns.join("\n")).unwrap();
        let project_root = ProjectRoot::new(&root_dir).unwrap();
        (root_dir, project_root)
    }

    #[test]
    fn sacred_file_guard_blocks_unauthorized_write() {
        let (_root, project_root) = temp_root_with_sacred(&["src/auth/**"]);
        let guard = SacredGuardServer::new(project_root);

        let params = serde_json::json!({ "path": "src/auth/middleware.rs" });
        let result = guard.check_write(&params);

        assert!(result.is_err(), "sacred path must be blocked");
        let error = result.unwrap_err();
        assert_eq!(
            error.code, -32003,
            "must return a forbidden (-32003) error code"
        );
        assert!(
            error.message.contains("sacred-file policy"),
            "error message must reference sacred-file policy"
        );
    }

    #[test]
    fn sacred_file_guard_allows_authorized_write() {
        let (_root, project_root) = temp_root_with_sacred(&["src/auth/**"]);
        let guard = SacredGuardServer::new(project_root);

        let params = serde_json::json!({
            "path": "src/auth/middleware.rs",
            "metadata": { "sacred_authorization": true }
        });
        let result = guard.check_write(&params);

        assert!(result.is_ok(), "authorized write must succeed");
        assert_eq!(result.unwrap()["allowed"], true);
    }

    #[test]
    fn sacred_file_guard_allows_non_sacred_path() {
        let (_root, project_root) = temp_root_with_sacred(&["src/auth/**"]);
        let guard = SacredGuardServer::new(project_root);

        let params = serde_json::json!({ "path": "src/utils.rs" });
        let result = guard.check_write(&params);

        assert!(result.is_ok(), "non-sacred path must be allowed");
    }

    #[test]
    fn list_patterns_returns_all_configured_patterns() {
        let (_root, project_root) = temp_root_with_sacred(&["configs/**", "src/crypto/**"]);
        let guard = SacredGuardServer::new(project_root);

        let result = guard.list_patterns(&serde_json::json!({})).unwrap();
        let patterns = result["patterns"].as_array().unwrap();
        assert!(patterns
            .iter()
            .any(|pattern| pattern.as_str() == Some("configs/**")));
        assert!(patterns
            .iter()
            .any(|pattern| pattern.as_str() == Some("src/crypto/**")));
    }
}
