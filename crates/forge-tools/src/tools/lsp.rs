//! # forge-tools: LSP MCP Server
//!
//! Wraps `forge-lsp::LspBridge` as an `McpServer` with five tools:
//! definition, references, diagnostics, hover, and rename. Every tool is
//! async and delegates directly to the bridge without `spawn_blocking` —
//! the LSP bridge is already async.
//!
//! ## Input
//! - JSON-RPC parameters: `file`, `line`, `column`, `new_name`
//!
//! ## Output
//! - JSON representations of `Location`, `Diagnostic`, `HoverInfo`, `TextEdit`
//!
//! ## Related
//! - `forge-lsp::bridge` — `LspBridge` and `LspServerConfig`
//! - `forge-tools::mcp` — `McpServer` trait and `ToolCapability`

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use yantra_lsp::LspBridge;

use crate::error::McpError;
use crate::mcp::{McpServer, ToolCapability};

/// LSP MCP server that routes code-intelligence queries to language servers.
pub struct LspMcpServer {
    bridge: Arc<LspBridge>,
    server_name: String,
}

impl LspMcpServer {
    /// Creates a new `LspMcpServer` wrapping the given bridge.
    pub fn new(bridge: LspBridge) -> Self {
        Self {
            bridge: Arc::new(bridge),
            server_name: "lsp".to_owned(),
        }
    }

    /// `lsp.get_definition` — go-to-definition for a symbol position.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the language server is unavailable or the
    /// file type is unsupported.
    pub async fn get_definition(&self, params: &Value) -> Result<Value, McpError> {
        let file = required_string(params, "file")?;
        let line = required_u32(params, "line")?;
        let column = required_u32(params, "column")?;

        let location = self
            .bridge
            .get_definition(&file, line, column)
            .await
            .map_err(|lsp_error| McpError::internal(lsp_error.to_string()))?;

        serde_json::to_value(&location)
            .map_err(|serialization_error| McpError::internal(serialization_error.to_string()))
    }

    /// `lsp.find_references` — all reference locations for a symbol position.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the language server is unavailable.
    pub async fn find_references(&self, params: &Value) -> Result<Value, McpError> {
        let file = required_string(params, "file")?;
        let line = required_u32(params, "line")?;
        let column = required_u32(params, "column")?;

        let locations = self
            .bridge
            .find_references(&file, line, column)
            .await
            .map_err(|lsp_error| McpError::internal(lsp_error.to_string()))?;

        serde_json::to_value(&locations)
            .map_err(|serialization_error| McpError::internal(serialization_error.to_string()))
    }

    /// `lsp.get_diagnostics` — current diagnostics for a file.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the language server is unavailable.
    pub async fn get_diagnostics(&self, params: &Value) -> Result<Value, McpError> {
        let file = required_string(params, "file")?;

        let diagnostics = self
            .bridge
            .get_diagnostics(&file)
            .await
            .map_err(|lsp_error| McpError::internal(lsp_error.to_string()))?;

        serde_json::to_value(&diagnostics)
            .map_err(|serialization_error| McpError::internal(serialization_error.to_string()))
    }

    /// `lsp.get_hover` — hover documentation for a symbol position.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the language server is unavailable.
    pub async fn get_hover(&self, params: &Value) -> Result<Value, McpError> {
        let file = required_string(params, "file")?;
        let line = required_u32(params, "line")?;
        let column = required_u32(params, "column")?;

        let hover_info = self
            .bridge
            .get_hover(&file, line, column)
            .await
            .map_err(|lsp_error| McpError::internal(lsp_error.to_string()))?;

        serde_json::to_value(&hover_info)
            .map_err(|serialization_error| McpError::internal(serialization_error.to_string()))
    }

    /// `lsp.rename_symbol` — workspace-wide rename edits for a symbol position.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the language server is unavailable or does not
    /// support rename.
    pub async fn rename_symbol(&self, params: &Value) -> Result<Value, McpError> {
        let file = required_string(params, "file")?;
        let line = required_u32(params, "line")?;
        let column = required_u32(params, "column")?;
        let new_name = required_string(params, "new_name")?;

        let text_edits = self
            .bridge
            .rename_symbol(&file, line, column, &new_name)
            .await
            .map_err(|lsp_error| McpError::internal(lsp_error.to_string()))?;

        serde_json::to_value(&text_edits)
            .map_err(|serialization_error| McpError::internal(serialization_error.to_string()))
    }
}

#[async_trait]
impl McpServer for LspMcpServer {
    fn name(&self) -> &str {
        &self.server_name
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError> {
        match method {
            "lsp.get_definition" => self.get_definition(&params).await,
            "lsp.find_references" => self.find_references(&params).await,
            "lsp.get_diagnostics" => self.get_diagnostics(&params).await,
            "lsp.get_hover" => self.get_hover(&params).await,
            "lsp.rename_symbol" => self.rename_symbol(&params).await,
            _ => Err(McpError::method_not_found(method.to_owned())),
        }
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![]
    }
}

fn required_string(params: &Value, field_name: &str) -> Result<String, McpError> {
    params
        .get(field_name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| McpError::invalid_params(format!("missing string field `{field_name}`")))
}

fn required_u32(params: &Value, field_name: &str) -> Result<u32, McpError> {
    params
        .get(field_name)
        .and_then(Value::as_u64)
        .map(|value| value as u32)
        .ok_or_else(|| McpError::invalid_params(format!("missing u32 field `{field_name}`")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_required_string_present() {
        let params = json!({ "file": "src/lib.rs" });
        let result = required_string(&params, "file").unwrap();
        assert_eq!(result, "src/lib.rs");
    }

    #[test]
    fn test_required_string_missing_returns_error() {
        let params = json!({});
        let result = required_string(&params, "file");
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("file"));
    }

    #[test]
    fn test_required_u32_present() {
        let params = json!({ "line": 42 });
        let result = required_u32(&params, "line").unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_required_u32_missing_returns_error() {
        let params = json!({});
        let result = required_u32(&params, "line");
        assert!(result.is_err());
    }
}
