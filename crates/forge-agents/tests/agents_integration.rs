//! # forge-agents integration tests: Researcher, Tester, Refactorer, Verifier, Committer
//!
//! Tests every new specialist agent against a mock router that returns
//! deterministic responses. No live LLM or subprocess invocations are made;
//! all external calls are intercepted by in-process mocks.
//!
//! The final acceptance test chains Researcher → Coder → Tester → Verifier →
//! Committer to verify that the full agent pipeline is wired correctly.
//!
//! ## Input
//! - Fixture `TaskNode` values with a pre-signed `TruthToken`
//! - `MockProvider` returning predetermined text for each agent type
//!
//! ## Output
//! - Assertions on `TaskResult` fields: outcome, diff presence, summary content
//!
//! ## Related
//! - `forge-agents::coder` — included in the acceptance chain
//! - `tests/coder_tests.rs` — established test helpers reused here

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use yantra_agents::{
    Agent, AgentContext, AgentError, CoderAgent, CommitSigningKey, CommitterAgent, Diff, FileDiff,
    RedTeamAgent, RefactorerAgent, ResearcherAgent, TaskResult, TesterAgent, VerifierAgent,
};
use yantra_core::{
    ModelCapability, ModelTier, Outcome, SessionId, Strictness, TaskClass, TaskId, TaskNode,
    TaskStatus, TruthToken,
};
use yantra_router::{
    CompletionRequest, CompletionResponse, FinishReason, ModelProvider, ProviderError,
    ProviderStatus, Router, RoutingPolicy,
};
use yantra_tools::{McpError, McpRouter, McpServer, ToolCapability};

// ---------------------------------------------------------------------------
// Shared test infrastructure
// ---------------------------------------------------------------------------

/// Mock MCP server that returns a predetermined JSON value for any method call.
struct MockMcpServer {
    server_name: &'static str,
    response_value: Value,
}

#[async_trait]
impl McpServer for MockMcpServer {
    fn name(&self) -> &str {
        self.server_name
    }

    async fn handle(&self, _method: &str, _params: Value) -> Result<Value, McpError> {
        Ok(self.response_value.clone())
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }
}

/// Mock MCP server that rejects all method calls with a given error message.
struct FailingMcpServer {
    server_name: &'static str,
}

#[async_trait]
impl McpServer for FailingMcpServer {
    fn name(&self) -> &str {
        self.server_name
    }

