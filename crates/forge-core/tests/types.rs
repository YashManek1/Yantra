use chrono::Utc;
use proptest::prelude::*;
use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;
use yantra_core::{
    AgentCapability, AgentKind, AgentSpec, DecisionId, ErrorRecord, ModelCapability, ModelId,
    ModelTier, Outcome, ProjectRoot, SessionId, Span, SpanId, Strictness, SymbolId, TaskClass,
    TaskId, TaskNode, TaskStatus, TruthToken, VerifyingKey,
};

fn json_round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let serialized_value = serde_json::to_string(value).unwrap();
    serde_json::from_str(&serialized_value).unwrap()
}

fn signing_key_pair() -> Ed25519KeyPair {
    let random_generator = SystemRandom::new();
    let key_pair_document = Ed25519KeyPair::generate_pkcs8(&random_generator).unwrap();
    Ed25519KeyPair::from_pkcs8(key_pair_document.as_ref()).unwrap()
}

#[test]
fn identifier_types_round_trip_through_serde() {
    let task_id = TaskId::new();
    let session_id = SessionId::new();
    let decision_id = DecisionId::new();
    let span_id = SpanId::new();
    let symbol_id = SymbolId::from_parts("src/lib.rs", "run_task", "function");

    assert_eq!(task_id, json_round_trip(&task_id));
    assert_eq!(session_id, json_round_trip(&session_id));
    assert_eq!(decision_id, json_round_trip(&decision_id));
    assert_eq!(span_id, json_round_trip(&span_id));
    assert_eq!(symbol_id, json_round_trip(&symbol_id));
}

#[test]
fn agent_types_round_trip_through_serde() {
    let agent_spec = AgentSpec {
        kind: AgentKind::Coder,
        capabilities: vec![
            AgentCapability::ReadFiles,
            AgentCapability::WriteFiles,
            AgentCapability::RunShell,
        ],
        permitted_tools: vec!["crg.subgraph".to_string(), "fs.read".to_string()],
    };

    assert_eq!(AgentKind::RedTeam, json_round_trip(&AgentKind::RedTeam));
    assert_eq!(
        AgentCapability::SacredWrite,
        json_round_trip(&AgentCapability::SacredWrite)
    );
    assert_eq!(agent_spec, json_round_trip(&agent_spec));
}

#[test]
fn model_types_round_trip_through_serde() {
    let model_id = ModelId::new("ollama:qwen2.5-coder:7b").unwrap();
    let model_capability = ModelCapability {
        context_limit: 32_768,
        supports_tools: true,
        cost_per_1k_in: 0.0,
        cost_per_1k_out: 0.0,
    };

    assert_eq!(ModelTier::Tier0, json_round_trip(&ModelTier::Tier0));
    assert_eq!(model_id, json_round_trip(&model_id));
    assert_eq!(model_capability, json_round_trip(&model_capability));
}

#[test]
fn task_and_truth_types_round_trip_through_serde() {
    let signing_key_pair = signing_key_pair();
    let truth_token = TruthToken::new(
        TaskId::new(),
        TaskClass::BugFix,
        Strictness::Light,
        &signing_key_pair,
    )
    .unwrap();
    let task_node = TaskNode {
        id: truth_token.task_id,
        description: "fix token verification".to_string(),
        status: TaskStatus::Pending,
        class: TaskClass::BugFix,
        dependencies: vec![TaskId::new()],
        assigned_agent: Some(AgentKind::Coder),
        truth_token: Some(truth_token.clone()),
        parent_decision_id: Some(DecisionId::new()),
    };
    let verifying_key = VerifyingKey::from_key_pair(&signing_key_pair);

    assert_eq!(TaskClass::Migration, json_round_trip(&TaskClass::Migration));
    assert_eq!(TaskStatus::Deferred, json_round_trip(&TaskStatus::Deferred));
    assert_eq!(Strictness::Strict, json_round_trip(&Strictness::Strict));
    assert_eq!(truth_token, json_round_trip(&truth_token));
    assert_eq!(task_node, json_round_trip(&task_node));
    assert_eq!(verifying_key, json_round_trip(&verifying_key));
}

#[test]
fn span_types_round_trip_through_serde() {
    let model_id = ModelId::new("openrouter:qwen/qwen3-coder:free").unwrap();
    let error_record = ErrorRecord {
        kind: "VerifierFailure".to_string(),
        message: "static analysis failed".to_string(),
        source_span: Some(SpanId::new()),
    };
    let span = Span {
        span_id: SpanId::new(),
        parent_id: Some(SpanId::new()),
        session_id: SessionId::new(),
        task_id: Some(TaskId::new()),
        truth_token: None,
        agent: Some(AgentKind::Tester),
        model: Some(model_id),
        tokens_in: 120,
        tokens_out: 44,
        cost_usd: 0.001,
        duration_ms: 42,
        started_at: Utc::now(),
        outcome: Outcome::Failure,
        error: Some(error_record),
    };

    assert_eq!(
        Outcome::BudgetExceeded,
        json_round_trip(&Outcome::BudgetExceeded)
    );
    assert_eq!(span, json_round_trip(&span));
}

#[test]
fn project_root_round_trips_through_serde() {
    let project_root = ProjectRoot::new(std::env::current_dir().unwrap()).unwrap();

    assert_eq!(project_root, json_round_trip(&project_root));
}

proptest! {
    #[test]
    fn symbol_id_from_same_inputs_always_equal(
        file_path in "\\PC*",
        symbol_name in "\\PC*",
        symbol_kind in "\\PC*",
    ) {
        let first_symbol_id = SymbolId::from_parts(&file_path, &symbol_name, &symbol_kind);
        let second_symbol_id = SymbolId::from_parts(&file_path, &symbol_name, &symbol_kind);

        prop_assert_eq!(first_symbol_id, second_symbol_id);
    }
}
