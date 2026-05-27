//! # forge-ast: Symbol Storage Schema
//!
//! Defines the `SQLite` schema for AST-derived files, symbols, call sites, and
//! imports. `forge-ast` owns the schema and helpers; `forge-crg` owns the
//! database lifecycle.
//!
//! ## Input
//! - Extracted `Symbol` records
//! - `SQLite` connections owned by CRG callers
//!
//! ## Output
//! - Schema setup and symbol round-trip helpers
//!
//! ## Related
//! - `forge-ast::symbol` — defines stored symbol records
//! - `forge-crg` — owns `.yantra/crg.sqlite`

use std::path::PathBuf;
use std::str::FromStr;

use chrono::Utc;
use rusqlite::{params, Connection};
use yantra_core::SymbolId;

use crate::error::{AstError, AstResult};
use crate::parser::LanguageKind;
use crate::symbol::{Symbol, SymbolKind};

/// `SQLite` schema for AST-derived CRG storage.
pub const AST_SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS files (
    id TEXT PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    language TEXT NOT NULL,
    loc INTEGER NOT NULL,
    last_modified TEXT NOT NULL,
    content_hash TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS symbols (
    id TEXT PRIMARY KEY,
    file_id TEXT NOT NULL REFERENCES files(id),
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    signature TEXT,
    docstring TEXT,
    connectivity_score INTEGER DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);

CREATE TABLE IF NOT EXISTS call_sites (
    id TEXT PRIMARY KEY,
    caller_symbol_id TEXT REFERENCES symbols(id),
    callee_name TEXT NOT NULL,
    callee_symbol_id TEXT REFERENCES symbols(id),
    line INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_calls_caller ON call_sites(caller_symbol_id);
CREATE INDEX IF NOT EXISTS idx_calls_callee ON call_sites(callee_symbol_id);

CREATE TABLE IF NOT EXISTS imports (
    id TEXT PRIMARY KEY,
    file_id TEXT NOT NULL REFERENCES files(id),
    module_path TEXT NOT NULL,
    imported_names TEXT NOT NULL
);";

/// Creates the AST schema in the supplied database.
///
/// # Errors
///
/// Returns `AstError::Database` when schema creation fails.
pub fn create_schema(database: &Connection) -> AstResult<()> {
    database.execute_batch(AST_SCHEMA_SQL)?;
    Ok(())
}

/// Initializes the AST database schema. Must be called once after opening the connection.
///
/// # Errors
///
/// Returns `AstError::Database` when schema creation fails.
pub fn initialize_schema(connection: &Connection) -> AstResult<()> {
    connection.execute_batch(AST_SCHEMA_SQL)?;
    Ok(())
}

/// Inserts or replaces a file record in the files table.
///
/// # Errors
///
/// Returns `AstError::Database` when insertion fails.
pub fn insert_file(
    database: &Connection,
    file_path: &std::path::Path,
    language: LanguageKind,
    loc: usize,
    content: &str,
) -> AstResult<()> {
    let file_id = file_id_for_path(file_path);
    let content_fingerprint = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        file_path.hash(&mut hasher);
        content.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    };
    database.execute(
        "INSERT OR REPLACE INTO files (
            id, path, language, loc, last_modified, content_hash
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            file_id,
            file_path.to_string_lossy(),
            serde_json::to_string(&language).unwrap_or_else(|_| "\"Rust\"".to_string()),
            i64::try_from(loc).unwrap_or(i64::MAX),
            Utc::now().to_rfc3339(),
            content_fingerprint,
        ],
    )?;
    Ok(())
}

/// Inserts or replaces one symbol and its owning file row.
///
/// # Errors
///
/// Returns `AstError::Database` when insertion fails.
pub fn insert_symbol(database: &Connection, symbol: &Symbol) -> AstResult<()> {
    let file_id = file_id_for_path(&symbol.file_path);
    let content_fingerprint = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        symbol.name.hash(&mut hasher);
        symbol.file_path.hash(&mut hasher);
        symbol.start_line.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    };
    database.execute(
        "INSERT OR REPLACE INTO files (
            id, path, language, loc, last_modified, content_hash
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            file_id,
            symbol.file_path.to_string_lossy(),
            serde_json::to_string(&symbol.language).unwrap_or_else(|_| "\"Rust\"".to_string()),
            i64::try_from(symbol.end_line).unwrap_or(i64::MAX),
            Utc::now().to_rfc3339(),
            content_fingerprint,
        ],
    )?;
    database.execute(
        "INSERT OR REPLACE INTO symbols (
            id, file_id, name, kind, start_line, end_line, signature, docstring
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            symbol.id.to_string(),
            file_id,
            symbol.name,
            serde_json::to_string(&symbol.kind).unwrap_or_else(|_| "\"Function\"".to_string()),
            i64::try_from(symbol.start_line).unwrap_or(i64::MAX),
            i64::try_from(symbol.end_line).unwrap_or(i64::MAX),
            symbol.signature,
            symbol.docstring,
        ],
    )?;
    Ok(())
}

/// Queries a symbol by identifier.
///
/// # Errors
///
/// Returns `AstError` when query or row reconstruction fails.
pub fn query_symbol(database: &Connection, symbol_id: &SymbolId) -> AstResult<Option<Symbol>> {
    let mut statement = database.prepare(
        "SELECT s.id, s.name, s.kind, f.path, s.start_line, s.end_line,
            s.signature, s.docstring, f.language
         FROM symbols s JOIN files f ON s.file_id = f.id
         WHERE s.id = ?1",
    )?;
    let mut rows = statement.query([symbol_id.to_string()])?;
    if let Some(row) = rows.next()? {
        let id_text: String = row.get(0)?;
        let kind_str: String = row.get(2)?;
        let path_text: String = row.get(3)?;
        let start_line: i64 = row.get(4)?;
        let end_line: i64 = row.get(5)?;
        let language_str: String = row.get(8)?;
        let kind: SymbolKind =
            serde_json::from_str(&kind_str).map_err(|deserialization_error| {
                AstError::CorruptDatabase {
                    detail: format!(
                        "failed to deserialize symbol kind '{kind_str}': {deserialization_error}"
                    ),
                }
            })?;
        let language: LanguageKind =
            serde_json::from_str(&language_str).map_err(|deserialization_error| {
                AstError::CorruptDatabase {
                    detail: format!(
                    "failed to deserialize language kind '{language_str}': {deserialization_error}"
                ),
                }
            })?;
        Ok(Some(Symbol {
            id: SymbolId::from_str(&id_text)?,
            name: row.get(1)?,
            kind,
            file_path: PathBuf::from(path_text),
            start_line: usize::try_from(start_line).unwrap_or_default(),
            end_line: usize::try_from(end_line).unwrap_or_default(),
            signature: row.get(6)?,
            docstring: row.get(7)?,
            language,
        }))
    } else {
        Ok(None)
    }
}

fn file_id_for_path(file_path: &std::path::Path) -> String {
    SymbolId::from_parts(&file_path.to_string_lossy(), "file", "file").to_string()
}
