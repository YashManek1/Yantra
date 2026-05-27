//! # forge-orchestrator: In-Process Event Bus
//!
//! Provides an in-process `tokio::sync::broadcast` channel for orchestrator
//! events. Every published `AgentMessage` is first written to `decisions.sqlite`
//! via `forge-obs::record_decision` before being broadcast to subscribers, so
//! the full event history survives crashes.
//!
//! ## Input
//! - `AgentMessage` variants produced by the scheduler, circuit breaker, and
//!   speculation engine
//! - A path to `decisions.sqlite` and a `SessionId` for archaeology records
//!
//! ## Output
//! - Durable `DecisionRecord` rows in `decisions.sqlite`
//! - Cloneable `broadcast::Receiver<AgentMessage>` for any subscriber
//!
//! ## Related
//! - `forge-obs::decision_archaeology` — persistence target for every message
//! - `forge-orchestrator::scheduler` — primary publisher of task lifecycle events

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use tokio::sync::broadcast;
use yantra_core::{connection_pool, AgentKind, DecisionId, SessionId, TaskId};
use yantra_obs::{record_decision, DecisionRecord};

use crate::error::OrchestratorError;

/// Channel capacity: 1 024 buffered messages before slow subscribers are dropped.
const BUS_CAPACITY: usize = 1_024;

/// Events broadcast on the orchestrator event bus.
///
/// Every variant is persisted to `decisions.sqlite` before broadcast.
#[derive(Debug, Clone)]
pub enum AgentMessage {
    /// A new task was added to the DAG.
    TaskQueued {
        /// Identifier of the newly queued task.
        task_id: TaskId,
    },
    /// The scheduler began executing a task.
    TaskStarted {
        /// Identifier of the task that started.
        task_id: TaskId,
        /// Agent kind dispatched to execute it.
        agent_kind: AgentKind,
    },
    /// A task completed successfully.
    TaskCompleted {
        /// Identifier of the completed task.
        task_id: TaskId,
        /// Decision produced by the agent execution.
        decision_id: DecisionId,
        /// Agent kind dispatched to execute it.
        agent_kind: AgentKind,
        /// One-line summary of what was done.
        summary: String,
    },
    /// A task execution failed (non-final — may be retried).
    TaskFailed {
        /// Identifier of the failed task.
        task_id: TaskId,
        /// Human-readable failure reason.
        reason: String,
        /// How many times this task has now failed.
        retry_count: u32,
    },
    /// A task exceeded the maximum retry count and is permanently failed.
    TaskMaxRetries {
        /// Identifier of the permanently failed task.
        task_id: TaskId,
    },
    /// The circuit breaker transitioned to Open, halting all dispatching.
    CircuitOpen {
        /// Observed LLM calls per minute that triggered the transition.
        calls_per_minute: u32,
    },
    /// The circuit breaker entered HalfOpen and will attempt a probe call.
    CircuitHalfOpen,
    /// The circuit breaker returned to Closed after a successful probe.
    CircuitClosed,
    /// The speculation engine produced predictions for a recently completed task.
    SpeculationReady {
        /// Predicted task descriptions (top 3).
        predicted_descriptions: Vec<String>,
    },
    /// The DAG ready-task count changed.
    DagUpdated {
        /// Number of tasks currently ready to execute.
        ready_count: usize,
    },
}

/// In-process event bus for orchestrator messages.
///
/// Wraps a `tokio::sync::broadcast::Sender` with a SQLite persistence layer.
/// Callers hold an `Arc<EventBus>` and call `publish` to emit events.
pub struct EventBus {
    sender: broadcast::Sender<AgentMessage>,
    decisions_db: Mutex<rusqlite::Connection>,
    session_id: SessionId,
}

impl EventBus {
    /// Opens the event bus, connecting to `decisions.sqlite` in `yantra_dir`.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` when the database cannot be opened
    /// or the decisions schema cannot be applied.
    pub fn open(yantra_dir: &Path, session_id: SessionId) -> Result<Arc<Self>, OrchestratorError> {
        let database_path = yantra_dir.join("decisions.sqlite");
        let pool = connection_pool(&database_path)
            .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
        let db_connection = Arc::try_unwrap(pool)
            .map_err(|_| OrchestratorError::Database("could not unwrap decisions pool".to_owned()))?
            .into_inner()
            .map_err(|_| OrchestratorError::Database("decisions mutex poisoned".to_owned()))?;

        yantra_obs::decision_archaeology::ensure_decisions_schema(&db_connection)?;

        let (sender, _initial_receiver) = broadcast::channel(BUS_CAPACITY);
        Ok(Arc::new(Self {
            sender,
            decisions_db: Mutex::new(db_connection),
            session_id,
        }))
    }

