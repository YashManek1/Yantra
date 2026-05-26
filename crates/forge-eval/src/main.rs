//! # forge-eval: `SWE-bench` Evaluation Runner
//!
//! Runs the Yantra runtime against a curated subset of `SWE-bench` Verified
//! instances to measure task success rate, cost, and latency regressions.
//! Intended for `CI` (nightly workflow) and local perf checks.
//!
//! ## Input
//! - `SWE-bench` instance `JSONL` from a local path or fetched subset
//! - Runtime configuration (model tiers, `CRG` settings) via environment
//!
//! ## Output
//! - A structured evaluation report written to stdout and to a JSON file
//! - Exit code 0 only if pass rate meets the configured threshold
//!
//! ## Related
//! - `forge-orchestrator` — runs each benchmark task through the full pipeline

fn main() {}
