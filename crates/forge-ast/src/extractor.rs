//! # forge-ast: Symbol, Call, and Import Extraction
//!
//! Extracts symbols, call sites, and imports from parsed Tree-sitter files.
//! Query files are compiled per language to validate extraction coverage, then
//! the parsed tree is walked into language-neutral records.
//!
//! ## Input
//! - `ParsedFile` values with source text and Tree-sitter trees
//! - Language query files under `queries/`
//!
//! ## Output
//! - `Symbol`, `CallSite`, and `Import` records
//!
//! ## Related
//! - `forge-ast::parser` — produces parsed files
//! - `forge-ast::symbol` — defines extracted symbol records

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tree_sitter::Node;
use yantra_core::SymbolId;

use crate::error::{AstError, AstResult};
use crate::parser::{LanguageKind, ParsedFile};
use crate::symbol::{Symbol, SymbolKind};

const RUST_QUERY: &str = include_str!("../queries/rust.scm");
const PYTHON_QUERY: &str = include_str!("../queries/python.scm");
const TYPESCRIPT_QUERY: &str = include_str!("../queries/typescript.scm");

/// Function call occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSite {
    /// File containing the call.
    pub caller_file: PathBuf,
    /// One-based line containing the call.
    pub caller_line: usize,
    /// Best-effort callee name.
    pub callee_name: String,
}

/// Import declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// File containing the import.
    pub file: PathBuf,
    /// Imported module path.
    pub module_path: String,
    /// Explicit imported names when available.
    pub imported_names: Vec<String>,
}

use std::sync::OnceLock;
use tree_sitter::Query;

static RUST_QUERY_CACHE: OnceLock<Result<Query, String>> = OnceLock::new();
static PYTHON_QUERY_CACHE: OnceLock<Result<Query, String>> = OnceLock::new();
static TYPESCRIPT_QUERY_CACHE: OnceLock<Result<Query, String>> = OnceLock::new();

/// Walks a file's AST in a single pass to extract symbols, call sites, and imports.
///
/// # Errors
///
/// Returns `AstError::Query` when the language query fails to compile.
pub fn extract_all(parsed: &ParsedFile) -> AstResult<(Vec<Symbol>, Vec<CallSite>, Vec<Import>)> {
    validate_query(parsed)?;
    let mut symbols = Vec::new();
    let mut call_sites = Vec::new();
    let mut imports = Vec::new();

    collect_all(
        parsed,
        parsed.tree.root_node(),
        &mut symbols,
        &mut call_sites,
        &mut imports,
    );

    Ok((symbols, call_sites, imports))
}

/// Extracts symbols from a parsed file.
///
/// # Errors
///
/// Returns `AstError::Query` when the language query fails to compile.
pub fn extract_symbols(parsed: &ParsedFile) -> AstResult<Vec<Symbol>> {
    let (symbols, _, _) = extract_all(parsed)?;
    Ok(symbols)
}

/// Extracts function call sites from a parsed file.
///
/// # Errors
///
/// Returns `AstError::Query` when the language query fails to compile.
pub fn extract_calls(parsed: &ParsedFile) -> AstResult<Vec<CallSite>> {
    let (_, call_sites, _) = extract_all(parsed)?;
    Ok(call_sites)
}

/// Extracts import declarations from a parsed file.
///
/// # Errors
///
/// Returns `AstError::Query` when the language query fails to compile.
pub fn extract_imports(parsed: &ParsedFile) -> AstResult<Vec<Import>> {
    let (_, _, imports) = extract_all(parsed)?;
    Ok(imports)
}

fn validate_query(parsed: &ParsedFile) -> AstResult<()> {
    let cache = match parsed.language {
        LanguageKind::Rust => &RUST_QUERY_CACHE,
        LanguageKind::Python => &PYTHON_QUERY_CACHE,
        LanguageKind::TypeScript | LanguageKind::JavaScript => &TYPESCRIPT_QUERY_CACHE,
    };

    let query_result = cache.get_or_init(|| {
        let query_source = match parsed.language {
            LanguageKind::Rust => RUST_QUERY,
            LanguageKind::Python => PYTHON_QUERY,
            LanguageKind::TypeScript | LanguageKind::JavaScript => TYPESCRIPT_QUERY,
        };
        Query::new(&parsed.language.tree_sitter_language(), query_source)
            .map_err(|error| error.to_string())
    });

    match query_result {
        Ok(_) => Ok(()),
        Err(reason) => Err(AstError::Query {
            language: parsed.language.as_str(),
            reason: reason.clone(),
        }),
    }
}

