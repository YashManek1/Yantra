//! # forge-ast: Error Types
//!
//! Defines typed errors for parser setup, source reads, query validation, and
//! `SQLite` persistence. AST callers receive one crate-local error type across
//! all parsing and extraction operations.
//!
//! ## Input
//! - Filesystem, parser, query, and `SQLite` failures
//!
//! ## Output
//! - `AstError` and `AstResult<T>`
//!
//! ## Related
//! - `forge-ast::parser` — reports language and parse errors
//! - `forge-ast::extractor` — reports query errors

use std::path::PathBuf;

use thiserror::Error;

/// Result alias used across `forge-ast`.
pub type AstResult<T> = Result<T, AstError>;

/// Error type for AST parsing and extraction.
#[derive(Debug, Error)]
pub enum AstError {
    /// File extension is not supported by the language registry.
    #[error("unsupported language for path {path}")]
    UnsupportedLanguage {
        /// Source path that could not be mapped.
        path: PathBuf,
    },

    /// Source file could not be read.
    #[error("failed to read source file {path}: {source}")]
    ReadSource {
        /// Source path.
        path: PathBuf,
        /// Filesystem error.
        source: std::io::Error,
    },

    /// Tree-sitter parser failed to parse a source file.
    #[error("failed to parse source file {path}")]
    ParseFailed {
        /// Source path.
        path: PathBuf,
    },

    /// Tree-sitter language could not be loaded into a parser.
    #[error("failed to configure parser for {language}: {reason}")]
    ParserLanguage {
        /// Language name.
        language: &'static str,
        /// Human-readable reason.
        reason: String,
    },

    /// Tree-sitter query failed to compile.
    #[error("failed to compile query for {language}: {reason}")]
    Query {
        /// Language name.
        language: &'static str,
        /// Human-readable reason.
        reason: String,
    },

    /// `SQLite` persistence failed.
    #[error("SQLite error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Core type parsing failed.
    #[error("core error: {0}")]
    Core(#[from] yantra_core::CoreError),
}