    /// Persists `message` to `decisions.sqlite` then broadcasts it.
    ///
    /// The broadcast will succeed as long as at least one receiver exists. If
    /// no receivers are active the send is silently dropped (this is normal
    /// during shutdown or before the first `subscribe` call).
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Obs` when persistence fails.
    pub fn publish(&self, message: AgentMessage) -> Result<(), OrchestratorError> {
        let decision_record = message_to_decision_record(&message, self.session_id);
        {
            let db_guard = self.decisions_db.lock();
            record_decision(&db_guard, &decision_record)?;
        }
        let _ = self.sender.send(message);
        Ok(())
    }

    /// Returns a new receiver for all future messages on this bus.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentMessage> {
        self.sender.subscribe()
    }
}

fn message_to_decision_record(message: &AgentMessage, session_id: SessionId) -> DecisionRecord {
    if let AgentMessage::TaskCompleted {
        decision_id,
        agent_kind,
        summary,
        ..
    } = message
    {
        let parsed_git_hash = if summary.starts_with("commit ") {
            summary
                .split_whitespace()
                .nth(1)
                .map(|s| s.trim_end_matches(':').to_owned())
        } else {
            None
        };
        DecisionRecord {
            id: *decision_id,
            timestamp: Utc::now(),
            session_id,
            parent_decision_id: None,
            action_type: "task_completed".to_owned(),
            reasoning: summary.clone(),
            alternatives_considered: vec![],
            truth_token: None,
            git_hash: parsed_git_hash,
            agent: *agent_kind,
        }
    } else {
        let (action_type, reasoning) = describe_message(message);
        DecisionRecord {
            id: DecisionId::new(),
            timestamp: Utc::now(),
            session_id,
            parent_decision_id: None,
            action_type,
            reasoning,
            alternatives_considered: vec![],
            truth_token: None,
            git_hash: None,
            agent: AgentKind::IntegrityChecker,
        }
    }
}

fn describe_message(message: &AgentMessage) -> (String, String) {
    match message {
        AgentMessage::TaskQueued { task_id } => (
            "task_queued".to_owned(),
            format!("task {task_id} added to the DAG"),
        ),
        AgentMessage::TaskStarted {
            task_id,
            agent_kind,
        } => (
            "task_started".to_owned(),
            format!("task {task_id} dispatched to {agent_kind:?}"),
        ),
        AgentMessage::TaskCompleted {
            task_id,
            decision_id,
            agent_kind: _,
            summary: _,
        } => (
            "task_completed".to_owned(),
            format!("task {task_id} completed; decision {decision_id}"),
        ),
        AgentMessage::TaskFailed {
            task_id,
            reason,
            retry_count,
        } => (
            "task_failed".to_owned(),
            format!("task {task_id} failed (attempt {retry_count}): {reason}"),
        ),
        AgentMessage::TaskMaxRetries { task_id } => (
            "task_max_retries".to_owned(),
            format!("task {task_id} exhausted all retry attempts"),
        ),
        AgentMessage::CircuitOpen { calls_per_minute } => (
            "circuit_open".to_owned(),
            format!("circuit breaker opened at {calls_per_minute} calls/min"),
        ),
        AgentMessage::CircuitHalfOpen => (
            "circuit_half_open".to_owned(),
            "circuit breaker entered half-open state".to_owned(),
        ),
        AgentMessage::CircuitClosed => (
            "circuit_closed".to_owned(),
            "circuit breaker closed after successful probe".to_owned(),
        ),
        AgentMessage::SpeculationReady {
            predicted_descriptions,
        } => (
            "speculation_ready".to_owned(),
            format!("{} predicted tasks available", predicted_descriptions.len()),
        ),
        AgentMessage::DagUpdated { ready_count } => (
            "dag_updated".to_owned(),
            format!("{ready_count} tasks ready to execute"),
        ),
    }
}