fn collect_all(
    parsed: &ParsedFile,
    node: Node<'_>,
    symbols: &mut Vec<Symbol>,
    call_sites: &mut Vec<CallSite>,
    imports: &mut Vec<Import>,
) {
    if let Some((name_node, symbol_kind)) = symbol_name_and_kind(parsed, node) {
        if let Ok(name) = name_node.utf8_text(parsed.source.as_bytes()) {
            symbols.push(build_symbol(parsed, node, name, symbol_kind));
        }
    }

    if matches!(node.kind(), "call_expression" | "call") {
        let callee_node = node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("callee"))
            .or_else(|| first_named_child(node));
        if let Some(callee_node) = callee_node {
            if let Some(callee_name) = callee_name(parsed, callee_node) {
                call_sites.push(CallSite {
                    caller_file: parsed.path.clone(),
                    caller_line: node.start_position().row + 1,
                    callee_name,
                });
            }
        }
    }

    let is_import = match parsed.language {
        LanguageKind::Rust => node.kind() == "use_declaration",
        LanguageKind::Python => matches!(node.kind(), "import_statement" | "import_from_statement"),
        LanguageKind::TypeScript | LanguageKind::JavaScript => {
            node.kind() == "import_statement"
                || node.kind() == "call_expression"
                    && node_text(parsed, node).starts_with("require")
        }
    };

    if is_import {
        imports.push(import_from_node(parsed, node));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_all(parsed, child, symbols, call_sites, imports);
    }
}

fn symbol_name_and_kind<'tree>(
    parsed: &ParsedFile,
    node: Node<'tree>,
) -> Option<(Node<'tree>, SymbolKind)> {
    match parsed.language {
        LanguageKind::Rust => rust_symbol_name_and_kind(node),
        LanguageKind::Python => python_symbol_name_and_kind(node),
        LanguageKind::TypeScript | LanguageKind::JavaScript => {
            typescript_symbol_name_and_kind(node)
        }
    }
}

fn rust_symbol_name_and_kind(node: Node<'_>) -> Option<(Node<'_>, SymbolKind)> {
    let symbol_kind = match node.kind() {
        "function_item" if inside_kind(node, "impl_item") => return None,
        "function_item" => SymbolKind::Function,
        "struct_item" => SymbolKind::Struct,
        "enum_item" => SymbolKind::Enum,
        "trait_item" => SymbolKind::Trait,
        "impl_item" => SymbolKind::Impl,
        "const_item" => SymbolKind::Const,
        "static_item" => SymbolKind::Static,
        "mod_item" => SymbolKind::Module,
        _ => return None,
    };
    if symbol_kind == SymbolKind::Impl {
        let name_node = node
            .children(&mut node.walk())
            .find(|child| matches!(child.kind(), "type_identifier" | "scoped_type_identifier"))?;
        Some((name_node, symbol_kind))
    } else {
        node.child_by_field_name("name")
            .map(|name_node| (name_node, symbol_kind))
    }
}

fn python_symbol_name_and_kind(node: Node<'_>) -> Option<(Node<'_>, SymbolKind)> {
    let symbol_kind = match node.kind() {
        "function_definition" if inside_kind(node, "class_definition") => SymbolKind::Method,
        "function_definition" => SymbolKind::Function,
        "class_definition" => SymbolKind::Class,
        _ => return None,
    };
    node.child_by_field_name("name")
        .map(|name_node| (name_node, symbol_kind))
}

fn typescript_symbol_name_and_kind(node: Node<'_>) -> Option<(Node<'_>, SymbolKind)> {
    let symbol_kind = match node.kind() {
        "function_declaration" | "generator_function_declaration" => SymbolKind::Function,
        "method_definition" => SymbolKind::Method,
        "class_declaration" => SymbolKind::Class,
        "interface_declaration" | "type_alias_declaration" => SymbolKind::Trait,
        "lexical_declaration" => SymbolKind::Const,
        _ => return None,
    };
    node.child_by_field_name("name")
        .map(|name_node| (name_node, symbol_kind))
        .or_else(|| first_identifier_child(node).map(|name_node| (name_node, symbol_kind)))
}

fn build_symbol(parsed: &ParsedFile, node: Node<'_>, name: &str, kind: SymbolKind) -> Symbol {
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;
    let file_path = parsed.path.clone();
    let language = parsed.language;
    let signature = signature_for_node(parsed, node);
    let docstring = docstring_before(parsed, start_line);
    let symbol_id = SymbolId::from_parts(&file_path.to_string_lossy(), name, kind.as_str());

    Symbol {
        id: symbol_id,
        name: name.to_string(),
        kind,
        file_path,
        start_line,
        end_line,
        signature,
        docstring,
        language,
    }
}