    async fn handle(&self, _method: &str, _params: Value) -> Result<Value, McpError> {
        Err(McpError {
            code: -32_000,
            message: format!("{} is not registered", self.server_name),
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }
}

/// Mock model provider returning a fixed response for a specific tier.
struct MockProvider {
    response_content: String,
    model_tier: ModelTier,
    provider_id: &'static str,
}

#[async_trait]
impl ModelProvider for MockProvider {
    fn id(&self) -> &'static str {
        self.provider_id
    }

    fn tier(&self) -> ModelTier {
        self.model_tier
    }

    fn capability(&self) -> ModelCapability {
        ModelCapability {
            context_limit: 128_000,
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
            tokens_in: 50,
            tokens_out: 100,
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

fn make_mock_router(response_content: impl Into<String>) -> Arc<Router> {
    let content = response_content.into();
    let tier0_provider: Arc<dyn ModelProvider> = Arc::new(MockProvider {
        response_content: content.clone(),
        model_tier: ModelTier::Tier0,
        provider_id: "mock-tier0",
    });
    let tier1_provider: Arc<dyn ModelProvider> = Arc::new(MockProvider {
        response_content: content,
        model_tier: ModelTier::Tier1,
        provider_id: "mock-tier1",
    });
    Arc::new(Router::new(
        RoutingPolicy::default(),
        vec![tier0_provider, tier1_provider],
    ))
}

fn make_crg_mcp_router() -> McpRouter {
    let empty_crg_response = json!({
        "text": "## Symbol: process_request\n```rust\npub fn process_request() {}\n```",
        "token_cost": 50,
        "included_node_count": 1
    });
    let mut mcp_router = McpRouter::new(["crg.subgraph".to_owned()]);
    mcp_router.register_server(Arc::new(MockMcpServer {
        server_name: "crg",
        response_value: empty_crg_response,
    }));
    mcp_router
}

fn make_agent_context(response_content: impl Into<String>) -> AgentContext {
    let temp_dir = std::env::temp_dir().join(format!("yantra-integration-test-{}", TaskId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let memory =
        Arc::new(yantra_memory::MemoryService::new(&temp_dir.join("memory.sqlite")).unwrap());
    AgentContext {
        router: make_mock_router(response_content),
        tools: make_crg_mcp_router(),
        session_id: SessionId::new(),
        upstream_results: Vec::new(),
        memory,
    }
}

fn make_truth_token(task_id: TaskId, task_class: TaskClass) -> TruthToken {
    use ring::rand::SystemRandom;
    use ring::signature::Ed25519KeyPair;

    let random_generator = SystemRandom::new();
    let pkcs8_document = Ed25519KeyPair::generate_pkcs8(&random_generator).unwrap();
    let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8_document.as_ref()).unwrap();
    TruthToken::new(task_id, task_class, Strictness::Light, &key_pair).unwrap()
}

fn make_task(task_class: TaskClass, description: &str) -> TaskNode {
    let task_id = TaskId::new();
    let temp_directory = std::env::temp_dir().join(format!("yantra-empty-{task_id}"));
    let _create_result = std::fs::create_dir_all(&temp_directory);
    let description_with_root = format!(
        "project_root:{} {}",
        temp_directory.to_string_lossy(),
        description
    );
    TaskNode {
        id: task_id,
        description: description_with_root,
        status: TaskStatus::Pending,
        class: task_class,
        dependencies: Vec::new(),
        assigned_agent: None,
        truth_token: Some(make_truth_token(task_id, task_class)),
        parent_decision_id: None,
    }
}

// ---------------------------------------------------------------------------
// ResearcherAgent integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn researcher_returns_success_with_memo_json_in_summary() {
    let memo_response = "SUMMARY: JWT rotation requires the existing token module.\n\
        CONFIDENCE: 0.85\n\
        FINDING: sign_jwt exists | confidence: 0.9 | source: kg\n\
        FINDING: jwt-simple provides rotate() | confidence: 0.8 | source: search";

    let mut mcp_router = McpRouter::new([]);
    mcp_router.register_server(Arc::new(FailingMcpServer {
        server_name: "search",
    }));

    let temp_dir = std::env::temp_dir().join(format!("yantra-integration-test-{}", TaskId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let memory =
        Arc::new(yantra_memory::MemoryService::new(&temp_dir.join("memory.sqlite")).unwrap());
    let context = AgentContext {
        router: make_mock_router(memo_response),
        tools: mcp_router,
        session_id: SessionId::new(),
        upstream_results: Vec::new(),
        memory,
    };

    let task = make_task(TaskClass::NewFeature, "add JWT rotation to auth service");
    let result = ResearcherAgent::new().run(task, context).await.unwrap();

    assert_eq!(result.outcome, Outcome::Success);
    assert!(
        result.diff.is_none(),
        "researcher must never produce a diff"
    );
    let parsed_memo: serde_json::Value =
        serde_json::from_str(&result.summary).expect("summary is valid JSON");
    assert!(
        parsed_memo.get("summary").is_some(),
        "memo JSON must contain a summary field"
    );
}

#[tokio::test]
async fn researcher_returns_error_when_truth_token_missing() {
    let task_without_token = TaskNode {
        id: TaskId::new(),
        description: "add JWT rotation".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: Vec::new(),
        assigned_agent: None,
        truth_token: None,
        parent_decision_id: None,
    };
    let error = ResearcherAgent::new()
        .run(task_without_token, make_agent_context(""))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::MissingTruthToken { .. }));
}

// ---------------------------------------------------------------------------
// TesterAgent integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tester_produces_test_diff_from_mock_response() {
    let tester_response = "Generated tests for process_request.\n\n\
        // File: src/tests.rs\n\
        --- a/src/tests.rs\n\
        +++ b/src/tests.rs\n\
        @@ -0,0 +1,5 @@\n\
        +#[test]\n\
        +fn process_request_returns_ok() {\n\
        +    assert!(true);\n\
        +}\n";

    let task = make_task(TaskClass::NewFeature, "add tests for the new endpoint");
    let result = TesterAgent::new()
        .run(task, make_agent_context(tester_response))
        .await
        .unwrap();

    assert_eq!(result.outcome, Outcome::Success);
    let diff = result.diff.expect("tester must produce a diff");
    assert_eq!(diff.files.len(), 1);
    assert_eq!(diff.files[0].file_path, "src/tests.rs");
    assert!(diff.files[0]
        .unified_diff
        .contains("process_request_returns_ok"));
}

#[tokio::test]
async fn tester_returns_failure_when_model_produces_no_test_diffs() {
    let task = make_task(TaskClass::NewFeature, "add tests for the endpoint");
    let result = TesterAgent::new()
        .run(task, make_agent_context("No tests could be generated."))
        .await
        .unwrap();
    assert_eq!(result.outcome, Outcome::Failure);
    assert!(result.diff.is_none());
}

#[tokio::test]
async fn tester_returns_error_when_truth_token_missing() {
    let task_without_token = TaskNode {
        id: TaskId::new(),
        description: "add tests".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: Vec::new(),
        assigned_agent: None,
        truth_token: None,
        parent_decision_id: None,
    };
    let error = TesterAgent::new()
        .run(task_without_token, make_agent_context(""))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::MissingTruthToken { .. }));
}

// ---------------------------------------------------------------------------
// RefactorerAgent integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refactorer_produces_diff_from_mock_response() {
    let refactorer_response = "GROUNDED SYMBOLS:\n\
        - process_request  [src/lib.rs]\n\n\
        INSERTION POINT: process_request  [src/lib.rs]\n\
        PRIMARY FILES TO EDIT: src/lib.rs\n\n\
        Extracted nested loop into `process_single_item` to reduce complexity.\n\n\
        // File: src/lib.rs\n\
        --- a/src/lib.rs\n\
        +++ b/src/lib.rs\n\
        @@ -1,5 +1,3 @@\n\
        -pub fn process_request() {\n\
        -    let v = vec![1,2,3];\n\
        +pub fn process_request() {\n\
        +    vec![1,2,3].iter().for_each(process_single_item);\n\
         }";

