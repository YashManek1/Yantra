//! # forge-lsp: Shared LSP Data Types
//!
//! Plain data structures that mirror the LSP protocol types used by `lsp-types`
//! but with simpler field names. These are the output types surfaced to MCP
//! callers; they do not carry LSP-protocol internals.
//!
//! ## Input
//! - Populated from `lsp_types` response values
//!
//! ## Output
//! - `Location`, `Diagnostic`, `HoverInfo`, `TextEdit`, `Language`
//!
//! ## Related
//! - `forge-lsp::client` — deserialises LSP responses into these types
//! - `forge-tools::tools::lsp` — serialises these types as JSON for MCP

use serde::{Deserialize, Serialize};

/// Source language that selects which LSP server to spawn.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Rust — uses `rust-analyzer`.
    Rust,
    /// Python — uses `pyright`.
    Python,
    /// TypeScript — uses `typescript-language-server --stdio`.
    TypeScript,
}

impl Language {
    /// Derives the language from a file extension.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "ts" | "tsx" => Some(Self::TypeScript),
            _ => None,
        }
    }

    /// Returns the default server binary name for this language.
    pub fn default_server_binary(&self) -> &'static str {
        match self {
            Self::Rust => "rust-analyzer",
            Self::Python => "pyright-langserver",
            Self::TypeScript => "typescript-language-server",
        }
    }

    /// Returns the language identifier string used in LSP `textDocument/didOpen`.
    pub fn lsp_language_id(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::TypeScript => "typescript",
        }
    }
}

/// A resolved source location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// Absolute path to the file.
    pub file: String,
    /// 0-based start line.
    pub start_line: u32,
    /// 0-based start column.
    pub start_column: u32,
    /// 0-based end line.
    pub end_line: u32,
    /// 0-based end column.
    pub end_column: u32,
}

/// LSP severity level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// A single diagnostic produced by the language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Diagnostic severity.
    pub severity: DiagnosticSeverity,
    /// Diagnostic message text.
    pub message: String,
    /// Source location of the diagnostic.
    pub location: Location,
    /// Optional diagnostic code (e.g. `E0308`).
    pub code: Option<String>,
}

/// Hover information returned by the language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoverInfo {
    /// The main hover text, usually a type signature or documentation.
    pub contents: String,
    /// Optional source range that the hover applies to.
    pub range: Option<Location>,
}

/// A single text replacement produced by a rename operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    /// File to which this edit applies.
    pub file: String,
    /// 0-based start line.
    pub start_line: u32,
    /// 0-based start column.
    pub start_column: u32,
    /// 0-based end line.
    pub end_line: u32,
    /// 0-based end column.
    pub end_column: u32,
    /// Text that replaces the range.
    pub new_text: String,
}
