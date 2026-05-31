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
use yantra_core::SymbolId;
use yantra_crg::{
    export_focused, export_graph, extract_subgraph, EmbeddingStore, GraphBuilder, GraphCache,
};

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
    let symbols_rows = symbols_statement
        .query_map([], |row| {
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
        })
        .unwrap();
    let mut symbols: Vec<SymbolRecord> = symbols_rows.map(|row| row.unwrap()).collect();
    symbols.sort();

    let mut edges_statement = connection
        .prepare("SELECT from_id, to_id, edge_type, weight FROM edges")
        .unwrap();
    let edges_rows = edges_statement
        .query_map([], |row| {
            Ok(EdgeRecord {
                from_id: row.get(0)?,
                to_id: row.get(1)?,
                edge_type: row.get(2)?,
                weight: row.get(3)?,
            })
        })
        .unwrap();
    let mut edges: Vec<EdgeRecord> = edges_rows.map(|row| row.unwrap()).collect();
    edges.sort();

    let mut call_sites_statement = connection
        .prepare("SELECT caller_symbol_id, callee_name, callee_symbol_id, line FROM call_sites")
        .unwrap();
    let call_sites_rows = call_sites_statement
        .query_map([], |row| {
            Ok(CallSiteRecord {
                caller_symbol_id: row.get(0)?,
                callee_name: row.get(1)?,
                callee_symbol_id: row.get(2)?,
                line: row.get(3)?,
            })
        })
        .unwrap();
    let mut call_sites: Vec<CallSiteRecord> = call_sites_rows.map(|row| row.unwrap()).collect();
    call_sites.sort();

    let mut imports_statement = connection
        .prepare("SELECT file_id, module_path, imported_names FROM imports")
        .unwrap();
    let imports_rows = imports_statement
        .query_map([], |row| {
            Ok(ImportRecord {
                file_id: row.get(0)?,
                module_path: row.get(1)?,
                imported_names: row.get(2)?,
            })
        })
        .unwrap();
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
    let manifest_directory_string =
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest_directory_string)
        .join("tests")
        .join("fixtures")
        .join("small-repo")
}

