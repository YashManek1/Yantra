//! # forge-verifier: Gate 1 Truth Drift Benchmark
//!
//! Measures the latency of running `TruthDriftGate::check` with a clean diff
//! that does not touch any out-of-scope file or dependency manifest. This is
//! the hot path exercised on every diff produced by the Coder agent.
//!
//! The p99 SLA target is ≤ 10 ms; typical measurements are well below 1 ms
//! because the gate is purely in-memory regex matching with no I/O.
//!
//! ## Input
//! - A synthetic `SourceTruth` with scope and constraint answers set
//! - A synthetic unified diff touching only an in-scope source file
//!
//! ## Output
//! - Criterion statistical report: mean, standard deviation, p50, p99 estimates
//!
//! ## Related
//! - `forge-verifier::gate1_truth_drift` — `TruthDriftGate::check` under test
//! - `forge-stvp::source_truth`          — `SourceTruth` consumed by the gate
//! - `forge-stvp::drift`                 — underlying `TruthDriftDetector`

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::Utc;
use criterion::{criterion_group, criterion_main, Criterion};
use yantra_core::{Strictness, TaskClass, TaskId};
use yantra_stvp::source_truth::SourceTruth;
use yantra_verifier::types::Diff;
use yantra_verifier::TruthDriftGate;

fn make_clean_source_truth() -> SourceTruth {
    let mut answers = BTreeMap::new();
    answers.insert("out_of_scope".to_owned(), "src/payments.rs".to_owned());
    answers.insert("new_deps_allowed".to_owned(), "yes".to_owned());

    SourceTruth {
        task_id: TaskId::new(),
        task_class: TaskClass::BugFix,
        strictness: Strictness::Strict,
        description: "fix null pointer dereference in session handler".to_owned(),
        created_at: Utc::now(),
        answers,
        augmented_question_ids: Vec::new(),
    }
}

fn make_clean_diff() -> Diff {
    let diff_text = "\
--- a/src/auth.rs\n\
+++ b/src/auth.rs\n\
@@ -10,3 +10,4 @@\n\
+    let session_guard = session.lock().expect(\"session lock must not be poisoned\");\n\
"
    .to_owned();

    Diff {
        text: diff_text,
        task_class: TaskClass::BugFix,
    }
}

fn bench_gate1_truth_drift_check(benchmark_context: &mut Criterion) {
    let source_truth = make_clean_source_truth();
    let clean_diff = make_clean_diff();

    for _warmup_index in 0..5 {
        let _warmup_result = TruthDriftGate::check(&source_truth, &clean_diff)
            .expect("warm-up gate1 check must succeed");
    }

    benchmark_context.bench_function("gate1_truth_drift_check", |bencher| {
        bencher.iter(|| {
            let _check_result = TruthDriftGate::check(&source_truth, &clean_diff)
                .expect("benchmark gate1 check must succeed");
        });
    });
}

criterion_group! {
    name    = verifier_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100);
    targets = bench_gate1_truth_drift_check
}
criterion_main!(verifier_benches);

#[test]
fn verifier_gate1_p99_meets_10ms_sla() {
    use std::time::Instant;

    let source_truth = make_clean_source_truth();
    let clean_diff = make_clean_diff();

    for _warmup_index in 0..5 {
        let _warmup_result = TruthDriftGate::check(&source_truth, &clean_diff)
            .expect("warm-up gate1 check must succeed");
    }

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _iteration_index in 0..100 {
        let iteration_start = Instant::now();
        let _check_result = TruthDriftGate::check(&source_truth, &clean_diff)
            .expect("sla-test gate1 check must succeed");
        iteration_durations.push(iteration_start.elapsed());
    }

    iteration_durations.sort();

    let p99_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 iterations must produce 100 duration samples");

    assert!(
        p99_duration.as_millis() < 10,
        "TruthDriftGate p99 latency exceeded 10ms SLA target: {p99_duration:?}",
    );
}
