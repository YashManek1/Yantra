//! # forge-lsp: Language Server Protocol Client and `MCP` Bridge
//!
//! Manages connections to per-language `LSP` servers (`rust-analyzer`, `pyright`,
//! `typescript-language-server`) and exposes their capabilities — go-to-definition,
//! references, diagnostics, hover, rename — as typed results that `forge-tools`
//! surfaces as `MCP` tool calls.
//!
//! ## Input
//! - `LSP` server binary paths from `configs/agents.toml` or defaults on PATH
//! - Source file URIs and cursor positions for targeted queries
//!
//! ## Output
//! - Structured `LSP` responses (`Location`, `Diagnostic`, `HoverInfo`, `TextEdit`)
//!   surfaced as typed values
//! - Error diagnostics forwarded to callers via `LspError`
//!
//! ## Related
//! - `forge-ast` — `AST` spans are cross-validated against `LSP` positions
//! - `forge-verifier` — uses `LSP` to detect hallucinated symbol references
//! - `forge-tools::tools::lsp` — `MCP` adapter layer

pub mod bridge;
pub mod client;
pub mod error;
pub mod types;

pub use bridge::{LspBridge, LspServerConfig};
pub use client::LspClient;
pub use error::{LspError, LspResult};
pub use types::{Diagnostic, DiagnosticSeverity, HoverInfo, Language, Location, TextEdit};
