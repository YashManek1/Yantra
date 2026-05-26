//! # forge-core: Observability Span Types
//!
//! Defines the provider-neutral span record used by observability, tracing,
//! cost accounting, and postmortem analysis. The type stores identifiers and
//! metadata only; persistence lives outside this crate.
//!
//! ## Input
//! - Agent, model, token, timing, and outcome metadata from runtime events
//!
//! ## Output
//! - `Span`, `Outcome`, and `ErrorRecord`
//!
//! ## Related
//! - `forge-core::id` — provides span, task, and session identifiers
//! - `forge-core::truth` — provides optional truth-token grounding

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::AgentKind;
use crate::id::{SessionId, SpanId, TaskId};
use crate::model::ModelId;
use crate::truth::TruthToken;

/// Runtime outcome for an observed operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Outcome {
    /// Operation completed successfully.
    Success,
    /// Operation failed with an error.
    Failure,
    /// Operation exceeded its time limit.
    Timeout,
    /// Circuit breaker prevented execution.
    CircuitOpen,
    /// Cost or token budget prevented execution.
    BudgetExceeded,
}

/// Structured error metadata attached to a span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorRecord {
    /// Stable error category.
    pub kind: String,
    /// Human-readable error message.
    pub message: String,
    /// Span that originated the failure, when known.
    pub source_span: Option<SpanId>,
}

/// Observability span emitted by Yantra runtime actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    /// Unique span identifier.
    pub span_id: SpanId,
    /// Parent span identifier, when this span is nested.
    pub parent_id: Option<SpanId>,
    /// Session containing this span.
    pub session_id: SessionId,
    /// Task associated with this span, when any.
    pub task_id: Option<TaskId>,
    /// Truth token grounding the task, when any.
    pub truth_token: Option<TruthToken>,
    /// Agent responsible for this span.
    pub agent: Option<AgentKind>,
    /// Model used by this span, when any.
    pub model: Option<ModelId>,
    /// Input tokens consumed.
    pub tokens_in: u64,
    /// Output tokens produced.
    pub tokens_out: u64,
    /// Cost in USD.
    pub cost_usd: f64,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Timestamp at which the span started.
    pub started_at: DateTime<Utc>,
    /// Operation outcome.
    pub outcome: Outcome,
    /// Error metadata when outcome is not successful.
    pub error: Option<ErrorRecord>,
}
