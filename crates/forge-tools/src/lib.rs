//! # forge-tools: `MCP` Tool Layer
//!
//! Implements all `MCP` servers that specialist agents are permitted to call:
//! filesystem reads/writes, shell execution, Git operations (via gitoxide),
//! `CRG` queries, `LSP` lookups, Knowledge Graph mutations, and browser control.
//!
//! ## Input
//! - Incoming `MCP` `JSON-RPC` requests from agent processes
//! - Per-agent capability declarations from `configs/agents.toml`
//!
//! ## Output
//! - `MCP` `JSON-RPC` responses returned to the requesting agent
//! - Audit records written to the decision log via `forge-obs`
//!
//! ## Related
//! - `forge-crg` — `CRG` query tool delegates here
//! - `forge-lsp` — `LSP` tool delegates here
//! - `forge-memory` — memory read/write tools delegate here
//! - `forge-agents` — consumers of every tool in this crate

pub mod error;
pub mod mcp;
pub mod tools;

pub use error::{McpError, ToolsError, ToolsResult};
pub use mcp::{
    JsonRpcErrorObject, JsonRpcRequest, JsonRpcResponse, McpRouter, McpServer, ToolCapability,
};
pub use tools::crg::CrgMcpServer;
pub use tools::diff_apply::apply_diff_to_file;
pub use tools::fs::FsMcpServer;
pub use tools::git::GitMcpServer;
pub use tools::lsp::LspMcpServer;
