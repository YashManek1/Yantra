//! # forge-obs: Observability Substrate
//!
//! Provides `OpenTelemetry` span instrumentation, a `SQLite`-backed trace store,
//! a real-time cost gauge, and the watchdog heartbeat monitor. Every LLM
//! call and every tool invocation must produce a span through this crate.
//!
//! ## Input
//! - Span start/finish events from any Yantra crate
//! - Cost metadata per LLM call (tokens in/out, model tier)
//!
//! ## Output
//! - `OTel` spans written to `traces.sqlite`
//! - Cumulative cost gauge accessible via `SSE` from `forge-serve`
//! - Watchdog signals that kill a hung orchestrator session
//!
//! ## Related
//! - `forge-orchestrator` — annotates every `DAG` node with spans
//! - `forge-serve` — streams the cost gauge to the Live Canvas
