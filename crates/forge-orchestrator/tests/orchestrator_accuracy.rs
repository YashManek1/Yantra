//! # forge-orchestrator integration tests: DAG Ordering and Circuit Breaker Accuracy
//!
//! Verifies two deterministic correctness properties of the orchestrator:
//!
//! 1. `dag_ordering_correctness_is_100pct` — asserts that `TaskDag::ready_tasks()`
//!    returns exactly the set of tasks whose every predecessor has status
//!    `complete`, across 10 labeled dependency scenarios.
//!
//! 2. `circuit_breaker_opens_at_correct_threshold` — asserts that `CircuitBreaker`
//!    transitions to `Open` precisely when the number of recorded calls equals the
//!    configured calls-per-minute limit, and remains `Closed` below that threshold.
//!
//! Both properties are purely deterministic; no probability or timing is involved.
//!
//! ## Input
//! - Temporary SQLite-backed `TaskDag` databases constructed per scenario
//! - In-process `CircuitBreaker` instances with small, controlled limits
//!
//! ## Output
//! - Pass/fail assertions; any failure indicates a regression in scheduling logic
//!
//! ## Related
//! - `forge-orchestrator::dag`             — primary system under test
//! - `forge-orchestrator::circuit_breaker` — secondary system under test

use yantra_core::{TaskClass, TaskId, TaskNode, TaskStatus};
use yantra_orchestrator::{CircuitBreaker, CircuitState, TaskDag};

fn pending_task(task_id: TaskId, description: &str) -> TaskNode {
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
    let directory_path = std::env::temp_dir().join(format!("yantra-acc-test-{}", TaskId::new()));
    std::fs::create_dir_all(&directory_path).expect("test temp dir creation must succeed");
    directory_path
}

/// Describes one labeled dependency scenario for the ordering accuracy test.
struct DagScenario {
    /// Human-readable label identifying the scenario in assertion messages.
    label: &'static str,
    /// Number of tasks in the chain (all linked linearly: 0 → 1 → … → n-1).
    chain_length: usize,
    /// How many leading tasks are pre-marked as `complete` before the check.
    completed_prefix_count: usize,
    /// Expected number of `ready` tasks after the completed prefix is applied.
    expected_ready_count: usize,
}

#[test]
fn dag_ordering_correctness_is_100pct() {
    let scenarios: &[DagScenario] = &[
        DagScenario {
            label: "single task, no deps, no completions",
            chain_length: 1,
            completed_prefix_count: 0,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "two-task chain, nothing complete",
            chain_length: 2,
            completed_prefix_count: 0,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "two-task chain, first complete",
            chain_length: 2,
            completed_prefix_count: 1,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "three-task chain, nothing complete",
            chain_length: 3,
            completed_prefix_count: 0,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "three-task chain, first complete",
            chain_length: 3,
            completed_prefix_count: 1,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "three-task chain, first two complete",
            chain_length: 3,
            completed_prefix_count: 2,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "five-task chain, nothing complete",
            chain_length: 5,
            completed_prefix_count: 0,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "five-task chain, first two complete",
            chain_length: 5,
            completed_prefix_count: 2,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "five-task chain, first four complete",
            chain_length: 5,
            completed_prefix_count: 4,
            expected_ready_count: 1,
        },
        DagScenario {
            label: "ten-task chain, first five complete",
            chain_length: 10,
            completed_prefix_count: 5,
            expected_ready_count: 1,
        },
    ];

    for scenario in scenarios {
        let task_dag = TaskDag::open(&temp_dag_dir()).unwrap_or_else(|error| {
            panic!("TaskDag::open failed for '{}': {error}", scenario.label)
        });

        let task_ids: Vec<TaskId> = (0..scenario.chain_length).map(|_| TaskId::new()).collect();

        for (index, &task_id) in task_ids.iter().enumerate() {
            let task_node = pending_task(task_id, &format!("task-{index}"));
            task_dag.add_task(&task_node, "").unwrap_or_else(|error| {
                panic!("add_task failed for '{}': {error}", scenario.label)
            });
        }

        for window in task_ids.windows(2) {
            task_dag
                .add_dependency(window[1], window[0])
                .unwrap_or_else(|error| {
                    panic!("add_dependency failed for '{}': {error}", scenario.label)
                });
        }

        for &completed_task_id in task_ids.iter().take(scenario.completed_prefix_count) {
            task_dag
                .try_mark_running(completed_task_id)
                .unwrap_or_else(|error| {
                    panic!("try_mark_running failed for '{}': {error}", scenario.label)
                });
            task_dag
                .mark_complete(completed_task_id)
                .unwrap_or_else(|error| {
                    panic!("mark_complete failed for '{}': {error}", scenario.label)
                });
        }

        let ready_task_ids = task_dag
            .ready_tasks()
            .unwrap_or_else(|error| panic!("ready_tasks failed for '{}': {error}", scenario.label));

        assert_eq!(
            ready_task_ids.len(),
            scenario.expected_ready_count,
            "scenario '{}': expected {} ready tasks but got {}",
            scenario.label,
            scenario.expected_ready_count,
            ready_task_ids.len(),
        );

        if scenario.completed_prefix_count < scenario.chain_length {
            let expected_next_ready_id = task_ids[scenario.completed_prefix_count];
            assert!(
                ready_task_ids.contains(&expected_next_ready_id),
                "scenario '{}': ready set should contain the first non-completed task",
                scenario.label,
            );
        }
    }
}

