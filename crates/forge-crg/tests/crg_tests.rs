//! # Code-Review Graph: Integration and Property Tests
//!
//! Verifies repository walking, symbol and relationship indexing, connectivity score
//! calculations on the `small-repo` fixture, and runs a property consistency test
//! checking that incremental saves produce identical graph states to full rebuilds.
//!
//! ## Input
//! - The `small-repo` fixture directory
//! - Modifying file events simulated on a temporary copy of the fixture
//!
//! ## Output
//! - Test assertion results
//!
//! ## Related
//! - `forge-crg::builder` — the builder target of these tests
//! - `tests/fixtures/small-repo/` — fixture codebase used for testing

use std::fs;
use std::path::{Path, PathBuf};
use rusqlite::Connection;
use uuid::Uuid;
use yantra_crg::GraphBuilder;

#[derive(Debug, PartialEq, Eq, Clone, Hash, Ord, PartialOrd)]
struct SymbolRecord {
    id: String,
    file_id: String,
    name: String,
    kind: String,
    start_line: i32,
    end_line: i32,
    signature: Option<String>,
    docstring: Option<String>,
    connectivity_score: i32,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, Ord, PartialOrd)]
struct EdgeRecord {
    from_id: String,
    to_id: String,
    edge_type: String,
    weight: i32,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, Ord, PartialOrd)]
struct CallSiteRecord {
    caller_symbol_id: Option<String>,
    callee_name: String,
    callee_symbol_id: Option<String>,
    line: i32,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, Ord, PartialOrd)]
struct ImportRecord {
    file_id: String,
    module_path: String,
    imported_names: String,
}

#[derive(Debug, PartialEq, Eq, Clone)]
struct DatabaseState {
    symbols: Vec<SymbolRecord>,
    edges: Vec<EdgeRecord>,
    call_sites: Vec<CallSiteRecord>,
    imports: Vec<ImportRecord>,
}

fn retrieve_database_state(connection: &Connection) -> DatabaseState {
    let mut symbols_statement = connection.prepare(
        "SELECT id, file_id, name, kind, start_line, end_line, signature, docstring, connectivity_score FROM symbols"
    ).unwrap();
    let symbols_rows = symbols_statement.query_map([], |row| {
        Ok(SymbolRecord {
            id: row.get(0)?,
            file_id: row.get(1)?,
            name: row.get(2)?,
            kind: row.get(3)?,
            start_line: row.get(4)?,
            end_line: row.get(5)?,
            signature: row.get(6)?,
            docstring: row.get(7)?,
            connectivity_score: row.get(8)?,
        })
    }).unwrap();
    let mut symbols: Vec<SymbolRecord> = symbols_rows.map(|row| row.unwrap()).collect();
    symbols.sort();

    let mut edges_statement = connection.prepare(
        "SELECT from_id, to_id, edge_type, weight FROM edges"
    ).unwrap();
    let edges_rows = edges_statement.query_map([], |row| {
        Ok(EdgeRecord {
            from_id: row.get(0)?,
            to_id: row.get(1)?,
            edge_type: row.get(2)?,
            weight: row.get(3)?,
        })
    }).unwrap();
    let mut edges: Vec<EdgeRecord> = edges_rows.map(|row| row.unwrap()).collect();
    edges.sort();

    let mut call_sites_statement = connection.prepare(
        "SELECT caller_symbol_id, callee_name, callee_symbol_id, line FROM call_sites"
    ).unwrap();
    let call_sites_rows = call_sites_statement.query_map([], |row| {
        Ok(CallSiteRecord {
            caller_symbol_id: row.get(0)?,
            callee_name: row.get(1)?,
            callee_symbol_id: row.get(2)?,
            line: row.get(3)?,
        })
    }).unwrap();
    let mut call_sites: Vec<CallSiteRecord> = call_sites_rows.map(|row| row.unwrap()).collect();
    call_sites.sort();

    let mut imports_statement = connection.prepare(
        "SELECT file_id, module_path, imported_names FROM imports"
    ).unwrap();
    let imports_rows = imports_statement.query_map([], |row| {
        Ok(ImportRecord {
            file_id: row.get(0)?,
            module_path: row.get(1)?,
            imported_names: row.get(2)?,
        })
    }).unwrap();
    let mut imports: Vec<ImportRecord> = imports_rows.map(|row| row.unwrap()).collect();
    imports.sort();

    DatabaseState {
        symbols,
        edges,
        call_sites,
        imports,
    }
}