    let task = make_task(TaskClass::Refactor, "reduce complexity in process_request");
    let result = RefactorerAgent::new()
        .run(task, make_agent_context(refactorer_response))
        .await
        .unwrap();

    assert_eq!(result.outcome, Outcome::Success);
    let diff = result.diff.expect("refactorer must produce a diff");
    assert_eq!(diff.files[0].file_path, "src/lib.rs");
    assert!(result.summary.contains("complexity"));
}

#[tokio::test]
async fn refactorer_returns_failure_when_no_diff_produced() {
    let task = make_task(TaskClass::Refactor, "refactor auth module");
    let result = RefactorerAgent::new()
        .run(
            task,
            make_agent_context("GROUNDED SYMBOLS: none\nNo refactoring needed."),
        )
        .await
        .unwrap();
    assert_eq!(result.outcome, Outcome::Failure);
    assert!(result.diff.is_none());
}

// ---------------------------------------------------------------------------
// VerifierAgent integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn verifier_returns_success_outcome_on_pass_verdict() {
    let verifier_response = "VERDICT: PASS\nREASON: All 12 tests passed; no regressions detected.";

    let task = make_task(TaskClass::Refactor, "verify refactoring passes all tests");
    let result = VerifierAgent::new()
        .run(task, make_agent_context(verifier_response))
        .await
        .unwrap();

    assert_eq!(result.outcome, Outcome::Success);
    let verdict: serde_json::Value =
        serde_json::from_str(&result.summary).expect("summary is valid JSON");
    assert_eq!(verdict["passed"], true);
    assert!(result.diff.is_none(), "verifier must never produce a diff");
}

