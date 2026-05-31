//! # forge-stvp: STVP Token Issue + Verify Benchmark
//!
//! Measures the end-to-end latency of issuing an Ed25519-signed `TruthToken`
//! for a `SourceTruth` and immediately verifying the signature against the
//! stored public key. The benchmark exercises the hot path that the
//! orchestrator traverses on every task dispatch.
//!
//! The p99 SLA target is ≤ 5 ms; typical measurements on this branch are
//! well below 1 ms because Ed25519 signing and SHA-256 hashing are very fast
//! operations.
//!
//! ## Input
//! - A temporary `.yantra` directory written to `$TMPDIR` for key storage
//! - A synthetic `SourceTruth` with a BugFix task class and Light strictness
//!
//! ## Output
//! - Criterion statistical report: mean, standard deviation, p50, p99 estimates
//!
//! ## Related
//! - `forge-stvp::token`        — `issue_token` and `verify_token` under test
//! - `forge-stvp::source_truth` — `SourceTruth` consumed by `issue_token`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use criterion::{criterion_group, criterion_main, Criterion};
use yantra_core::{Strictness, TaskClass, TaskId};
use yantra_stvp::source_truth::SourceTruth;
use yantra_stvp::token::{issue_token, verify_token, SigningKey};

fn make_temp_yantra_dir() -> PathBuf {
    let yantra_dir = std::env::temp_dir().join(format!("yantra-stvp-bench-{}", TaskId::new()));
    std::fs::create_dir_all(&yantra_dir).expect("creating temp yantra directory must succeed");
    yantra_dir
}

fn make_sample_source_truth() -> SourceTruth {
    SourceTruth {
        task_id: TaskId::new(),
        task_class: TaskClass::BugFix,
        strictness: Strictness::Light,
        description: "fix null pointer dereference in session handler".to_owned(),
        created_at: Utc::now(),
        answers: BTreeMap::new(),
        augmented_question_ids: Vec::new(),
    }
}

fn bench_token_issue_and_verify(benchmark_context: &mut Criterion) {
    let yantra_dir = make_temp_yantra_dir();
    let signing_key =
        SigningKey::load_or_generate(&yantra_dir).expect("signing key must be generated");

    let source_truth = make_sample_source_truth();

    for _warmup_index in 0..5 {
        let warmup_token =
            issue_token(&source_truth, &signing_key).expect("warm-up token issue must succeed");
        let _warmup_valid =
            verify_token(&warmup_token, &yantra_dir).expect("warm-up verify must succeed");
    }

    benchmark_context.bench_function("token_issue_and_verify", |bencher| {
        bencher.iter(|| {
            let token = issue_token(&source_truth, &signing_key)
                .expect("benchmark token issue must succeed");
            let _is_valid =
                verify_token(&token, &yantra_dir).expect("benchmark token verify must succeed");
        });
    });
}

criterion_group! {
    name    = stvp_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100);
    targets = bench_token_issue_and_verify
}
criterion_main!(stvp_benches);

#[test]
fn stvp_token_p99_meets_5ms_sla() {
    use std::time::Instant;

    let yantra_dir = make_temp_yantra_dir();
    let signing_key =
        SigningKey::load_or_generate(&yantra_dir).expect("signing key must be generated");
    let source_truth = make_sample_source_truth();

    for _warmup_index in 0..5 {
        let warmup_token =
            issue_token(&source_truth, &signing_key).expect("warm-up token issue must succeed");
        let _warmup_valid =
            verify_token(&warmup_token, &yantra_dir).expect("warm-up verify must succeed");
    }

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _iteration_index in 0..100 {
        let iteration_start = Instant::now();
        let token =
            issue_token(&source_truth, &signing_key).expect("sla-test token issue must succeed");
        let _is_valid =
            verify_token(&token, &yantra_dir).expect("sla-test token verify must succeed");
        iteration_durations.push(iteration_start.elapsed());
    }

    iteration_durations.sort();

    let p99_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 iterations must produce 100 duration samples");

    assert!(
        p99_duration.as_millis() < 5,
        "STVP token issue+verify p99 latency exceeded 5ms SLA target: {p99_duration:?}",
    );
}
