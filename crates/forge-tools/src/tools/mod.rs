//! # forge-tools: MCP Server Modules
//!
//! Collects the concrete tool servers that agents invoke through the MCP
//! router. Inputs are server-specific JSON parameters; outputs are JSON values
//! returned through `forge-tools::mcp`.
//!
//! ## Input
//! - JSON parameters from `McpRouter`
//! - Project-root scoped runtime state
//!
//! ## Output
//! - Filesystem, Git, CRG, and LSP MCP responses
//!
//! ## Related
//! - `forge-tools::mcp` — shared routing and transport layer

pub mod crg;
pub mod fs;
pub mod git;
pub mod lsp;
