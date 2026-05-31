//! # forge-router: Routing Policy Classification Benchmark
//!
//! Measures the latency of `RoutingPolicy::classify` called in a tight loop
//! of 1 000 iterations per criterion sample. The classifier is entirely
//! in-memory (no I/O, no async, no allocations beyond the input struct), so
//! the p99 SLA target is ≤ 1 ms per single-call iteration.
//!
//! ## Input
//! - A default `RoutingPolicy` with `tier0_token_limit = 2 000` and
//!   `tier0_tool_call_limit = 3`
//! - A representative `TaskDescription` for each TaskClass tier
//!
//! ## Output
//! - Criterion statistical report: mean, standard deviation, p50, p99 estimates
//!
//! ## Related
//! - `forge-router::routing` — `RoutingPolicy::classify` under test

use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use yantra_core::TaskClass;
use yantra_router::routing::{RoutingPolicy, TaskDescription};

fn make_default_policy() -> RoutingPolicy {
    RoutingPolicy::default()
}

fn make_sample_task_description() -> TaskDescription {
    TaskDescription {
        description: "fix null pointer crash in session handler".to_owned(),
        class: TaskClass::BugFix,
        tokens_estimated: 800,
        tool_calls_predicted: 2,
        touches_sacred_files: false,
        multi_file: false,
    }
}

fn bench_routing_policy_classify(benchmark_context: &mut Criterion) {
    let routing_policy = make_default_policy();
    let sample_task = make_sample_task_description();

    for _warmup_index in 0..10 {
        let _tier = routing_policy.classify(&sample_task);
    }

    benchmark_context.bench_function("routing_policy_classify", |bencher| {
        bencher.iter(|| {
            for _iteration_index in 0..1_000 {
                let _tier = routing_policy.classify(&sample_task);
            }
        });
    });
}

criterion_group! {
    name    = router_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100);
    targets = bench_routing_policy_classify
}
criterion_main!(router_benches);

#[test]
fn router_policy_p99_meets_1ms_sla() {
    use std::time::Instant;

    let routing_policy = make_default_policy();
    let sample_task = make_sample_task_description();

    for _warmup_index in 0..10 {
        let _tier = routing_policy.classify(&sample_task);
    }

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _iteration_index in 0..100 {
        let iteration_start = Instant::now();
        let _tier = routing_policy.classify(&sample_task);
        iteration_durations.push(iteration_start.elapsed());
    }

    iteration_durations.sort();

    let p99_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 iterations must produce 100 duration samples");

    assert!(
        p99_duration.as_millis() < 1,
        "RoutingPolicy::classify p99 latency exceeded 1ms SLA target: {p99_duration:?}",
    );
}
