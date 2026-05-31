//! # forge-orchestrator: DAG Schedule and CSP Planner Benchmarks
//!
//! Measures the latency of two core scheduling primitives:
//!
//! 1. `dag_schedule_10_tasks` — opens a single `TaskDag`, then per iteration
//!    clears it, inserts a linear chain of 10 `TaskNode`s, records all dependency
//!    edges, and calls `ready_tasks()` to verify scheduling readiness. The
//!    `TaskDag::open` cost is excluded from the measurement.
//!
//! 2. `csp_planner_enumerate` — constructs five tasks with three `Precedes`
//!    constraints and calls `CspPlanner::solve` to enumerate valid orderings.
//!
//! Both benchmarks target a p99 latency of ≤ 20 ms.
//!
//! ## Input
//! - A single persistent `TaskDag` database, cleared between criterion samples
//! - Fixed arrays of `TaskId` values for the CSP planner
//!
//! ## Output
//! - Criterion statistical report (mean, standard deviation, p50, p99 estimates)
//!
//! ## Related
//! - `forge-orchestrator::dag`         — `TaskDag` under test
//! - `forge-orchestrator::csp_planner` — `CspPlanner` under test

use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use yantra_core::{TaskClass, TaskId, TaskNode, TaskStatus};
use yantra_orchestrator::{Constraint, CspPlanner, TaskDag};

fn build_pending_task(task_id: TaskId, description: &str) -> TaskNode {
    TaskNode {
        id: task_id,
        description: description.to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: vec![],
        assigned_agent: None,
        truth_token: None,
        parent_decision_id: None,
    }
}

fn temp_dag_dir() -> std::path::PathBuf {
    let directory_path = std::env::temp_dir().join(format!("yantra-orch-bench-{}", TaskId::new()));
    std::fs::create_dir_all(&directory_path).expect("bench temp dir creation must succeed");
    directory_path
}

fn bench_dag_schedule_10_tasks(benchmark_context: &mut Criterion) {
    let dag_directory = temp_dag_dir();
    let task_dag = TaskDag::open(&dag_directory).expect("TaskDag::open must succeed");

    benchmark_context.bench_function("dag_schedule_10_tasks", |bencher| {
        bencher.iter(|| {
            task_dag
                .clear()
                .expect("TaskDag::clear must succeed between benchmark iterations");

            let task_ids: Vec<TaskId> = (0..10).map(|_| TaskId::new()).collect();

            for (index, &task_id) in task_ids.iter().enumerate() {
                let task_node = build_pending_task(task_id, &format!("task-{index}"));
                task_dag
                    .add_task(&task_node, "")
                    .expect("add_task must succeed");
            }

            for window in task_ids.windows(2) {
                task_dag
                    .add_dependency(window[1], window[0])
                    .expect("add_dependency must succeed");
            }

            let ready_task_ids = task_dag.ready_tasks().expect("ready_tasks must succeed");
            assert_eq!(
                ready_task_ids.len(),
                1,
                "only the first task in the chain should be ready"
            );
        });
    });
}

fn bench_csp_planner_enumerate(benchmark_context: &mut Criterion) {
    let task_id_a = TaskId::new();
    let task_id_b = TaskId::new();
    let task_id_c = TaskId::new();
    let task_id_d = TaskId::new();
    let task_id_e = TaskId::new();

    let task_ids = vec![task_id_a, task_id_b, task_id_c, task_id_d, task_id_e];

    let hard_constraints = vec![
        Constraint::Precedes {
            before: task_id_a,
            after: task_id_b,
        },
        Constraint::Precedes {
            before: task_id_b,
            after: task_id_c,
        },
        Constraint::Precedes {
            before: task_id_c,
            after: task_id_d,
        },
    ];

    let planner = CspPlanner::new();

    benchmark_context.bench_function("csp_planner_enumerate", |bencher| {
        bencher.iter(|| {
            let plans = planner
                .solve(&task_ids, &hard_constraints, &[])
                .expect("CspPlanner::solve must succeed");
            assert!(
                !plans.is_empty(),
                "CSP solve must return at least one valid plan"
            );
        });
    });
}

criterion_group! {
    name    = orchestrator_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(50);
    targets = bench_dag_schedule_10_tasks, bench_csp_planner_enumerate
}
criterion_main!(orchestrator_benches);
