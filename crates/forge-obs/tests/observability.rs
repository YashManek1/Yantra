use chrono::Utc;
use proptest::prelude::*;
use rusqlite::Connection;
use yantra_core::{
    AgentKind, DecisionId, ErrorRecord, ModelCapability, ModelId, Outcome, SessionId, Span, SpanId,
    TaskId,
};
use yantra_obs::{
    causal_chain, descendants, query_session_spans, query_task_spans, record_decision, record_span,
    AnomalyDetector, AnomalySample, CostGauge, CostStatus, CostThresholds, DecisionRecord,
};

fn sample_span(session_id: SessionId, task_id: TaskId) -> Span {
    Span {
        span_id: SpanId::new(),
        parent_id: None,
        session_id,
        task_id: Some(task_id),
        truth_token: None,
        agent: Some(AgentKind::Coder),
        model: Some(ModelId::new("ollama:qwen2.5-coder:7b").unwrap()),
        tokens_in: 1_000,
        tokens_out: 500,
        cost_usd: 0.012,
        duration_ms: 42,
        started_at: Utc::now(),
        outcome: Outcome::Success,
        error: Some(ErrorRecord {
            kind: "None".to_string(),
            message: "no error".to_string(),
            source_span: None,
        }),
    }
}

#[test]
fn span_insertion_and_retrieval_round_trip() {
    let database = Connection::open_in_memory().unwrap();
    let session_id = SessionId::new();
    let task_id = TaskId::new();
    let span = sample_span(session_id, task_id);

    record_span(&database, &span).unwrap();

    let session_spans = query_session_spans(&database, session_id).unwrap();
    let task_spans = query_task_spans(&database, task_id).unwrap();

    assert_eq!(session_spans, vec![span.clone()]);
    assert_eq!(task_spans, vec![span]);
}

#[test]
fn causal_chain_traverses_five_levels() {
    let database = Connection::open_in_memory().unwrap();
    let session_id = SessionId::new();
    let mut parent_decision_id = None;
    let mut decision_ids = Vec::new();

    for decision_index in 0..5 {
        let decision_id = DecisionId::new();
        let decision = DecisionRecord {
            id: decision_id,
            timestamp: Utc::now(),
            session_id,
            parent_decision_id,
            action_type: format!("step-{decision_index}"),
            reasoning: format!("reasoning {decision_index}"),
            alternatives_considered: vec![format!("alternative {decision_index}")],
            truth_token: None,
            git_hash: None,
            agent: AgentKind::ConflictResolver,
        };
        record_decision(&database, &decision).unwrap();
        parent_decision_id = Some(decision_id);
        decision_ids.push(decision_id);
    }

    let chain = causal_chain(&database, *decision_ids.last().unwrap()).unwrap();
    let descendant_records = descendants(&database, decision_ids[0]).unwrap();

    assert_eq!(chain.len(), 5);
    assert_eq!(chain[0].id, decision_ids[4]);
    assert_eq!(chain[4].id, decision_ids[0]);
    assert_eq!(descendant_records.len(), 4);
}

#[test]
fn cost_gauge_fires_thresholds() {
    let gauge = CostGauge::new(CostThresholds {
        soft: 1.0,
        hard: 2.0,
        kill: 3.0,
    });
    let model_capability = ModelCapability {
        context_limit: 4_096,
        supports_tools: false,
        cost_per_1k_in: 1.0,
        cost_per_1k_out: 1.0,
    };

    assert_eq!(gauge.check_threshold(), CostStatus::Ok);
    gauge.record_call(500, 500, &model_capability);
    assert_eq!(gauge.check_threshold(), CostStatus::Warn);
    gauge.record_call(500, 500, &model_capability);
    assert_eq!(gauge.check_threshold(), CostStatus::Pause);
    gauge.record_call(500, 500, &model_capability);
    assert_eq!(gauge.check_threshold(), CostStatus::Kill);
}

proptest! {
    #[test]
    fn anomaly_detector_emits_no_false_positives_on_normal_data(
        error_rate in 0.0_f64..0.2,
        latency_ms in 50.0_f64..200.0,
        tokens_per_minute in 100.0_f64..1_000.0,
    ) {
        let mut detector = AnomalyDetector::new();
        for sample_index in 0..150 {
            let sample = AnomalySample {
                sampled_at: Utc::now(),
                error_rate,
                latency_ms,
                tokens_per_minute,
            };
            let anomaly_event = detector.observe(sample);
            prop_assert!(anomaly_event.is_none(), "sample {sample_index} unexpectedly emitted anomaly");
        }
    }
}
