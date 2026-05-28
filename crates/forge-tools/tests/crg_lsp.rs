//! # forge-tools: CRG and LSP Integration Tests
//!
//! Tests that exercise the CRG MCP server and LSP MCP server through the public
//! `yantra-tools` API. CRG tests build an in-memory graph on a small Rust
//! fixture; LSP tests verify graceful error handling when no server binary
//! is available.
//!
//! ## Input
//! - Temporary project directories with Rust fixture files
//!
//! ## Output
//! - Assertions over MCP responses, error codes, and JSON shapes
//!
//! ## Related
//! - `yantra_tools::CrgMcpServer`
//! - `yantra_tools::LspMcpServer`
//! - `yantra_tools::McpRouter`

use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::Connection;
use serde_json::{json, Value};
use yantra_core::TaskId;
use yantra_crg::{EmbeddingStore, GraphBuilder, GraphCache};
use yantra_lsp::LspBridge;
use yantra_tools::{CrgMcpServer, LspMcpServer, McpRouter};

static EMBEDDING_INIT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn embedding_init_lock() -> &'static Mutex<()> {
    EMBEDDING_INIT_LOCK.get_or_init(|| Mutex::new(()))
}

fn temporary_project(label: &str) -> std::path::PathBuf {
    let project_directory =
        std::env::temp_dir().join(format!("yantra-tools-crg-{label}-{}", TaskId::new()));
    if project_directory.exists() {
        std::fs::remove_dir_all(&project_directory).unwrap();
    }
    std::fs::create_dir_all(&project_directory).unwrap();
    project_directory
}

fn build_small_crg_fixture() -> (rusqlite::Connection, EmbeddingStore, GraphCache) {
    let project_directory = temporary_project("crg-fixture");

    std::fs::write(
        project_directory.join("lib.rs"),
        r#"
pub fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

pub fn farewell(name: &str) -> String {
    let greeting = greet(name);
    format!("Goodbye from: {}", greeting)
}
"#,
    )
    .unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&project_directory).unwrap();

    let _embedding_guard = embedding_init_lock().lock().unwrap();
    let embedding_store = EmbeddingStore::new().unwrap();
    embedding_store
        .embed_all(graph_builder.connection())
        .unwrap();
    drop(_embedding_guard);

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    let _ = std::fs::remove_dir_all(&project_directory);

    let owned_connection = graph_builder.into_connection();
    (owned_connection, embedding_store, graph_cache)
}

#[tokio::test]
async fn crg_subgraph_returns_rendered_text() {
    let (sqlite_connection, embedding_store, graph_cache) = build_small_crg_fixture();
    let server = CrgMcpServer::new(sqlite_connection, embedding_store, graph_cache);

    let response = server
        .subgraph(&json!({ "task": "greet a user by name", "budget": 2000 }))
        .await
        .unwrap();

    assert!(response["text"]
        .as_str()
        .is_some_and(|text| !text.is_empty()));
    assert!(response["token_cost"].as_u64().unwrap_or(0) <= 2000);
    assert!(response["included_node_count"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test]
async fn crg_impact_of_change_returns_report() {
    let (sqlite_connection, embedding_store, graph_cache) = build_small_crg_fixture();

    let root_symbol_id: String = sqlite_connection
        .query_row(
            "SELECT id FROM symbols WHERE name = 'greet' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let server = CrgMcpServer::new(sqlite_connection, embedding_store, graph_cache);
    let response = server
        .impact_of_change(&json!({ "symbol_id": root_symbol_id }))
        .await
        .unwrap();

    assert_eq!(response["root_symbol"]["name"], "greet");
    assert!(response["affected_symbols"].is_array());
    assert!(response["hop_map"].is_object());
}

#[tokio::test]
async fn crg_find_pattern_returns_symbols_list() {
    let (sqlite_connection, embedding_store, graph_cache) = build_small_crg_fixture();
    let server = CrgMcpServer::new(sqlite_connection, embedding_store, graph_cache);

    let response = server
        .find_pattern(&json!({ "query": "greet" }))
        .await
        .unwrap();

    let symbols = response["symbols"].as_array().unwrap();
    assert!(
        symbols
            .iter()
            .any(|symbol| symbol["name"].as_str().unwrap_or("").contains("greet")),
        "Expected at least one symbol with 'greet' in name, got: {symbols:?}"
    );
}

#[tokio::test]
async fn crg_dependencies_of_returns_callee_list() {
    let (sqlite_connection, embedding_store, graph_cache) = build_small_crg_fixture();

    let farewell_id: String = sqlite_connection
        .query_row(
            "SELECT id FROM symbols WHERE name = 'farewell' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let server = CrgMcpServer::new(sqlite_connection, embedding_store, graph_cache);
    let response = server
        .dependencies_of(&json!({ "symbol_id": farewell_id }))
        .await
        .unwrap();

    assert!(response["dependencies"].is_array());
}

#[tokio::test]
async fn crg_mcp_json_rpc_routing_via_router() {
    let (sqlite_connection, embedding_store, graph_cache) = build_small_crg_fixture();
    let server = CrgMcpServer::new(sqlite_connection, embedding_store, graph_cache);
    let mut router = McpRouter::new(["crg.find_pattern".to_owned()]);
    router.register_server(Arc::new(server));

    let request = json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "crg.find_pattern",
        "params": { "query": "greet" }
    });
    let response_line = router
        .handle_json_rpc_line(&request.to_string())
        .await
        .unwrap();
    let response: Value = serde_json::from_str(&response_line).unwrap();

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 10);
    assert!(response.get("error").is_none());
    assert!(response["result"]["symbols"].is_array());
}

#[tokio::test]
async fn lsp_get_definition_returns_error_when_server_unavailable() {
    let project_directory = temporary_project("lsp-unavailable");
    let bridge = LspBridge::with_configs(&project_directory, {
        use yantra_lsp::{Language, LspServerConfig};
        let mut configs = std::collections::HashMap::new();
        configs.insert(
            Language::Rust,
            LspServerConfig {
                binary: "yantra-nonexistent-binary-xyz".to_owned(),
                args: vec![],
            },
        );
        configs
    });
    let server = LspMcpServer::new(bridge);

    let response = server
        .get_definition(&json!({ "file": "src/lib.rs", "line": 0, "column": 0 }))
        .await;

    assert!(
        response.is_err(),
        "Expected error when server binary is unavailable"
    );
    let error_message = response.unwrap_err().message;
    assert!(!error_message.is_empty());
}

#[tokio::test]
async fn lsp_mcp_json_rpc_routing_unknown_method_returns_error_frame() {
    let project_directory = temporary_project("lsp-mcp-routing");
    let bridge = LspBridge::with_configs(&project_directory, {
        use yantra_lsp::{Language, LspServerConfig};
        let mut configs = std::collections::HashMap::new();
        configs.insert(
            Language::Rust,
            LspServerConfig {
                binary: "yantra-nonexistent-binary-xyz".to_owned(),
                args: vec![],
            },
        );
        configs
    });
    let server = LspMcpServer::new(bridge);
    let mut router = McpRouter::new(["lsp.get_hover".to_owned()]);
    router.register_server(Arc::new(server));

    let request = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "lsp.get_hover",
        "params": { "file": "src/lib.rs", "line": 0, "column": 0 }
    });
    let response_line = router
        .handle_json_rpc_line(&request.to_string())
        .await
        .unwrap();
    let response: Value = serde_json::from_str(&response_line).unwrap();

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 5);
    assert!(
        response.get("error").is_some(),
        "Expected error frame when LSP server unavailable"
    );
}
