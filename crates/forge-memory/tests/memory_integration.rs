//! Integration tests for `forge-memory` public API.
//!
//! Exercises the full multi-tier memory pipeline end-to-end using only
//! types and functions exported from the crate root. Each test owns its
//! own SQLite file under `std::env::temp_dir()` so they run in parallel
//! without interference.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use yantra_core::TaskId;
use yantra_memory::{
    assemble_prompt, ArchivalStore, ConversationTurn, HeartbeatHandle, MemoryShard,
    MistakeLibraryStore, MistakeRecord, RecallStore, ShardType, TemporalKnowledgeGraph, TruthHash,
    TruthVault,
};

fn temp_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("yantra-mem-integration-{label}-{}", TaskId::new()))
        .join("memory.sqlite")
}

// ---------------------------------------------------------------------------
// Recall tier: write turn → end session → summary generated → can query
// ---------------------------------------------------------------------------

#[test]
fn recall_write_turn_then_summarize_then_query() {
    let db_path = temp_path("recall");
    let recall_store = RecallStore::new(&db_path).expect("store created");

    let session_id = TaskId::new().to_string();

    let turn_a = ConversationTurn {
        id: TaskId::new().to_string(),
        session_id: session_id.clone(),
        timestamp: Utc::now(),
        role: "user".to_owned(),
        content: "implement JWT token rotation".to_owned(),
        summary: None,
        tokens: None,
    };
    let turn_b = ConversationTurn {
        id: TaskId::new().to_string(),
        session_id: session_id.clone(),
        timestamp: Utc::now(),
        role: "assistant".to_owned(),
        content: "Rotating tokens via refresh endpoint".to_owned(),
        summary: None,
        tokens: None,
    };

    recall_store.record_turn(&turn_a).expect("turn A stored");
    recall_store.record_turn(&turn_b).expect("turn B stored");

    recall_store
        .summarize_session(
            &session_id,
            "Implemented JWT rotation",
            Some("rotation endpoint added"),
            None,
            Some("use RS256 over HS256"),
        )
        .expect("session summarized");

    let turns = recall_store
        .query_session(&session_id)
        .expect("query succeeds");
    assert_eq!(turns.len(), 2, "both turns must be retrievable");
    assert_eq!(turns[0].role, "user");
    assert_eq!(turns[1].role, "assistant");

    let summary = recall_store
        .get_session_summary(&session_id)
        .expect("query succeeds")
        .expect("summary must exist");
    assert!(summary.summary.contains("JWT"));
    assert_eq!(
        summary.accomplished.as_deref(),
        Some("rotation endpoint added")
    );
    assert_eq!(
        summary.decisions_made.as_deref(),
        Some("use RS256 over HS256")
    );
}

// ---------------------------------------------------------------------------
// Archival tier: index shards → search returns most relevant
// ---------------------------------------------------------------------------

#[test]
fn archival_index_and_search_returns_relevant_shard() {
    let archival_store = ArchivalStore::new(&temp_path("archival")).expect("store created");

    let auth_shard = MemoryShard {
        id: TaskId::new().to_string(),
        shard_type: ShardType::CodeChunk,
        source_ref: Some("src/auth.rs".to_owned()),
        content: "jwt token authentication verify credentials login".to_owned(),
        created_at: Utc::now(),
    };
    let unrelated_shard = MemoryShard {
        id: TaskId::new().to_string(),
        shard_type: ShardType::ResearchMemo,
        source_ref: None,
        content: "database migration schema table column index".to_owned(),
        created_at: Utc::now(),
    };

    archival_store
        .index(&auth_shard)
        .expect("auth shard indexed");
    archival_store
        .index(&unrelated_shard)
        .expect("unrelated shard indexed");

    let results = archival_store
        .search("jwt token authentication", 1)
        .expect("search succeeds");

    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("jwt"),
        "top result should be the auth shard"
    );
}

// ---------------------------------------------------------------------------
// Temporal Knowledge Graph: assert facts → query current → check history
// ---------------------------------------------------------------------------

#[test]
fn tkg_assert_and_query_current_and_historical() {
    let tkg = TemporalKnowledgeGraph::new(&temp_path("tkg")).expect("TKG created");

    let time_a = Utc::now();
    let time_b = time_a + ChronoDuration::seconds(5);

    tkg.assert_fact("src/auth.rs", "status", "stable", time_a)
        .expect("first fact asserted");
    tkg.assert_fact("src/auth.rs", "status", "breaking", time_b)
        .expect("second fact asserted");

    let current_facts = tkg.query_current("src/auth.rs").expect("query succeeds");
    assert_eq!(current_facts.len(), 1, "only the latest fact is current");
    assert_eq!(current_facts[0].object, "breaking");

    let historical_query_time = time_a + ChronoDuration::seconds(2);
    let historical_facts = tkg
        .query_at("src/auth.rs", historical_query_time)
        .expect("historical query succeeds");
    assert_eq!(historical_facts.len(), 1);
    assert_eq!(
        historical_facts[0].object, "stable",
        "at t=2s the first fact should be active"
    );
}