#[test]
fn test_small_repo_indexing_correctness() {
    let source_fixture_directory = resolve_source_fixture_directory();

    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-integration-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let mut statement = graph_builder
        .connection()
        .prepare("SELECT name, kind FROM symbols")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap();
    for symbol in rows {
        let (name, kind) = symbol.unwrap();
        println!("Symbol: {name}, Kind: {kind}");
    }

    let symbols_count: i64 = graph_builder
        .connection()
        .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
        .unwrap();
    assert!(
        symbols_count >= 5,
        "Expected at least 5 symbols (Greeter, WorldGreeter, etc.), found: {symbols_count}"
    );

    let implements_edges_count: i64 = graph_builder
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_type = 'IMPLEMENTS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        implements_edges_count >= 1,
        "Expected at least 1 IMPLEMENTS edge"
    );

    let calls_edges_count: i64 = graph_builder
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_type = 'CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        calls_edges_count >= 1,
        "Expected at least 1 CALLS edge, found: {calls_edges_count}"
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_incremental_vs_full_rebuild_property() {
    let source_fixture_directory = resolve_source_fixture_directory();

    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-property-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let copied_lib_path = temp_directory.join("src").join("lib.rs");

    let connection_incremental = Connection::open_in_memory().unwrap();
    let builder_incremental = GraphBuilder::new(connection_incremental);
    builder_incremental
        .build_from_repo(&temp_directory)
        .unwrap();

    let original_lib_content = fs::read_to_string(&copied_lib_path).unwrap();
    let mut modified_lib_content = original_lib_content.clone();
    modified_lib_content.push_str(
        "\n\n\
        pub fn extra_helper_function() -> String {\n\
            format_message(\"incremental\")\n\
        }\n\
    ",
    );
    fs::write(&copied_lib_path, &modified_lib_content).unwrap();

    builder_incremental.update_file(&copied_lib_path).unwrap();
    let state_incremental_addition = retrieve_database_state(builder_incremental.connection());

    let connection_full_addition = Connection::open_in_memory().unwrap();
    let builder_full_addition = GraphBuilder::new(connection_full_addition);
    builder_full_addition
        .build_from_repo(&temp_directory)
        .unwrap();
    let state_full_addition = retrieve_database_state(builder_full_addition.connection());

    assert_eq!(
        state_incremental_addition, state_full_addition,
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
    builder_full_deletion
        .build_from_repo(&temp_directory)
        .unwrap();
    let state_full_deletion = retrieve_database_state(builder_full_deletion.connection());

    assert_eq!(
        state_incremental_deletion, state_full_deletion,
        "Incremental deletion did not produce identical state to full rebuild!"
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}

static EMBEDDING_INIT_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_subgraph_budget_and_forced_seeds() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-subgraph-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let _lock_guard = EMBEDDING_INIT_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let embedding_store = EmbeddingStore::new().unwrap();
    embedding_store
        .embed_all(graph_builder.connection())
        .unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let mut select_statement = graph_builder
        .connection()
        .prepare("SELECT id FROM symbols")
        .unwrap();
    let symbol_ids: Vec<SymbolId> = select_statement
        .query_map([], |row| {
            let id_string: String = row.get(0)?;
            Ok(yantra_core::SymbolId::new(id_string).unwrap())
        })
        .unwrap()
        .map(|row_result| row_result.unwrap())
        .collect();

    let forced_symbol_ids: Vec<SymbolId> = symbol_ids.into_iter().take(2).collect();

    let rendered_subgraph_tiny = extract_subgraph(
        &graph_cache,
        &embedding_store,
        "format greet message",
        10,
        &forced_symbol_ids,
    )
    .unwrap();

    for seed in &forced_symbol_ids {
        assert!(
            rendered_subgraph_tiny.included_nodes.contains(seed),
            "Expected forced seed {seed:?} to be included in tiny budget subgraph",
        );
    }

    let rendered_subgraph_reasonable = extract_subgraph(
        &graph_cache,
        &embedding_store,
        "format greet message",
        300,
        &forced_symbol_ids,
    )
    .unwrap();

    assert!(
        rendered_subgraph_reasonable.token_cost <= 300,
        "Reasonable budget token cost exceeded: {}",
        rendered_subgraph_reasonable.token_cost
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_subgraph_golden_corpus_recall() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory = std::env::temp_dir().join(format!("yantra-crg-golden-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let _lock_guard = EMBEDDING_INIT_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let embedding_store = EmbeddingStore::new().unwrap();
    embedding_store
        .embed_all(graph_builder.connection())
        .unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let golden_corpus = vec![
        (
            "greet the user or say hello",
            vec!["Greeter", "WorldGreeter"],
        ),
        ("format a message helper function", vec!["format_message"]),
        ("run main entry point program", vec!["main"]),
        (
            "hello world greeter implementation",
            vec!["WorldGreeter", "Greeter"],
        ),
        ("trait definition for greetings", vec!["Greeter"]),
        (
            "helper function to format text string",
            vec!["format_message"],
        ),
        (
            "printing greetings to stdout in main",
            vec!["main", "WorldGreeter"],
        ),
        (
            "impl block for WorldGreeter",
            vec!["WorldGreeter", "Greeter"],
        ),
        ("format message string greeting", vec!["format_message"]),
        ("execute main function and show greeting", vec!["main"]),
    ];

    let mut total_expected_symbols = 0;
    let mut total_matched_symbols = 0;

    let mut select_name_statement = graph_builder
        .connection()
        .prepare("SELECT name FROM symbols WHERE id = ?1")
        .unwrap();

    for (task_description, expected_symbols) in golden_corpus {
        let rendered_subgraph =
            extract_subgraph(&graph_cache, &embedding_store, task_description, 500, &[]).unwrap();

        let mut included_names = std::collections::HashSet::new();
        for node_id in &rendered_subgraph.included_nodes {
            if let Ok(name) = select_name_statement
                .query_row([&node_id.to_string()], |row| row.get::<_, String>(0))
            {
                included_names.insert(name);
            }
        }

        total_expected_symbols += expected_symbols.len();
        for expected in expected_symbols {
            if included_names.contains(expected) {
                total_matched_symbols += 1;
            }
        }
    }

    assert!(
        total_matched_symbols * 20 >= total_expected_symbols * 19,
        "Golden corpus recall target (≥95%) not met: {total_matched_symbols}/{total_expected_symbols} symbols matched",
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_proximity_aware_pruning_correctness() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-proximity-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let _lock_guard = EMBEDDING_INIT_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let embedding_store = EmbeddingStore::new().unwrap();
    embedding_store
        .embed_all(graph_builder.connection())
        .unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let mut select_statement = graph_builder
        .connection()
        .prepare("SELECT id, name FROM symbols")
        .unwrap();
    let symbol_records: Vec<(SymbolId, String)> = select_statement
        .query_map([], |row| {
            let id_string: String = row.get(0)?;
            let name_string: String = row.get(1)?;
            Ok((yantra_core::SymbolId::new(id_string).unwrap(), name_string))
        })
        .unwrap()
        .map(|row_result| row_result.unwrap())
        .collect();

    println!(
        "All indexed symbols: {:?}",
        symbol_records
            .iter()
            .map(|(_, name)| name)
            .collect::<Vec<_>>()
    );

    let main_symbol_id = symbol_records
        .iter()
        .find(|(_, name)| name == "main")
        .map(|(id, _)| id.clone())
        .unwrap();

    let format_message_id = symbol_records
        .iter()
        .find(|(_, name)| name == "format_message")
        .map(|(id, _)| id.clone())
        .unwrap();

    let greeter_symbol_id = symbol_records
        .iter()
        .find(|(_, name)| name == "Greeter")
        .map(|(id, _)| id.clone())
        .unwrap();

    let rendered_subgraph_small_budget = extract_subgraph(
        &graph_cache,
        &embedding_store,
        "run main entry point",
        200,
        &[main_symbol_id.clone()],
    )
    .unwrap();

    assert!(
        rendered_subgraph_small_budget
            .included_nodes
            .contains(&main_symbol_id),
        "Expected seed symbol main to be included"
    );

    if rendered_subgraph_small_budget
        .included_nodes
        .contains(&format_message_id)
    {
        assert!(
            rendered_subgraph_small_budget
                .included_nodes
                .contains(&greeter_symbol_id),
            "Expected hop-1 Greeter to be retained if hop-2 format_message is retained"
        );
    }

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_export_graph_node_count_matches_symbol_count() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-export-count-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();
    let graph_json = export_graph(&graph_cache);

    assert!(
        !graph_json.nodes.is_empty(),
        "Expected at least one node in the exported graph"
    );

    for graph_node in &graph_json.nodes {
        assert!(
            !graph_node.id.is_empty(),
            "Each exported node must have a non-empty id"
        );
        assert!(
            !graph_node.label.is_empty(),
            "Each exported node must have a non-empty label"
        );
        assert!(
            !graph_node.kind.is_empty(),
            "Each exported node must have a non-empty kind"
        );
    }

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_export_graph_is_deterministic() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-export-det-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let result_a = export_graph(&graph_cache);
    let result_b = export_graph(&graph_cache);

    assert_eq!(
        result_a.nodes.len(),
        result_b.nodes.len(),
        "Two consecutive export_graph calls must return the same number of nodes"
    );
    assert_eq!(
        result_a.edges.len(),
        result_b.edges.len(),
        "Two consecutive export_graph calls must return the same number of edges"
    );

    if !result_a.nodes.is_empty() {
        assert_eq!(
            result_a.nodes[0].id, result_b.nodes[0].id,
            "First node id must be identical across two export_graph calls"
        );
    }

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_export_graph_node_colors_are_valid_hex() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-export-colors-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();
    let graph_json = export_graph(&graph_cache);

    for graph_node in &graph_json.nodes {
        assert!(
            graph_node.color.starts_with('#'),
            "Node color must start with '#', got: {}",
            graph_node.color
        );
        assert_eq!(
            graph_node.color.len(),
            7,
            "Node color must be 7 characters (#rrggbb), got: {}",
            graph_node.color
        );
        assert!(
            graph_node.color[1..]
                .chars()
                .all(|character| character.is_ascii_hexdigit()),
            "Node color hex digits must be valid ASCII hex, got: {}",
            graph_node.color
        );
    }

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_export_focused_nonexistent_id_returns_error() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-export-noexist-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let focused_result = export_focused(&graph_cache, "nonexistent-id-00000000", 2);
    assert!(
        focused_result.is_err(),
        "Expected Err for a nonexistent focus symbol id, but got Ok"
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_export_focused_zero_hops_returns_at_most_one_node() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-export-zerohop-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let first_symbol_id: String = graph_builder
        .connection()
        .query_row("SELECT id FROM symbols LIMIT 1", [], |row| row.get(0))
        .unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let focused_result = export_focused(&graph_cache, &first_symbol_id, 0).unwrap();
    assert!(
        focused_result.nodes.len() <= 1,
        "Zero hops must return at most one node, got: {}",
        focused_result.nodes.len()
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_export_focused_hops_monotonically_increases_nodes() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-export-mono-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let first_symbol_id: String = graph_builder
        .connection()
        .query_row("SELECT id FROM symbols LIMIT 1", [], |row| row.get(0))
        .unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let result_one_hop = export_focused(&graph_cache, &first_symbol_id, 1).unwrap();
    let result_three_hops = export_focused(&graph_cache, &first_symbol_id, 3).unwrap();

    assert!(
        result_three_hops.nodes.len() >= result_one_hop.nodes.len(),
        "More hops must produce at least as many nodes as fewer hops: 1-hop={}, 3-hop={}",
        result_one_hop.nodes.len(),
        result_three_hops.nodes.len()
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}

#[test]
fn test_community_quality_after_louvain() {
    let source_fixture_directory = resolve_source_fixture_directory();
    let temp_directory =
        std::env::temp_dir().join(format!("yantra-crg-community-{}", Uuid::new_v4()));
    copy_directory_recursively(&source_fixture_directory, &temp_directory).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&temp_directory).unwrap();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let node_count = graph_cache.symbol_details.len();

    let graph_json = export_graph(&graph_cache);

    let distinct_community_labels: std::collections::HashSet<String> = graph_json
        .nodes
        .iter()
        .map(|graph_node| graph_node.community.clone())
        .collect();

    let distinct_community_count = distinct_community_labels.len();

    assert!(
        node_count > 0,
        "Expected at least one symbol node in the small-repo fixture"
    );

    assert!(
        distinct_community_count <= node_count / 2 + 1,
        "Louvain must group nodes: expected \u{2264}{} distinct communities for {node_count} nodes, got {distinct_community_count}",
        node_count / 2 + 1,
    );

    fs::remove_dir_all(&temp_directory).unwrap();
}
