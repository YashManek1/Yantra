//! # forge-agents: Coder Integration Tests
//!
//! Tests the full `CoderAgent` pipeline against a small in-memory CRG fixture
//! that contains an `add_numbers` function. A deterministic mock `ModelProvider`
//! at Tier 0 returns a hard-coded unified diff so tests remain offline and fast.
//!
//! Tests that only assert on the model response (not CRG content) use a
//! `MockCrgServer` to avoid loading the fastembed ONNX model, which prevents
//! race conditions on the shared HuggingFace model cache in CI.
//!
//! ## Input
//! - Temporary project directory with a single `lib.rs` fixture file
//! - `MockProvider` returning a predetermined diff response
//!
//! ## Output
//! - Assertions on `TaskResult` fields: outcome, diff file path, diff content
//!
//! ## Related
//! - `forge-agents::coder` — the implementation under test
//! - `forge-crg` — builds the in-memory graph used for CRG MCP calls
//! - `forge-tools::crg` — `CrgMcpServer` registered into the `McpRouter`

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use rusqlite::Connection;
use serde_json::{json, Value};
use yantra_agents::{Agent, AgentContext, AgentError, CoderAgent};
use yantra_core::{
    AgentKind, ModelCapability, ModelTier, Outcome, SessionId, Strictness, TaskClass, TaskId,
    TaskNode, TaskStatus, TruthToken,
};
use yantra_crg::{EmbeddingStore, GraphBuilder, GraphCache};
use yantra_router::{
    CompletionRequest, CompletionResponse, FinishReason, ModelProvider, ProviderError,
    ProviderStatus, Router, RoutingPolicy,
};
use yantra_tools::{CrgMcpServer, McpError, McpRouter, McpServer, ToolCapability};

static EMBEDDING_LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();

fn embedding_lock() -> &'static std::sync::Mutex<()> {
    EMBEDDING_LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Lightweight CRG stand-in that returns an empty subgraph without loading the
/// ONNX embedding model. Used by tests that only assert on model response
/// content, not on CRG retrieval quality.
struct MockCrgServer;

#[async_trait]
impl McpServer for MockCrgServer {
    fn name(&self) -> &'static str {
        "crg"
    }

    async fn handle(&self, _method: &str, _params: Value) -> Result<Value, McpError> {
        Ok(json!({ "text": "", "token_cost": 0, "included_node_count": 0 }))
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }
}

fn build_add_numbers_fixture() -> (Connection, EmbeddingStore, GraphCache) {
    let fixture_dir = std::env::temp_dir().join(format!("yantra-agents-coder-{}", TaskId::new()));
    std::fs::create_dir_all(&fixture_dir).unwrap();
    std::fs::write(
        fixture_dir.join("lib.rs"),
        "pub fn add_numbers(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(&fixture_dir).unwrap();

    let _embedding_guard = embedding_lock().lock().unwrap();
    let embedding_store = EmbeddingStore::new().unwrap();
    embedding_store
        .embed_all(graph_builder.connection())
        .unwrap();
    drop(_embedding_guard);

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();
    let _ = std::fs::remove_dir_all(&fixture_dir);

    let owned_connection = graph_builder.into_connection();
    (owned_connection, embedding_store, graph_cache)
}

fn make_test_truth_token() -> TruthToken {
    use ring::rand::SystemRandom;
    use ring::signature::Ed25519KeyPair;

    let rng = SystemRandom::new();
    let pkcs8_document = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8_document.as_ref()).unwrap();
    TruthToken::new(
        TaskId::new(),
        TaskClass::Docstring,
        Strictness::Trust,
        &key_pair,
    )
    .unwrap()
}

struct MockProvider {
    response_content: String,
}

#[async_trait]
impl ModelProvider for MockProvider {
    fn id(&self) -> &'static str {
        "mock-tier0"
    }

    fn tier(&self) -> ModelTier {
        ModelTier::Tier0
    }

    fn capability(&self) -> ModelCapability {
        ModelCapability {
            context_limit: 4096,
            supports_tools: false,
            cost_per_1k_in: 0.0,
            cost_per_1k_out: 0.0,
        }
    }

    async fn status(&self) -> ProviderStatus {
        ProviderStatus::Available
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        Ok(CompletionResponse {
            content: self.response_content.clone(),
            tool_calls: Vec::new(),
            tokens_in: 10,
            tokens_out: 50,
            finish_reason: FinishReason::Stop,
        })
    }

    async fn complete_stream(
        &self,
        _request: CompletionRequest,
    ) -> Result<
        std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<String, ProviderError>> + Send>>,
        ProviderError,
    > {
        let (channel_sender, channel_receiver) = tokio::sync::mpsc::channel(1);
        let _send_result = channel_sender.send(Ok(self.response_content.clone())).await;
        Ok(Box::pin(yantra_router::provider::ReceiverStream::new(
            channel_receiver,
        )))
    }
}

