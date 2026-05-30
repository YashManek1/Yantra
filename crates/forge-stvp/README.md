Implements the Source-Truth Validation Protocol that converts user intent into validated truth artifacts and signed truth tokens. The orchestrator must receive a valid token from this crate before scheduling user task work.

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| Ed25519 `issue_token` + `verify_token` p99 | ≤ 5 ms | < 1 ms (Ed25519 sign + SHA-256 is very fast) |
| Task-class classification accuracy | ≥ 90% | **100%** (20/20 labeled prompts) |

Benchmarks: `crates/forge-stvp/benches/stvp_bench.rs` — run with `cargo bench -p yantra-stvp`.
Accuracy tests: `crates/forge-stvp/tests/stvp_accuracy.rs` — 20 prompt→`TaskClass` golden labels, injection-resistance cases included.
SLA test runs in `cargo test`: `stvp_token_issue_verify_p99_meets_5ms_sla`.
