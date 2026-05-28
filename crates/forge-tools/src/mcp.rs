//! # forge-tools: MCP JSON-RPC Router
//!
//! Provides a small MCP server framework over JSON-RPC 2.0 stdio transport.
//! Inputs are method names, JSON parameters, and per-agent permitted tool sets;
//! outputs are routed JSON values or JSON-RPC error responses.
//!
//! ## Input
//! - JSON-RPC 2.0 requests from stdin or in-process tests
//! - Registered `McpServer` implementations
//! - Agent permitted tool names
//!
//! ## Output
//! - JSON-RPC 2.0 response objects
//! - Tool capability metadata for policy checks
//!
//! ## Related
//! - `forge-tools::tools::fs` — filesystem MCP server
//! - `forge-tools::tools::git` — gitoxide MCP server

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::McpError;

/// Capability label exposed by an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCapability {
    /// Read project files.
    ReadFiles,
    /// Write project files.
    WriteFiles,
    /// Delete project files.
    DeleteFiles,
    /// Read Git repository state.
    GitRead,
    /// Write Git repository state.
    GitWrite,
    /// Write files protected by sacred-file policy.
    SacredWrite,
    /// Execute commands inside an isolated sandbox container.
    SandboxedExec,
}

/// Trait implemented by every MCP server in Yantra.
#[async_trait]
pub trait McpServer: Send + Sync {
    /// Returns the stable server name.
    fn name(&self) -> &str;

    /// Handles a single method call with JSON parameters.
    async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError>;

    /// Returns the capabilities this server can exercise.
    fn capabilities(&self) -> Vec<ToolCapability>;
}

/// JSON-RPC request object accepted by the stdio transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// JSON-RPC protocol version.
    pub jsonrpc: String,
    /// Request identifier echoed in the response.
    pub id: Value,
    /// Method name such as `fs.read_file`.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC response object produced by the router.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// JSON-RPC protocol version.
    pub jsonrpc: String,
    /// Request identifier echoed in the response.
    pub id: Value,
    /// Successful result value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error result value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorObject>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}

/// Routes MCP methods to registered servers and enforces permitted tool names.
#[derive(Clone, Default)]
pub struct McpRouter {
    servers_by_prefix: HashMap<String, Arc<dyn McpServer>>,
    permitted_methods: HashSet<String>,
}

impl McpRouter {
    /// Creates an empty router with the provided permitted method names.
    pub fn new(permitted_methods: impl IntoIterator<Item = String>) -> Self {
        Self {
            servers_by_prefix: HashMap::new(),
            permitted_methods: permitted_methods.into_iter().collect(),
        }
    }

    /// Registers a server under its method prefix.
    pub fn register_server(&mut self, server: Arc<dyn McpServer>) {
        let server_prefix = server.name().to_owned();
        self.servers_by_prefix.insert(server_prefix, server);
    }

    /// Handles an in-process method call.
    ///
    /// # Errors
    ///
    /// Returns an MCP error when the method is not permitted, no registered
    /// server owns the method prefix, or the target server rejects the call.
    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError> {
        if !self.permitted_methods.contains(method) {
            return Err(McpError::forbidden(format!(
                "method not permitted: {method}"
            )));
        }

        let server_prefix = method
            .split_once('.')
            .map_or(method, |(prefix, _operation)| prefix);
        let server = self
            .servers_by_prefix
            .get(server_prefix)
            .ok_or_else(|| McpError::method_not_found(method.to_owned()))?;

        server.handle(method, params).await
    }

    /// Handles a JSON-RPC request string and returns a JSON-RPC response string.
    ///
    /// # Errors
    ///
    /// Returns a serialization failure if the response cannot be encoded.
    pub async fn handle_json_rpc_line(
        &self,
        request_line: &str,
    ) -> Result<String, serde_json::Error> {
        let request = serde_json::from_str::<JsonRpcRequest>(request_line);
        let response = match request {
            Ok(parsed_request) if parsed_request.jsonrpc == "2.0" => {
                self.response_for_request(parsed_request).await
            }
            Ok(parsed_request) => JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id: parsed_request.id,
                result: None,
                error: Some(JsonRpcErrorObject {
                    code: -32600,
                    message: "invalid jsonrpc version".to_owned(),
                }),
            },
            Err(parse_error) => JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id: Value::Null,
                result: None,
                error: Some(JsonRpcErrorObject {
                    code: -32700,
                    message: parse_error.to_string(),
                }),
            },
        };

        serde_json::to_string(&response)
    }

    /// Runs a newline-delimited JSON-RPC stdio loop until stdin closes.
    ///
    /// # Errors
    ///
    /// Returns an IO error if stdin or stdout cannot be read or written.
    pub async fn run_stdio(self) -> std::io::Result<()> {
        let stdin_reader = BufReader::new(tokio::io::stdin());
        let mut stdin_lines = stdin_reader.lines();
        let mut stdout_writer = tokio::io::stdout();

        while let Some(request_line) = stdin_lines.next_line().await? {
            let response_line = self
                .handle_json_rpc_line(&request_line)
                .await
                .unwrap_or_else(|serialization_error| {
                    json!({
                        "jsonrpc": "2.0",
                        "id": null,
                        "error": {
                            "code": -32603,
                            "message": serialization_error.to_string()
                        }
                    })
                    .to_string()
                });
            stdout_writer.write_all(response_line.as_bytes()).await?;
            stdout_writer.write_all(b"\n").await?;
            stdout_writer.flush().await?;
        }

        Ok(())
    }

    async fn response_for_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        match self.handle(&request.method, request.params).await {
            Ok(result_value) => JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id: request.id,
                result: Some(result_value),
                error: None,
            },
            Err(error) => JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id: request.id,
                result: None,
                error: Some(JsonRpcErrorObject {
                    code: error.code,
                    message: error.message,
                }),
            },
        }
    }
}