fn build_agent_context(
    sqlite_connection: Connection,
    embedding_store: EmbeddingStore,
    graph_cache: GraphCache,
    mock_response: String,
) -> AgentContext {
    let crg_server = CrgMcpServer::new(sqlite_connection, embedding_store, graph_cache);
    let mut mcp_router = McpRouter::new(["crg.subgraph".to_owned()]);
    mcp_router.register_server(Arc::new(crg_server));
    build_context_from_mcp(mcp_router, mock_response)
}

fn build_context_with_mock_crg(mock_response: String) -> AgentContext {
    let mut mcp_router = McpRouter::new(["crg.subgraph".to_owned()]);
    mcp_router.register_server(Arc::new(MockCrgServer));
    build_context_from_mcp(mcp_router, mock_response)
}

fn build_context_from_mcp(mcp_router: McpRouter, mock_response: String) -> AgentContext {
    let mock_provider: Arc<dyn ModelProvider> = Arc::new(MockProvider {
        response_content: mock_response,
    });
    let router = Router::new(RoutingPolicy::default(), vec![mock_provider]);
    let temp_dir = std::env::temp_dir().join(format!("yantra-coder-test-{}", TaskId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let memory =
        Arc::new(yantra_memory::MemoryService::new(&temp_dir.join("memory.sqlite")).unwrap());
    AgentContext {
        router: Arc::new(router),
        tools: mcp_router,
        session_id: SessionId::new(),
        upstream_results: Vec::new(),
        memory,
    }
}

fn make_docstring_task() -> TaskNode {
    TaskNode {
        id: TaskId::new(),
        description: "add docstring to `add_numbers`".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::Docstring,
        dependencies: Vec::new(),
        assigned_agent: Some(AgentKind::Coder),
        truth_token: Some(make_test_truth_token()),
        parent_decision_id: None,
    }
}

#[tokio::test]
async fn coder_produces_docstring_diff_for_add_numbers() {
    let (sqlite_connection, embedding_store, graph_cache) = build_add_numbers_fixture();

    let mock_diff_response = "\
I'll add a `///` doc comment to `add_numbers`.

// File: lib.rs
--- a/lib.rs
+++ b/lib.rs
@@ -1,3 +1,4 @@
+/// Adds two integers and returns their sum.
 pub fn add_numbers(a: i32, b: i32) -> i32 {
     a + b
 }"
    .to_owned();

    let context = build_agent_context(
        sqlite_connection,
        embedding_store,
        graph_cache,
        mock_diff_response,
    );
    let agent = CoderAgent::new();
    let result = agent.run(make_docstring_task(), context).await.unwrap();

    assert_eq!(result.outcome, Outcome::Success);
    let diff = result.diff.expect("expected a diff");
    assert_eq!(diff.files.len(), 1);
    assert_eq!(diff.files[0].file_path, "lib.rs");
    assert!(
        diff.files[0].unified_diff.contains("Adds two integers"),
        "diff should contain the docstring text"
    );
}

#[tokio::test]
async fn coder_returns_failure_outcome_when_model_produces_no_diff() {
    let (sqlite_connection, embedding_store, graph_cache) = build_add_numbers_fixture();
    let context = build_agent_context(
        sqlite_connection,
        embedding_store,
        graph_cache,
        "I cannot determine which symbols to modify without more context.".to_owned(),
    );
    let agent = CoderAgent::new();
    let result = agent.run(make_docstring_task(), context).await.unwrap();

    assert_eq!(result.outcome, Outcome::Failure);
    assert!(result.diff.is_none());
}

#[tokio::test]
async fn coder_returns_error_when_truth_token_missing() {
    let (sqlite_connection, embedding_store, graph_cache) = build_add_numbers_fixture();
    let context = build_agent_context(
        sqlite_connection,
        embedding_store,
        graph_cache,
        String::new(),
    );

    let task_without_token = TaskNode {
        id: TaskId::new(),
        description: "add docstring to `add_numbers`".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::Docstring,
        dependencies: Vec::new(),
        assigned_agent: Some(AgentKind::Coder),
        truth_token: None,
        parent_decision_id: None,
    };

    let agent = CoderAgent::new();
    let error = agent.run(task_without_token, context).await.unwrap_err();

    assert!(matches!(error, AgentError::MissingTruthToken { .. }));
}

#[tokio::test]
async fn coder_diff_summary_line_matches_first_response_line() {
    let context = build_context_with_mock_crg(
        "Adding docstring to add_numbers.\n\n// File: lib.rs\n--- a/lib.rs\n+++ b/lib.rs\n@@ -1 +1,2 @@\n+/// Doc.\n pub fn add_numbers".to_owned(),
    );

    let agent = CoderAgent::new();
    let result = agent.run(make_docstring_task(), context).await.unwrap();

    assert_eq!(result.summary, "Adding docstring to add_numbers.");
}
