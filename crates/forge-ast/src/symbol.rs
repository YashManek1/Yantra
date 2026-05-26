//! # forge-ast: Symbol Model
//!
//! Defines extracted source symbols and their language-neutral kinds. Symbols
//! are the primary nodes consumed by the Code-Review Graph.
//!
//! ## Input
//! - Tree-sitter nodes and source snippets
//!
//! ## Output
//! - `Symbol` and `SymbolKind` values
//!
//! ## Related
//! - `forge-core::id` — provides deterministic `SymbolId`
//! - `forge-ast::extractor` — produces symbols

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use yantra_core::SymbolId;

use crate::parser::LanguageKind;

/// Extracted source symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    /// Deterministic symbol identifier.
    pub id: SymbolId,
    /// Symbol name.
    pub name: String,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// Source file path.
    pub file_path: PathBuf,
    /// One-based start line.
    pub start_line: usize,
    /// One-based end line.
    pub end_line: usize,
    /// Best-effort source signature.
    pub signature: Option<String>,
    /// Best-effort docstring or leading comment.
    pub docstring: Option<String>,
    /// Source language.
    pub language: LanguageKind,
}

/// Language-neutral source symbol kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Free function.
    Function,
    /// Rust struct.
    Struct,
    /// Rust enum.
    Enum,
    /// Rust trait.
    Trait,
    /// Rust impl block.
    Impl,
    /// Constant declaration.
    Const,
    /// Static declaration.
    Static,
    /// Module declaration.
    Module,
    /// Method declaration.
    Method,
    /// Class declaration.
    Class,
}

impl SymbolKind {
    /// Returns a stable lowercase symbol kind label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Impl => "impl",
            Self::Const => "const",
            Self::Static => "static",
            Self::Module => "module",
            Self::Method => "method",
            Self::Class => "class",
        }
    }
}
