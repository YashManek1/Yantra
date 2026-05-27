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

pub mod anomaly;
pub mod cost;
pub(crate) mod db_util;
pub mod decision_archaeology;
pub mod error;
pub mod traces;
pub mod tracing;
pub mod watchdog;

pub use anomaly::{AnomalyDetector, AnomalyEvent, AnomalyMetric, AnomalySample};
pub use cost::{CostGauge, CostStatus, CostThresholds};
pub use decision_archaeology::{
    causal_chain, descendants, record_decision, DecisionRecord, DECISIONS_SCHEMA_SQL,
};
pub use error::{ObsError, ObsResult};
pub use traces::{query_session_spans, query_task_spans, record_span, TRACES_SCHEMA_SQL};
pub use tracing::{init_tracing, SqliteSpanLayer, TracingConfig, TracingGuard};
pub use watchdog::{WatchdogClient, WatchdogMessage, HEARTBEAT_INTERVAL, SILENCE_KILL_THRESHOLD};
