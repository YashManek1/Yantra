//! # forge-ast: Tree-sitter Parser Registry
//!
//! Maps source file extensions to Tree-sitter grammars and parses source
//! files into `ParsedFile` values. Hackathon scope includes Rust, Python,
//! TypeScript, and JavaScript.
//!
//! ## Input
//! - Source file paths and source bytes
//!
//! ## Output
//! - `ParsedFile` containing the source text, language, and parsed tree
//!
//! ## Related
//! - `forge-ast::extractor` — consumes parsed files
//! - `forge-ast::symbol` — receives language metadata

use std::path::{Path, PathBuf};

use tree_sitter::{Language, Parser, Tree};

use crate::error::{AstError, AstResult};

/// Language supported by the AST layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LanguageKind {
    /// Rust source.
    Rust,
    /// Python source.
    Python,
    /// TypeScript source.
    TypeScript,
    /// JavaScript source parsed by the TypeScript grammar.
    JavaScript,
}

impl LanguageKind {
    /// Returns the stable language name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
        }
    }

    /// Returns the Tree-sitter `Language` object for this language variant.
    pub(crate) fn tree_sitter_language(self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::TypeScript | Self::JavaScript => {
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
            }
        }
    }
}

/// Registry mapping file extensions to Tree-sitter parsers.
#[derive(Debug, Clone, Copy, Default)]
pub struct LanguageRegistry;

impl LanguageRegistry {
    /// Detects a supported language from a path extension.
    pub fn language_for_path(path: &Path) -> Option<LanguageKind> {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("rs") => Some(LanguageKind::Rust),
            Some("py") => Some(LanguageKind::Python),
            Some("ts" | "tsx") => Some(LanguageKind::TypeScript),
            Some("js" | "jsx" | "mjs" | "cjs") => Some(LanguageKind::JavaScript),
            _ => None,
        }
    }

    /// Builds a parser for a language.
    ///
    /// # Errors
    ///
    /// Returns `AstError::ParserLanguage` when Tree-sitter rejects the grammar.
    pub fn parser_for_language(language: LanguageKind) -> AstResult<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&language.tree_sitter_language())
            .map_err(|error| AstError::ParserLanguage {
                language: language.as_str(),
                reason: error.to_string(),
            })?;
        Ok(parser)
    }
}

/// Parsed source file plus its source text.
#[derive(Debug)]
pub struct ParsedFile {
    /// Source file path.
    pub path: PathBuf,
    /// Source language.
    pub language: LanguageKind,
    /// Source text.
    pub source: String,
    /// Parsed Tree-sitter tree.
    pub tree: Tree,
}

/// Parses a source file using the language registry.
///
/// # Blocking
///
/// This function performs synchronous I/O. Call it from async code via
/// `tokio::task::spawn_blocking` to avoid blocking the runtime thread.
///
/// # Errors
///
/// Returns `AstError` when the language is unsupported, source cannot be read,
/// parser setup fails, or parsing returns no tree.
pub fn parse_file(path: &Path) -> AstResult<ParsedFile> {
    let language =
        LanguageRegistry::language_for_path(path).ok_or_else(|| AstError::UnsupportedLanguage {
            path: path.to_path_buf(),
        })?;
    let source = std::fs::read_to_string(path).map_err(|source_error| AstError::ReadSource {
        path: path.to_path_buf(),
        source: source_error,
    })?;
    let mut parser = LanguageRegistry::parser_for_language(language)?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| AstError::ParseFailed {
            path: path.to_path_buf(),
        })?;

    Ok(ParsedFile {
        path: path.to_path_buf(),
        language,
        source,
        tree,
    })
}
