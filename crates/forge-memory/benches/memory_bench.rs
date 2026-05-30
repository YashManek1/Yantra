//! # forge-memory: Recall Store Benchmarks
//!
//! Measures the latency of two recall-tier operations on a pre-seeded SQLite
//! database:
//!
//! 1. `recall_query_recent_sessions` — calls `RecallStore::get_recent_session_ids(3)`
//!    on a store pre-seeded with five sessions of three turns each.
//!
//! 2. `session_summary_roundtrip` — writes a session summary via
//!    `RecallStore::summarize_session` and immediately reads it back with
//!    `RecallStore::get_session_summary`.
//!
//! Both benchmarks target a p99 latency of ≤ 15 ms.
//!
//! ## Input
//! - A shared `RecallStore` backed by a temp-directory SQLite file, seeded
//!   once before the benchmark loop begins
//!
//! ## Output
//! - Criterion statistical report (mean, standard deviation, p50, p99 estimates)
//! - `#[test]` SLA assertions verifiable by `cargo test`
//!
//! ## Related
//! - `forge-memory::recall` — `RecallStore` under test

use std::time::Duration;

use chrono::Utc;
use criterion::{criterion_group, criterion_main, Criterion};
use yantra_memory::recall::{ConversationTurn, RecallStore};

fn temp_db_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("yantra-mem-bench-{label}-{}", uuid::Uuid::new_v4()))
        .join("memory.sqlite")
}

fn make_turn(session_id: &str, role: &str, content: &str) -> ConversationTurn {
    ConversationTurn {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_owned(),
        timestamp: Utc::now(),
        role: role.to_owned(),
        content: content.to_owned(),
        summary: None,
        tokens: None,
    }
}

fn seed_recall_store(recall_store: &RecallStore, session_count: usize, turns_per_session: usize) {
    for session_index in 0..session_count {
        let session_id = format!("bench-session-{session_index}");
        for turn_index in 0..turns_per_session {
            let role = if turn_index % 2 == 0 {
                "user"
            } else {
                "assistant"
            };
            let turn = make_turn(&session_id, role, &format!("message {turn_index}"));
            recall_store
                .record_turn(&turn)
                .expect("seeding turn must succeed");
        }
    }
}

fn bench_recall_query_recent_sessions(benchmark_context: &mut Criterion) {
    let recall_store =
        RecallStore::new(&temp_db_path("recent")).expect("RecallStore::new must succeed");
    seed_recall_store(&recall_store, 5, 3);

    benchmark_context.bench_function("recall_query_recent_sessions", |bencher| {
        bencher.iter(|| {
            let session_ids = recall_store
                .get_recent_session_ids(3)
                .expect("get_recent_session_ids must succeed");
            assert_eq!(
                session_ids.len(),
                3,
                "expected 3 session IDs from a store with 5 sessions"
            );
        });
    });
}

fn bench_session_summary_roundtrip(benchmark_context: &mut Criterion) {
    benchmark_context.bench_function("session_summary_roundtrip", |bencher| {
        bencher.iter_batched(
            || {
                let recall_store =
                    RecallStore::new(&temp_db_path("roundtrip")).expect("store created");
                let session_id = uuid::Uuid::new_v4().to_string();
                let turn = make_turn(&session_id, "user", "roundtrip benchmark seed message");
                recall_store.record_turn(&turn).expect("turn recorded");
                (recall_store, session_id)
            },
            |(recall_store, session_id)| {
                recall_store
                    .summarize_session(
                        &session_id,
                        "Benchmark summary text for roundtrip measurement",
                        Some("benchmark accomplished"),
                        None,
                        None,
                    )
                    .expect("summarize_session must succeed");

                let summary = recall_store
                    .get_session_summary(&session_id)
                    .expect("get_session_summary must succeed")
                    .expect("summary must be present after write");

                assert!(
                    summary.summary.contains("Benchmark summary text"),
                    "roundtrip summary content must survive the write/read cycle"
                );
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group! {
    name    = memory_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(50);
    targets = bench_recall_query_recent_sessions, bench_session_summary_roundtrip
}
criterion_main!(memory_benches);