fn import_from_node(parsed: &ParsedFile, node: Node<'_>) -> Import {
    let text = node_text(parsed, node);
    Import {
        file: parsed.path.clone(),
        module_path: module_path_from_import(parsed.language, &text),
        imported_names: imported_names_from_import(parsed.language, &text),
    }
}

fn module_path_from_import(language: LanguageKind, text: &str) -> String {
    match language {
        LanguageKind::Rust => text
            .trim()
            .trim_start_matches("use")
            .trim()
            .trim_end_matches(';')
            .to_string(),
        LanguageKind::Python => text
            .trim()
            .strip_prefix("from ")
            .and_then(|rest| {
                rest.split_once(" import ")
                    .map(|(module_path, _)| module_path)
            })
            .or_else(|| text.trim().strip_prefix("import "))
            .unwrap_or(text.trim())
            .to_string(),
        LanguageKind::TypeScript | LanguageKind::JavaScript => text
            .split(['"', '\''])
            .nth(1)
            .unwrap_or(text.trim())
            .to_string(),
    }
}

fn imported_names_from_import(language: LanguageKind, text: &str) -> Vec<String> {
    match language {
        LanguageKind::Rust => text
            .trim()
            .trim_start_matches("use")
            .trim()
            .trim_end_matches(';')
            .split("::")
            .last()
            .map(split_import_names)
            .unwrap_or_default(),
        LanguageKind::Python => text.trim().split_once(" import ").map_or_else(
            || {
                text.trim()
                    .trim_start_matches("import")
                    .split(',')
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .collect()
            },
            |(_, names)| split_import_names(names),
        ),
        LanguageKind::TypeScript | LanguageKind::JavaScript => text
            .split_once(" from ")
            .map(|(names, _)| split_import_names(names.trim_start_matches("import").trim()))
            .unwrap_or_default(),
    }
}

fn split_import_names(text: &str) -> Vec<String> {
    text.trim()
        .trim_matches('{')
        .trim_matches('}')
        .split(',')
        .map(str::trim)
        .map(|name| name.split_whitespace().next().unwrap_or_default())
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn callee_name(parsed: &ParsedFile, node: Node<'_>) -> Option<String> {
    if matches!(node.kind(), "identifier" | "field_identifier") {
        return Some(node_text(parsed, node));
    }
    let text = node_text(parsed, node);
    text.split(['.', ':'])
        .filter(|part| !part.is_empty())
        .next_back()
        .map(|part| {
            part.trim()
                .trim_end_matches(')')
                .trim_end_matches('(')
                .to_string()
        })
}

fn signature_for_node(parsed: &ParsedFile, node: Node<'_>) -> Option<String> {
    node_text(parsed, node)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
}

fn docstring_before(parsed: &ParsedFile, start_line: usize) -> Option<String> {
    let lines = parsed.source.lines().collect::<Vec<_>>();
    if start_line <= 1 || start_line > lines.len() + 1 {
        return None;
    }

    let mut collected_lines = Vec::new();
    for line in lines[..start_line - 1].iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if collected_lines.is_empty() {
                continue;
            }
            break;
        }
        if is_doc_line(trimmed) {
            collected_lines.push(clean_doc_line(trimmed));
        } else {
            break;
        }
    }
    collected_lines.reverse();
    (!collected_lines.is_empty()).then(|| collected_lines.join("\n"))
}

fn is_doc_line(line: &str) -> bool {
    line.starts_with("///")
        || line.starts_with("//!")
        || line.starts_with("//")
        || line.starts_with('#')
        || line.starts_with('*')
}

fn clean_doc_line(line: &str) -> String {
    line.trim_start_matches('/')
        .trim_start_matches('!')
        .trim_start_matches('#')
        .trim_start_matches('*')
        .trim()
        .to_string()
}

fn node_text(parsed: &ParsedFile, node: Node<'_>) -> String {
    node.utf8_text(parsed.source.as_bytes())
        .unwrap_or_else(|utf8_error| {
            tracing::warn!(
                "UTF-8 decode failed for node at {}:{}: {}",
                node.start_position().row,
                node.start_position().column,
                utf8_error
            );
            ""
        })
        .to_string()
}

fn first_identifier_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    let identifier_node = node
        .children(&mut cursor)
        .find(|child| matches!(child.kind(), "identifier" | "type_identifier"));
    identifier_node
}

fn first_named_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    let named_node = node.children(&mut cursor).find(Node::is_named);
    named_node
}

fn inside_kind(mut node: Node<'_>, kind: &str) -> bool {
    while let Some(parent) = node.parent() {
        if parent.kind() == kind {
            return true;
        }
        node = parent;
    }
    false
}
