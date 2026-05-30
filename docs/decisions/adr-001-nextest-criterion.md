# ADR-001: Adopt nextest + criterion as the test and benchmark infrastructure

**Date:** 2026-05-29
**Status:** Accepted
**Deciders:** Yash Manek (Sankalp Systems)

## Context

`cargo test` is single-threaded by default and produces minimal output for CI reporting. Criterion provides statistical confidence for benchmark numbers (p50/p99 with noise filtering), whereas the built-in benchmark harness is unstable and gives raw wall-clock numbers. Yantra has latency targets for several hot paths (AST re-parse <150ms p99, CRG extraction <50ms p99) that require trustworthy measurement.

CI pipelines also benefit from JUnit XML output for test results visualization in GitHub Actions and other CI systems, which `cargo test` does not produce natively.

## Decision

Adopt `cargo-nextest` as the primary test runner for the workspace and `criterion` for all latency-critical benchmarks. Aliases `cargo test-all` and `cargo bench` are defined in `.cargo/config.toml`.

## Options Considered

| Option | Pros | Cons |
|--------|------|------|
| `cargo test` only | Zero new dependencies | Single-threaded, no JUnit output, no retries |
| `cargo-nextest` | Parallel, JUnit XML, retry on flake, per-test timeout | One extra install step |
| `criterion` | Statistical p50/p99, outlier detection, HTML reports | Adds `criterion` dependency to each bench crate |
| Built-in bench harness | Zero extra deps | Unstable feature, no statistical rigor |

## Consequences

**Positive:**
- Faster CI: nextest runs tests in parallel across all crates.
- Reliable latency numbers: criterion filters outliers and reports p50/p99 with confidence intervals.
- JUnit XML output enables CI dashboards and trend tracking.
- Per-test timeouts prevent hangs from blocking the entire suite.

**Negative:**
- Requires `cargo install cargo-nextest` in the dev setup script.
- Benchmark crates must add `criterion` as a dev-dependency.

## Related

- `crates/forge-ast/benches/ast_bench.rs` — first criterion benchmark; p99 = 7.2ms
- `crates/forge-crg/benches/` — CRG extraction benchmark (target <50ms)
- `nextest.toml` — workspace nextest configuration
- `docs/decisions/adr-002-louvain-community-detection.md`
