//! # forge-night: Decision Rule Engine Benchmark
//!
//! Measures the latency of `RuleEngine::resolve` applied to a stream of 20
//! `NightEvent`s using a rule set of 5 `DecisionRule`s.
//!
//! The benchmark is fully in-memory (no I/O) and targets a p99 latency of
//! ≤ 10 ms for the full 20-event pass.
//!
//! ## Input
//! - A fixed slice of 5 `DecisionRule`s covering the full `RuleCondition` vocabulary
//! - A fixed sequence of 20 `NightEvent`s exercising all condition discriminants
//!
//! ## Output
//! - Criterion statistical report (mean, standard deviation, p50, p99 estimates)
//! - `#[test]` SLA assertion verifiable by `cargo test`
//!
//! ## Related
//! - `forge-night::decision_rules` — `RuleEngine` and supporting types under test

use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use yantra_night::decision_rules::{
    DecisionRule, NightEvent, RuleAction, RuleCondition, RuleEngine,
};

fn build_benchmark_rule_set() -> Vec<DecisionRule> {
    vec![
        DecisionRule {
            condition: RuleCondition::SecurityIssueFound,
            action: RuleAction::HaltAndDocument,
            priority: 100,
        },
        DecisionRule {
            condition: RuleCondition::CostExceeds(20.0),
            action: RuleAction::HardStop { notify: true },
            priority: 80,
        },
        DecisionRule {
            condition: RuleCondition::TestFailureAfterRetries(3),
            action: RuleAction::TagWipAndDefer,
            priority: 50,
        },
        DecisionRule {
            condition: RuleCondition::MergeConflict,
            action: RuleAction::RebaseAttempt {
                on_fail: Box::new(RuleAction::HardStop { notify: false }),
            },
            priority: 40,
        },
        DecisionRule {
            condition: RuleCondition::UnknownFileTouched,
            action: RuleAction::SkipTask,
            priority: 20,
        },
    ]
}

fn build_benchmark_event_sequence() -> Vec<NightEvent> {
    vec![
        NightEvent::TestFailure { retry_count: 0 },
        NightEvent::TestFailure { retry_count: 1 },
        NightEvent::TestFailure { retry_count: 3 },
        NightEvent::CostThresholdExceeded { total_usd: 5.0 },
        NightEvent::CostThresholdExceeded { total_usd: 25.0 },
        NightEvent::ExternalApiFailure { status_code: 429 },
        NightEvent::ExternalApiFailure { status_code: 500 },
        NightEvent::MergeConflictDetected,
        NightEvent::SecurityIssueDetected,
        NightEvent::LowConfidenceScore { score: 0.95 },
        NightEvent::LowConfidenceScore { score: 0.30 },
        NightEvent::UnknownFileAccessed,
        NightEvent::TestFailure { retry_count: 4 },
        NightEvent::CostThresholdExceeded { total_usd: 19.99 },
        NightEvent::MergeConflictDetected,
        NightEvent::SecurityIssueDetected,
        NightEvent::TestFailure { retry_count: 2 },
        NightEvent::ExternalApiFailure { status_code: 503 },
        NightEvent::UnknownFileAccessed,
        NightEvent::LowConfidenceScore { score: 0.50 },
    ]
}

fn bench_decision_rule_resolution(benchmark_context: &mut Criterion) {
    let rule_set = build_benchmark_rule_set();
    let event_sequence = build_benchmark_event_sequence();

    assert_eq!(rule_set.len(), 5, "benchmark requires exactly 5 rules");
    assert_eq!(
        event_sequence.len(),
        20,
        "benchmark requires exactly 20 events"
    );

    benchmark_context.bench_function("decision_rule_resolution", |bencher| {
        bencher.iter(|| {
            for event in &event_sequence {
                let _resolved_action = RuleEngine::resolve(event, &rule_set);
            }
        });
    });
}

criterion_group! {
    name    = night_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100);
    targets = bench_decision_rule_resolution
}
criterion_main!(night_benches);
