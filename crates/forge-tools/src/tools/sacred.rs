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

        if path_is_sacred {
            verify_sacred_authorization(&self.project_root, params)?;
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

fn verify_sacred_authorization(project_root: &ProjectRoot, params: &Value) -> Result<(), McpError> {
    let token_val = params
        .get("truth_token")
        .or_else(|| params.get("metadata").and_then(|m| m.get("truth_token")))
        .or_else(|| params.get("_meta").and_then(|m| m.get("truth_token")))
        .ok_or_else(|| McpError::forbidden("sacred action requires a truth token"))?;

    let token: yantra_core::truth::TruthToken = serde_json::from_value(token_val.clone())
        .map_err(|err| McpError::invalid_params(format!("invalid truth token format: {err}")))?;

    if !token.sacred_authorized {
        return Err(McpError::forbidden(
            "truth token does not authorize sacred modifications",
        ));
    }

    let yantra_dir = project_root.as_path().join(".yantra");
    let pub_file_path = yantra_dir.join("session.pub");
    if !pub_file_path.exists() {
        return Err(McpError::forbidden(
            "session public key not found; cannot verify token",
        ));
    }
    let public_key_bytes = std::fs::read(&pub_file_path)
        .map_err(|source| McpError::internal(format!("failed to read session.pub: {source}")))?;
    let verifying_key = yantra_core::truth::VerifyingKey::new(public_key_bytes);
    if !token.verify(&verifying_key) {
        return Err(McpError::forbidden("truth token signature is invalid"));
    }

    Ok(())
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

    fn generate_and_save_key(
        yantra_dir: &std::path::Path,
    ) -> (
        ring::signature::Ed25519KeyPair,
        yantra_core::truth::VerifyingKey,
    ) {
        use ring::rand::SystemRandom;
        use ring::signature::Ed25519KeyPair;
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let verifying_key = yantra_core::truth::VerifyingKey::from_key_pair(&key_pair);
        std::fs::write(yantra_dir.join("session.pub"), verifying_key.as_bytes()).unwrap();
        (key_pair, verifying_key)
    }

    #[test]
    fn sacred_file_guard_blocks_unauthorized_write() {
        let (root, project_root) = temp_root_with_sacred(&["src/auth/**"]);
        let _key_pair = generate_and_save_key(&root.join(".yantra"));
        let guard = SacredGuardServer::new(project_root);

        let params = serde_json::json!({ "path": "src/auth/middleware.rs" });
        let result = guard.check_write(&params);

        assert!(result.is_err(), "sacred path must be blocked");
        let error = result.unwrap_err();
        assert_eq!(
            error.code, -32003,
            "must return a forbidden (-32003) error code"
        );
    }

    #[test]
    fn sacred_file_guard_allows_authorized_write() {
        let (root, project_root) = temp_root_with_sacred(&["src/auth/**"]);
        let (key_pair, _verifying_key) = generate_and_save_key(&root.join(".yantra"));
        let guard = SacredGuardServer::new(project_root);

        let token = yantra_core::truth::TruthToken::new(
            yantra_core::TaskId::new(),
            yantra_core::TaskClass::BugFix,
            yantra_core::Strictness::Light,
            true,
            [0u8; 32],
            &key_pair,
        )
        .unwrap();

        let params = serde_json::json!({
            "path": "src/auth/middleware.rs",
            "metadata": { "truth_token": token }
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
