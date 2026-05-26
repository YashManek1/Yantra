//! # forge-lsp: Language Server Protocol Client and MCP Bridge
//!
//! Manages connections to per-language LSP servers (rust-analyzer, pyright,
//! tsserver) and exposes their capabilities — go-to-definition, diagnostics,
//! hover — as MCP tool calls that agents can invoke.
//!
//! ## Input
//! - LSP server binary paths from `configs/agents.toml`
//! - Source file URIs and positions for targeted queries
//!
//! ## Output
//! - Structured LSP responses (definitions, references, diagnostics)
//!   surfaced as typed `LspResult` values
//! - Diagnostic streams forwarded to the sidecar for Live Canvas alerts
//!
//! ## Related
//! - `forge-ast` — AST spans are cross-validated against LSP positions
//! - `forge-verifier` — uses LSP to detect hallucinated symbol references
//! - `forge-sidecar` — receives diagnostic push events from this crate