#[tokio::test]
async fn verifier_returns_failure_outcome_on_fail_verdict() {
    let verifier_response = "VERDICT: FAIL\n\
        REASON: auth::login_works panicked.\n\
        FAILING_TESTS:\n\
        - auth::login_works: assertion failed at src/auth.rs:10\n\
        RECOMMENDED_FIX: Fix the JWT secret environment variable lookup.";

    let task = make_task(TaskClass::BugFix, "verify bug fix passes tests");
    let result = VerifierAgent::new()
        .run(task, make_agent_context(verifier_response))
        .await
        .unwrap();

    assert_eq!(result.outcome, Outcome::Failure);
    let verdict: serde_json::Value =
        serde_json::from_str(&result.summary).expect("summary is valid JSON");
    assert_eq!(verdict["passed"], false);
    assert_eq!(
        verdict["failing_test_summaries"].as_array().unwrap().len(),
        1
    );
}

// ---------------------------------------------------------------------------
// CommitterAgent integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn committer_produces_signed_commit_with_git_mcp_server() {
    let commit_hash = "abc1234def5678";
    let git_response = json!({ "hash": commit_hash, "ref": "refs/heads/main" });

    let mut mcp_router = McpRouter::new(["git.commit".to_owned()]);
    mcp_router.register_server(Arc::new(MockMcpServer {
        server_name: "git",
        response_value: git_response,
    }));

    let temp_dir = std::env::temp_dir().join(format!("yantra-integration-test-{}", TaskId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let memory =
        Arc::new(yantra_memory::MemoryService::new(&temp_dir.join("memory.sqlite")).unwrap());
    let context = AgentContext {
        router: make_mock_router(""),
        tools: mcp_router,
        session_id: SessionId::new(),
        upstream_results: Vec::new(),
        memory,
    };

    let signing_key = CommitSigningKey::generate().expect("key generation succeeds");
    let agent = CommitterAgent::new(signing_key);

    let task = make_task(
        TaskClass::NewFeature,
        "feat: add JWT rotation\n\nThe old token was never invalidated after rotation.",
    );
    let result = agent.run(task, context).await.unwrap();

    assert_eq!(result.outcome, Outcome::Success);
    assert!(
        result.summary.contains(commit_hash),
        "summary must include commit hash"
    );
    assert!(result.diff.is_none(), "committer must never produce a diff");
}

#[tokio::test]
async fn committer_succeeds_even_when_git_mcp_is_unavailable() {
    let mcp_router = McpRouter::new([]);

    let temp_dir = std::env::temp_dir().join(format!("yantra-integration-test-{}", TaskId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let memory =
        Arc::new(yantra_memory::MemoryService::new(&temp_dir.join("memory.sqlite")).unwrap());
    let context = AgentContext {
        router: make_mock_router(""),
        tools: mcp_router,
        session_id: SessionId::new(),
        upstream_results: Vec::new(),
        memory,
    };

    let signing_key = CommitSigningKey::generate().expect("key generation succeeds");
    let agent = CommitterAgent::new(signing_key);

    let task = make_task(TaskClass::Chore, "chore: update dependencies");
    let result = agent.run(task, context).await.unwrap();

    assert_eq!(result.outcome, Outcome::Success);
    assert!(result.summary.contains("git.commit unavailable"));
}

#[tokio::test]
async fn committer_returns_error_when_truth_token_missing() {
    let task_without_token = TaskNode {
        id: TaskId::new(),
        description: "commit this change".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: Vec::new(),
        assigned_agent: None,
        truth_token: None,
        parent_decision_id: None,
    };

    let signing_key = CommitSigningKey::generate().expect("key generation succeeds");
    let agent = CommitterAgent::new(signing_key);

    let error = agent
        .run(task_without_token, make_agent_context(""))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::MissingTruthToken { .. }));
}

// ---------------------------------------------------------------------------
// Acceptance test: Researcher → Coder → Tester → Verifier → Committer
// ---------------------------------------------------------------------------

fn make_chained_task(
    task_id: TaskId,
    task_class: TaskClass,
    description: &str,
    truth_token: TruthToken,
) -> TaskNode {
    let temp_directory = std::env::temp_dir().join(format!("yantra-empty-{task_id}"));
    let _create_result = std::fs::create_dir_all(&temp_directory);
    let description_with_root = format!(
        "project_root:{} {}",
        temp_directory.to_string_lossy(),
        description
    );
    TaskNode {
        id: task_id,
        description: description_with_root,
        status: TaskStatus::Pending,
        class: task_class,
        dependencies: Vec::new(),
        assigned_agent: None,
        truth_token: Some(truth_token),
        parent_decision_id: None,
    }
}

#[tokio::test]
async fn acceptance_chained_agent_flow_researcher_to_committer() {
    let task_id = TaskId::new();
    let task_class = TaskClass::NewFeature;
    let truth_token = make_truth_token(task_id, task_class);

    let session_id = SessionId::new();

    // --- Step 1: Researcher ---
    let researcher_response = "SUMMARY: Use existing token.rs module for JWT rotation.\n\
        CONFIDENCE: 0.85\n\
        FINDING: sign_jwt is in src/auth/token.rs | confidence: 0.9 | source: kg";

    let temp_dir = std::env::temp_dir().join(format!("yantra-integration-test-{}", TaskId::new()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let memory =
        Arc::new(yantra_memory::MemoryService::new(&temp_dir.join("memory.sqlite")).unwrap());

    let researcher_context = AgentContext {
        router: make_mock_router(researcher_response),
        tools: McpRouter::new([]),
        session_id,
        upstream_results: Vec::new(),
        memory: Arc::clone(&memory),
    };
    let researcher_task =
        make_chained_task(task_id, task_class, "add JWT rotation", truth_token.clone());
    let researcher_result = ResearcherAgent::new()
        .run(researcher_task, researcher_context)
        .await
        .expect("researcher must succeed");
    assert_eq!(researcher_result.outcome, Outcome::Success);
    assert!(researcher_result.diff.is_none());

    // --- Step 2: Coder ---
    let coder_response = "Adding rotate_jwt function.\n\n\
        // File: src/auth/token.rs\n\
        --- a/src/auth/token.rs\n\
        +++ b/src/auth/token.rs\n\
        @@ -1,3 +1,8 @@\n\
        +/// Rotates the JWT and invalidates the old token.\n\
        +pub fn rotate_jwt(old_token: &str) -> String {\n\
        +    format!(\"new-{old_token}\")\n\
        +}\n\
        + pub fn sign_jwt() {}";

    let coder_context = AgentContext {
        router: make_mock_router(coder_response),
        tools: make_crg_mcp_router(),
        session_id,
        upstream_results: vec![researcher_result.clone()],
        memory: Arc::clone(&memory),
    };
    let coder_task =
        make_chained_task(task_id, task_class, "add JWT rotation", truth_token.clone());
    let coder_result = CoderAgent::new()
        .run(coder_task, coder_context)
        .await
        .expect("coder must succeed");
    assert_eq!(coder_result.outcome, Outcome::Success);
    let coder_diff = coder_result
        .diff
        .as_ref()
        .expect("coder must produce a diff");
    assert_eq!(coder_diff.files[0].file_path, "src/auth/token.rs");

    // --- Step 3: Tester ---
    let tester_response = "Tests for rotate_jwt.\n\n\
        // File: src/auth/token_tests.rs\n\
        --- a/src/auth/token_tests.rs\n\
        +++ b/src/auth/token_tests.rs\n\
        @@ -0,0 +1,5 @@\n\
        +#[test]\n\
        +fn rotate_jwt_produces_new_token() {\n\
        +    assert!(rotate_jwt(\"old\").starts_with(\"new-\"));\n\
        +}";

    let tester_context = AgentContext {
        router: make_mock_router(tester_response),
        tools: make_crg_mcp_router(),
        session_id,
        upstream_results: vec![coder_result.clone()],
        memory: Arc::clone(&memory),
    };
    let tester_task = make_chained_task(
        task_id,
        task_class,
        "add tests for rotate_jwt",
        truth_token.clone(),
    );
    let tester_result = TesterAgent::new()
        .run(tester_task, tester_context)
        .await
        .expect("tester must succeed");
    assert_eq!(tester_result.outcome, Outcome::Success);
    assert!(tester_result.diff.is_some());

    // --- Step 4: Verifier ---
    let verifier_response =
        "VERDICT: PASS\nREASON: All 5 tests passed; rotate_jwt is correctly covered.";

    let verifier_context = AgentContext {
        router: make_mock_router(verifier_response),
        tools: McpRouter::new([]),
        session_id,
        upstream_results: vec![tester_result.clone()],
        memory: Arc::clone(&memory),
    };
    let verifier_task = make_chained_task(
        task_id,
        task_class,
        "verify rotate_jwt tests pass",
        truth_token.clone(),
    );
    let verifier_result = VerifierAgent::new()
        .run(verifier_task, verifier_context)
        .await
        .expect("verifier must succeed");
    assert_eq!(verifier_result.outcome, Outcome::Success);

    // --- Step 5: Committer ---
    let commit_hash = "deadbeef1234";
    let git_response = json!({ "hash": commit_hash });

    let mut commit_mcp_router = McpRouter::new(["git.commit".to_owned()]);
    commit_mcp_router.register_server(Arc::new(MockMcpServer {
        server_name: "git",
        response_value: git_response,
    }));

    let committer_context = AgentContext {
        router: make_mock_router(""),
        tools: commit_mcp_router,
        session_id,
        upstream_results: vec![verifier_result.clone()],
        memory: Arc::clone(&memory),
    };
    let signing_key = CommitSigningKey::generate().expect("key generation succeeds");
    let committer_task = make_chained_task(
        task_id,
        task_class,
        "feat: add JWT rotation\n\nThe old token was never invalidated.",
        truth_token,
    );
    let committer_result = CommitterAgent::new(signing_key)
        .run(committer_task, committer_context)
        .await
        .expect("committer must succeed");

    assert_eq!(committer_result.outcome, Outcome::Success);
    assert!(
        committer_result.summary.contains(commit_hash),
        "final commit summary must include the commit hash"
    );
}

#[tokio::test]
async fn red_team_agent_verdict_pass_outcome() {
    use yantra_core::DecisionId;
    let mock_response =
        "VERDICT: PASS\nREASON: The changes look clean and have no major security concerns.";
    let context = make_agent_context(mock_response);

    let task = make_task(TaskClass::NewFeature, "add jwt rotation");
    let coder_result = TaskResult {
        task_id: task.id,
        outcome: Outcome::Success,
        diff: Some(Diff {
            files: vec![FileDiff {
                file_path: "src/main.rs".to_owned(),
                unified_diff: "+++ src/main.rs\n+fn main() {}\n".to_owned(),
            }],
        }),
        summary: "wrote main".to_owned(),
        decision_id: DecisionId::new(),
    };

    let mut context_with_upstream = context;
    context_with_upstream.upstream_results.push(coder_result);

    let result = RedTeamAgent::new()
        .run(task, context_with_upstream)
        .await
        .unwrap();
    assert_eq!(result.outcome, Outcome::Success);
    assert!(result.summary.contains("passed\":true"));
}
