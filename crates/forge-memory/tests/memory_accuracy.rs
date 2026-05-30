//! # forge-memory integration tests: Recall Store Accuracy
//!
//! Verifies two deterministic correctness properties of `RecallStore`:
//!
//! 1. `recall_ordering_by_recency_is_correct` — seeds five sessions with
//!    staggered timestamps and asserts that `get_recent_session_ids(3)` returns
//!    the three most recently active sessions in descending recency order.
//!
//! 2. `session_summary_roundtrip_is_lossless` — writes a `SessionSummary` via
//!    `summarize_session` and asserts that every field survives the roundtrip
//!    through `get_session_summary` without mutation or truncation.
//!
//! Both properties are purely deterministic; the SQLite engine guarantees stable
//! ORDER BY results and exact TEXT storage.
//!
//! ## Input
//! - Ephemeral temp-directory SQLite databases constructed per test
//! - Known session IDs and content strings for field-level assertion
//!
//! ## Output
//! - Pass/fail assertions; any failure indicates a regression in recall semantics
//!
//! ## Related
//! - `forge-memory::recall` — `RecallStore` under test

use std::time::Duration;

use chrono::Utc;
use yantra_memory::recall::{ConversationTurn, RecallStore};

fn temp_db_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("yantra-mem-acc-{label}-{}", uuid::Uuid::new_v4()))
        .join("memory.sqlite")
}

fn make_turn_at_offset(
    session_id: &str,
    role: &str,
    content: &str,
    offset_secs: i64,
) -> ConversationTurn {
    let timestamp = Utc::now() + chrono::Duration::seconds(offset_secs);
    ConversationTurn {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_owned(),
        timestamp,
        role: role.to_owned(),
        content: content.to_owned(),
        summary: None,
        tokens: None,
    }
}

#[test]
fn recall_ordering_by_recency_is_correct() {
    let recall_store =
        RecallStore::new(&temp_db_path("ordering")).expect("RecallStore::new must succeed");

    let session_id_oldest = "session-oldest".to_owned();
    let session_id_second = "session-second".to_owned();
    let session_id_third = "session-third".to_owned();
    let session_id_fourth = "session-fourth".to_owned();
    let session_id_newest = "session-newest".to_owned();

    let session_timestamps: &[(&str, i64)] = &[
        (&session_id_oldest, -400),
        (&session_id_second, -300),
        (&session_id_third, -200),
        (&session_id_fourth, -100),
        (&session_id_newest, 0),
    ];

    for (session_id, base_offset_secs) in session_timestamps {
        for turn_index in 0..3_i64 {
            let turn_offset = base_offset_secs + turn_index;
            let role = if turn_index % 2 == 0 {
                "user"
            } else {
                "assistant"
            };
            let turn = make_turn_at_offset(
                session_id,
                role,
                &format!("content for turn {turn_index}"),
                turn_offset,
            );
            recall_store
                .record_turn(&turn)
                .expect("record_turn must succeed");
        }
    }

    let recent_session_ids = recall_store
        .get_recent_session_ids(3)
        .expect("get_recent_session_ids must succeed");

    assert_eq!(
        recent_session_ids.len(),
        3,
        "get_recent_session_ids(3) must return exactly 3 results when 5 sessions exist"
    );

    assert_eq!(
        recent_session_ids[0], session_id_newest,
        "first result must be the most recently active session"
    );
    assert_eq!(
        recent_session_ids[1], session_id_fourth,
        "second result must be the second most recently active session"
    );
    assert_eq!(
        recent_session_ids[2], session_id_third,
        "third result must be the third most recently active session"
    );

    assert!(
        !recent_session_ids.contains(&session_id_oldest),
        "oldest session must not appear when only the top 3 are requested"
    );
    assert!(
        !recent_session_ids.contains(&session_id_second),
        "second-oldest session must not appear when only the top 3 are requested"
    );
}

#[test]
fn session_summary_roundtrip_is_lossless() {
    let recall_store =
        RecallStore::new(&temp_db_path("summary")).expect("RecallStore::new must succeed");

    let session_id = "accuracy-test-session-001".to_owned();

    let seed_turn = make_turn_at_offset(&session_id, "user", "initial user message", 0);
    recall_store
        .record_turn(&seed_turn)
        .expect("record_turn must succeed");

    let expected_summary_text = "Implemented OAuth2 PKCE flow with RS256 token signing.";
    let expected_accomplished = "OAuth2 login endpoint live and tested";
    let expected_unresolved = "token refresh loop not yet integrated";
    let expected_decisions_made = "use RS256 over HS256 for asymmetric key distribution";

    recall_store
        .summarize_session(
            &session_id,
            expected_summary_text,
            Some(expected_accomplished),
            Some(expected_unresolved),
            Some(expected_decisions_made),
        )
        .expect("summarize_session must succeed");

    let retrieved_summary = recall_store
        .get_session_summary(&session_id)
        .expect("get_session_summary must succeed")
        .expect("summary must be present after write");

    assert_eq!(
        retrieved_summary.session_id, session_id,
        "session_id must survive the roundtrip unchanged"
    );
    assert_eq!(
        retrieved_summary.summary, expected_summary_text,
        "summary text must survive the roundtrip unchanged"
    );
    assert_eq!(
        retrieved_summary.accomplished.as_deref(),
        Some(expected_accomplished),
        "accomplished field must survive the roundtrip unchanged"
    );
    assert_eq!(
        retrieved_summary.unresolved.as_deref(),
        Some(expected_unresolved),
        "unresolved field must survive the roundtrip unchanged"
    );
    assert_eq!(
        retrieved_summary.decisions_made.as_deref(),
        Some(expected_decisions_made),
        "decisions_made field must survive the roundtrip unchanged"
    );

    std::thread::sleep(Duration::from_millis(1));

    let second_retrieval = recall_store
        .get_session_summary(&session_id)
        .expect("second get_session_summary must succeed")
        .expect("summary must still be present on second read");

    assert_eq!(
        second_retrieval.summary, expected_summary_text,
        "summary text must be identical on repeated reads"
    );
}

#[test]
fn memory_recall_p99_meets_15ms_sla() {
    use std::time::{Duration, Instant};

    let recall_store =
        RecallStore::new(&temp_db_path("sla")).expect("RecallStore::new must succeed");

    for session_index in 0..5 {
        let session_id = format!("sla-session-{session_index}");
        for turn_index in 0..3_i64 {
            let role = if turn_index % 2 == 0 {
                "user"
            } else {
                "assistant"
            };
            let turn = make_turn_at_offset(
                &session_id,
                role,
                &format!("sla message {turn_index}"),
                session_index * 10 + turn_index,
            );
            recall_store
                .record_turn(&turn)
                .expect("seed turn must succeed");
        }
    }

    for _warmup_index in 0..10 {
        recall_store
            .get_recent_session_ids(3)
            .expect("warmup query must succeed");
    }

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _ in 0..100 {
        let iteration_start_time = Instant::now();
        let session_ids = recall_store
            .get_recent_session_ids(3)
            .expect("get_recent_session_ids must succeed");
        assert_eq!(session_ids.len(), 3);
        iteration_durations.push(iteration_start_time.elapsed());
    }

    iteration_durations.sort();

    let p99_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 iterations must produce 100 duration samples");

    assert!(
        p99_duration.as_millis() < 15,
        "recall query p99 latency exceeded 15ms target: {p99_duration:?}",
    );
}
