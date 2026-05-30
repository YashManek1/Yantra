Implements the verification layer that checks generated diffs against truth drift, static analysis, tests, and grounding signals. It is the gate between agent output and any committed or user-visible change.

## Gates

| Gate | What it checks | Enforcement |
|------|----------------|-------------|
| Gate 1 — Truth Drift | Diff touches out-of-scope files or violates `new_deps_allowed` | Synchronous; blocks dispatch |
| Gate 2 — Hallucination L1 | CRG symbol cross-check: added identifiers not in graph | Synchronous; emits warnings |
| Gate 2 — Hallucination L2 | LSP diagnostic cross-check: confirms undefined symbols | Async; requires `lsp_bridge` in `VerificationContext` |
| Gate 3 — Static Analysis | Delegates to `cargo check` output | Async |
| Boolean Exit Gate | All tests must pass (`cargo nextest`) | CI-enforced |

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| Gate 1 truth-drift check p99 | ≤ 10 ms | < 1 ms (pure in-memory logic) |
| Gate 1 accuracy (10 seeded fixtures: 5 PASS + 5 FAIL) | ≥ 90% | **100%** (10/10) |
| Hallucination L1 detection precision (5 hallucinated diffs) | ≥ 80% | **100%** (5/5) |
| Hallucination L1 false-positive rate (5 clean diffs) | 0 FP | **0** false positives |

Benchmarks: `crates/forge-verifier/benches/verifier_bench.rs` — run with `cargo bench -p yantra-verifier`.
Accuracy tests: `crates/forge-verifier/tests/verifier_accuracy.rs` — gate1 fixture corpus + hallucination precision/recall.
SLA test: `verifier_gate1_p99_meets_10ms_sla`.