// ---------------------------------------------------------------------------
// Truth Vault: store YAML → retrieve by task ID and content hash
// ---------------------------------------------------------------------------

#[test]
fn truth_vault_store_and_retrieve_by_task_id_and_hash() {
    let vault = TruthVault::new(&temp_path("vault")).expect("vault created");
    let task_id = TaskId::new();
    let yaml_content = "task_id: abc123\ndescription: add rate limiting middleware\n";

    let truth_hash: TruthHash = vault.store(task_id, yaml_content).expect("stored");
    assert!(!truth_hash.as_str().is_empty(), "hash must be non-empty");

    let by_id = vault
        .get_by_task_id(task_id)
        .expect("query succeeds")
        .expect("content present");
    assert_eq!(by_id, yaml_content);

    let by_hash = vault
        .get_by_hash(&truth_hash)
        .expect("query succeeds")
        .expect("content present");
    assert_eq!(by_hash, yaml_content);
}

// ---------------------------------------------------------------------------
// Mistake Library: record failure → query → reference count increments
// ---------------------------------------------------------------------------

#[test]
fn mistake_library_record_and_query_with_reference_counting() {
    use yantra_core::TaskClass;

    let library = MistakeLibraryStore::new(&temp_path("mistakes")).expect("store created");

    let record = MistakeRecord::new(
        TaskClass::BugFix,
        "off-by-one in pagination cursor",
        "loop used 0..n instead of 0..n-1",
        "verify boundary conditions with a unit test that checks count == expected",
    );
    library.record_failure(&record).expect("record stored");

    let first_query = library
        .query_for_task_class(TaskClass::BugFix)
        .expect("first query");
    assert_eq!(first_query.len(), 1);
    assert!(first_query[0].prevention_rule.contains("boundary"));
    assert_eq!(
        first_query[0].times_referenced, 0,
        "times_referenced before this query should be 0"
    );

    let second_query = library
        .query_for_task_class(TaskClass::BugFix)
        .expect("second query");
    assert_eq!(
        second_query[0].times_referenced, 1,
        "counter should reflect the previous query"
    );
}

// ---------------------------------------------------------------------------
// Working memory: assemble_prompt respects token budget and drop order
// ---------------------------------------------------------------------------

#[test]
fn working_memory_assembles_prompt_and_drops_low_priority_when_over_budget() {
    use yantra_memory::AssembledPrompt;

    let large_shard = MemoryShard {
        id: TaskId::new().to_string(),
        shard_type: ShardType::ResearchMemo,
        source_ref: None,
        content: "x".repeat(8_000),
        created_at: Utc::now(),
    };

    let result: AssembledPrompt = assemble_prompt(
        "You are a Rust systems engineer.",
        "implement rate limiting middleware",
        &[],
        &[large_shard],
        "",
        &[],
        50,
    );

    assert!(
        result.token_count <= 50,
        "assembled prompt must be within token budget"
    );
    assert!(
        result.sections_dropped.contains(&"memory_shard"),
        "memory shard should be dropped when over budget"
    );
    assert!(
        result.text.contains("SYSTEM PERSONA"),
        "persona is never dropped"
    );
    assert!(result.text.contains("TASK"), "task is never dropped");
}

// ---------------------------------------------------------------------------
// Heartbeat: starts, ticks, and stops cleanly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn heartbeat_runs_and_stops_cleanly() {
    let recall_store = Arc::new(RecallStore::new(&temp_path("heartbeat")).expect("store created"));

    let session_id = TaskId::new().to_string();
    let turn = ConversationTurn {
        id: TaskId::new().to_string(),
        session_id: session_id.clone(),
        timestamp: Utc::now(),
        role: "user".to_owned(),
        content: "refactor the database layer".to_owned(),
        summary: None,
        tokens: None,
    };
    recall_store.record_turn(&turn).expect("turn recorded");

    let mut handle: HeartbeatHandle =
        yantra_memory::start_heartbeat(Arc::clone(&recall_store), Duration::from_millis(20));

    tokio::time::sleep(Duration::from_millis(80)).await;
    handle.stop();

    let mut summary = None;
    for _ in 0..10 {
        summary = recall_store
            .get_session_summary(&session_id)
            .expect("query succeeds");
        if summary.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert!(
        summary.is_some(),
        "heartbeat should have auto-summarized the session"
    );
}
