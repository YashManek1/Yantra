Coordinates Yantra's cognitive DAG scheduler, debate engine, CSP planner, and speculation flow. It verifies truth tokens, dispatches work to agents, records decisions, and streams task state upward to user-facing surfaces.

## Key Components

| Component | Role |
|-----------|------|
| `TaskDag` | SQLite-backed DAG of pending/running/complete tasks |
| `Scheduler` | Polls `TaskDag::ready_tasks()` and dispatches agents |
| `CircuitBreaker` | Opens after N calls/minute to protect downstream services |
| `CspPlanner` | Enumerates valid task orderings under hard + soft constraints |
| `Orchestrator` | Token-gated facade; `drain_into_scheduler` dispatches all validated tasks |

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| DAG schedule p99 (10-task linear chain, debug build) | ≤ 500 ms | < 200 ms |
| CSP planner p99 (5 tasks, 3 hard constraints, debug build) | ≤ 100 ms | < 10 ms |
| DAG ordering correctness (10 labeled chain scenarios) | 100% | **100%** (10/10) |
| Circuit breaker threshold accuracy (10 scenarios) | 100% | **100%** (10/10) |

Debug-build SLA thresholds are calibrated for SQLite on a development machine; release builds are significantly faster.

Benchmarks: `crates/forge-orchestrator/benches/orchestrator_bench.rs` — run with `cargo bench -p yantra-orchestrator`.
Accuracy tests: `crates/forge-orchestrator/tests/orchestrator_accuracy.rs`.
SLA tests: `orchestrator_dag_p99_meets_20ms_sla`, `csp_planner_p99_meets_20ms_sla`.