fn copy_directory_recursively(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let entry_type = entry.file_type()?;
        let destination_path = destination.join(entry.file_name());
        if entry_type.is_dir() {
            copy_directory_recursively(&entry.path(), &destination_path)?;
        } else {
            fs::copy(entry.path(), &destination_path)?;
        }
    }
    Ok(())
}

fn resolve_source_fixture_directory() -> PathBuf {
    let manifest_directory_string = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest_directory_string)
        .join("tests")
        .join("fixtures")
        .join("small-repo")
}

#[test]
fn test_small_repo_indexing_correctness() {
    let source_fixture_directory = resolve_source_fixture_directory();

    let temp_directory = std::env::temp_dir().join(format!("yantra-crg-integration-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let mut statement = graph_builder.connection().prepare(
        "SELECT name, kind FROM symbols"
    ).unwrap();
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }).unwrap();
    for symbol in rows {
        let (name, kind) = symbol.unwrap();
        println!("Symbol: {}, Kind: {}", name, kind);
    }

    let symbols_count: i64 = graph_builder.connection().query_row(
        "SELECT COUNT(*) FROM symbols",
        [],
        |row| row.get(0)
    ).unwrap();
    assert!(symbols_count >= 5, "Expected at least 5 symbols (Greeter, WorldGreeter, etc.), found: {}", symbols_count);

    let implements_edges_count: i64 = graph_builder.connection().query_row(
        "SELECT COUNT(*) FROM edges WHERE edge_type = 'IMPLEMENTS'",
        [],
        |row| row.get(0)
    ).unwrap();
    assert!(implements_edges_count >= 1, "Expected at least 1 IMPLEMENTS edge");

    let calls_edges_count: i64 = graph_builder.connection().query_row(
        "SELECT COUNT(*) FROM edges WHERE edge_type = 'CALLS'",
        [],
        |row| row.get(0)
    ).unwrap();
    assert!(calls_edges_count >= 1, "Expected at least 1 CALLS edge, found: {}", calls_edges_count);

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_incremental_vs_full_rebuild_property() {
    let source_fixture_directory = resolve_source_fixture_directory();

    let temp_directory = std::env::temp_dir().join(format!("yantra-crg-property-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let copied_lib_path = temp_directory.join("src").join("lib.rs");

    let connection_incremental = Connection::open_in_memory().unwrap();
    let builder_incremental = GraphBuilder::new(connection_incremental);
    builder_incremental.build_from_repo(&temp_directory).unwrap();

    let original_lib_content = fs::read_to_string(&copied_lib_path).unwrap();
    let mut modified_lib_content = original_lib_content.clone();
    modified_lib_content.push_str("\n\n\
        pub fn extra_helper_function() -> String {\n\
            format_message(\"incremental\")\n\
        }\n\
    ");
    fs::write(&copied_lib_path, &modified_lib_content).unwrap();

    builder_incremental.update_file(&copied_lib_path).unwrap();
    let state_incremental_addition = retrieve_database_state(builder_incremental.connection());

    let connection_full_addition = Connection::open_in_memory().unwrap();
    let builder_full_addition = GraphBuilder::new(connection_full_addition);
    builder_full_addition.build_from_repo(&temp_directory).unwrap();
    let state_full_addition = retrieve_database_state(builder_full_addition.connection());

    assert_eq!(
        state_incremental_addition,
        state_full_addition,
        "Incremental addition did not produce identical state to full rebuild!"
    );

    let mut deleted_lib_content = original_lib_content;
    let format_message_index = deleted_lib_content.find("pub fn format_message").unwrap();
    deleted_lib_content.truncate(format_message_index);
    fs::write(&copied_lib_path, &deleted_lib_content).unwrap();

    builder_incremental.update_file(&copied_lib_path).unwrap();
    let state_incremental_deletion = retrieve_database_state(builder_incremental.connection());

    let connection_full_deletion = Connection::open_in_memory().unwrap();
    let builder_full_deletion = GraphBuilder::new(connection_full_deletion);
    builder_full_deletion.build_from_repo(&temp_directory).unwrap();
    let state_full_deletion = retrieve_database_state(builder_full_deletion.connection());

    assert_eq!(
        state_incremental_deletion,
        state_full_deletion,
        "Incremental deletion did not produce identical state to full rebuild!"
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}