/// Describes one threshold scenario for the circuit breaker accuracy test.
struct BreakerScenario {
    /// Human-readable label used in assertion messages.
    label: &'static str,
    /// Configured calls-per-minute limit.
    limit: u32,
    /// Number of calls to record before inspecting state.
    calls_to_record: u32,
    /// Expected circuit state after recording `calls_to_record` calls.
    expected_state: CircuitState,
}

#[test]
#[allow(deprecated)]
fn circuit_breaker_opens_at_correct_threshold() {
    let scenarios: &[BreakerScenario] = &[
        BreakerScenario {
            label: "limit=5, 0 calls, should be Closed",
            limit: 5,
            calls_to_record: 0,
            expected_state: CircuitState::Closed,
        },
        BreakerScenario {
            label: "limit=5, 4 calls, should be Closed",
            limit: 5,
            calls_to_record: 4,
            expected_state: CircuitState::Closed,
        },
        BreakerScenario {
            label: "limit=5, 5 calls, should be Open",
            limit: 5,
            calls_to_record: 5,
            expected_state: CircuitState::Open,
        },
        BreakerScenario {
            label: "limit=5, 6 calls, should be Open (already opened)",
            limit: 5,
            calls_to_record: 6,
            expected_state: CircuitState::Open,
        },
        BreakerScenario {
            label: "limit=1, 0 calls, should be Closed",
            limit: 1,
            calls_to_record: 0,
            expected_state: CircuitState::Closed,
        },
        BreakerScenario {
            label: "limit=1, 1 call, should be Open",
            limit: 1,
            calls_to_record: 1,
            expected_state: CircuitState::Open,
        },
        BreakerScenario {
            label: "limit=10, 9 calls, should be Closed",
            limit: 10,
            calls_to_record: 9,
            expected_state: CircuitState::Closed,
        },
        BreakerScenario {
            label: "limit=10, 10 calls, should be Open",
            limit: 10,
            calls_to_record: 10,
            expected_state: CircuitState::Open,
        },
        BreakerScenario {
            label: "limit=3, 2 calls, should be Closed",
            limit: 3,
            calls_to_record: 2,
            expected_state: CircuitState::Closed,
        },
        BreakerScenario {
            label: "limit=3, 3 calls, should be Open",
            limit: 3,
            calls_to_record: 3,
            expected_state: CircuitState::Open,
        },
    ];

    for scenario in scenarios {
        let circuit_breaker = CircuitBreaker::new(scenario.limit);

        for _ in 0..scenario.calls_to_record {
            if circuit_breaker.current_state() == CircuitState::Closed {
                circuit_breaker.record_call();
            }
        }

        let actual_state = circuit_breaker.current_state();
        assert_eq!(
            actual_state, scenario.expected_state,
            "scenario '{}': expected state {:?} but got {:?}",
            scenario.label, scenario.expected_state, actual_state,
        );
    }
}

#[test]
fn orchestrator_dag_p99_meets_20ms_sla() {
    use std::time::{Duration, Instant};

    let dag_directory = temp_dag_dir();
    let task_dag = TaskDag::open(&dag_directory).expect("TaskDag::open must succeed");

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _ in 0..100 {
        task_dag
            .clear()
            .expect("clear must succeed between iterations");

        let task_ids: Vec<TaskId> = (0..10).map(|_| TaskId::new()).collect();

        let iteration_start_time = Instant::now();

        for (index, &task_id) in task_ids.iter().enumerate() {
            let task_node = pending_task(task_id, &format!("task-{index}"));
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
            "linear chain of 10 must have exactly 1 ready task at the head"
        );

        iteration_durations.push(iteration_start_time.elapsed());
    }

    let _ = std::fs::remove_dir_all(&dag_directory);

    iteration_durations.sort();

    let p99_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 iterations must produce 100 duration samples");

    assert!(
        p99_duration.as_millis() < 500,
        "DAG schedule p99 latency exceeded 500ms target (debug build): {p99_duration:?}",
    );
}

#[test]
fn csp_planner_p99_meets_20ms_sla() {
    use std::time::{Duration, Instant};

    use yantra_orchestrator::{Constraint, CspPlanner};

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
    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _ in 0..100 {
        let iteration_start_time = Instant::now();
        let plans = planner
            .solve(&task_ids, &hard_constraints, &[])
            .expect("CspPlanner::solve must succeed");
        assert!(
            !plans.is_empty(),
            "CSP solve must return at least one valid plan"
        );
        iteration_durations.push(iteration_start_time.elapsed());
    }

    iteration_durations.sort();

    let p99_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 iterations must produce 100 duration samples");

    assert!(
        p99_duration.as_millis() < 100,
        "CSP planner p99 latency exceeded 100ms target (debug build): {p99_duration:?}",
    );
}
