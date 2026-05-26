//! # Code-Review Graph: Database Schema and Migrations
//!
//! Defines the SQLite tables, indices, and database migration lifecycle for the
//! Code-Review Graph. This schema integrates AST-derived symbols with relational
//! call edges and temporary model session facts.
//!
//! ## Input
//! - Active SQLite database connection
//!
//! ## Output
//! - SQLite table structures and indices created in the target database
//!
//! ## Related
//! - `forge-ast::db` — provides the base symbol/file/call schema definitions
//! - `forge-crg::builder` — populates the tables defined by this schema

use rusqlite::Connection;

pub const CRG_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS files (
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
);

CREATE TABLE IF NOT EXISTS edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    from_id TEXT NOT NULL,
    to_id TEXT NOT NULL,
    edge_type TEXT NOT NULL,
    weight INTEGER DEFAULT 1,
    UNIQUE(from_id, to_id, edge_type)
);
CREATE INDEX IF NOT EXISTS idx_edges_from ON edges(from_id, edge_type);
CREATE INDEX IF NOT EXISTS idx_edges_to ON edges(to_id, edge_type);

CREATE TABLE IF NOT EXISTS graph_facts (
    id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    weight REAL DEFAULT 1.0,
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    source_session TEXT
);
CREATE INDEX IF NOT EXISTS idx_facts_subject ON graph_facts(subject);
CREATE INDEX IF NOT EXISTS idx_facts_validity ON graph_facts(valid_from, valid_until);
";

pub fn create_crg_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(CRG_SCHEMA_SQL)?;
    Ok(())
}
