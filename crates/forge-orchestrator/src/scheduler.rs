//! # forge-orchestrator: DAG Scheduler
//!
//! Polls the `TaskDag` every `poll_interval` for ready tasks and dispatches
//! each one to its assigned agent via `tokio::spawn`. Failed tasks are retried
//! up to three times; after the third failure the task is permanently marked
//! `failed` and its ID is sent on the human-review channel.
//!
//! ## Input
//! - `TaskNode` values registered via `register_task`
//! - Ready-task signals from `TaskDag::ready_tasks` every poll cycle
//!
//! ## Output
//! - `AgentMessage` events on the `EventBus`
//! - Completed / failed task status updates in the `TaskDag`
//! - `TaskId` values on the human-review channel for permanently failed tasks
//!
//! ## Related
//! - `forge-orchestrator::dag` — source of ready tasks and status sink
//! - `forge-orchestrator::circuit_breaker` — gates dispatch when Open
//! - `forge-orchestrator::event_bus` — receives lifecycle events

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use yantra_agents::{Agent, AgentContext, TaskResult};
use yantra_core::{hex_encode, AgentKind, Outcome, SessionId, TaskId, TaskNode};
use yantra_router::Router;
use yantra_tools::McpRouter;

use crate::circuit_breaker::{CircuitBreaker, CircuitState};
use crate::dag::TaskDag;
use crate::error::OrchestratorError;
use crate::event_bus::{AgentMessage, EventBus};

/// Maximum number of times a task is retried before permanent failure.
const MAX_RETRY_COUNT: u32 = 3;

struct SchedulerInner {
    dag: Arc<TaskDag>,
    task_store: Mutex<HashMap<TaskId, TaskNode>>,
    agents: HashMap<AgentKind, Arc<dyn Agent>>,
    event_bus: Arc<EventBus>,
    circuit_breaker: Arc<CircuitBreaker>,
    router: Arc<Router>,
    tools: McpRouter,
    session_id: SessionId,
    retry_counts: Mutex<HashMap<TaskId, u32>>,
    human_review_sender: mpsc::UnboundedSender<TaskId>,
    poll_interval: Duration,
}

/// Background DAG scheduler.
///
/// Wraps an `Arc<SchedulerInner>` so cloning is O(1) and `register_task` is
/// safe to call while the background loop is running.
#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<SchedulerInner>,
}

impl Scheduler {
    /// Creates a scheduler and returns the human-review receiver alongside it.
    ///
    /// The `agents` map must include every `AgentKind` that appears as
    /// `assigned_agent` in registered tasks. Tasks with `assigned_agent = None`
    /// default to `AgentKind::Coder`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dag: Arc<TaskDag>,
        agents: HashMap<AgentKind, Arc<dyn Agent>>,
        event_bus: Arc<EventBus>,
        circuit_breaker: Arc<CircuitBreaker>,
        router: Arc<Router>,
        tools: McpRouter,
        session_id: SessionId,
        poll_interval: Duration,
    ) -> (Self, mpsc::UnboundedReceiver<TaskId>) {
        let (human_review_sender, human_review_receiver) = mpsc::unbounded_channel();
        let scheduler = Self {
            inner: Arc::new(SchedulerInner {
                dag,
                task_store: Mutex::new(HashMap::new()),
                agents,
                event_bus,
                circuit_breaker,
                router,
                tools,
                session_id,
                retry_counts: Mutex::new(HashMap::new()),
                human_review_sender,
                poll_interval,
            }),
        };
        (scheduler, human_review_receiver)
    }

    /// Adds a task to the DAG and the in-memory store.
    ///
    /// Applies all edges from `TaskNode.dependencies`. The `TruthToken`
    /// signature is stored in the DAG for audit when present.
    ///
    /// Publishes `AgentMessage::TaskQueued` on the event bus.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on DAG persistence failure.
    pub fn register_task(&self, task: TaskNode) -> Result<(), OrchestratorError> {
        let truth_token_sig = task
            .truth_token
            .as_ref()
            .map(|token| hex_encode(&token.signature))
            .unwrap_or_default();

        self.inner.dag.add_task(&task, &truth_token_sig)?;

        for &dep_id in &task.dependencies {
            self.inner.dag.add_dependency(task.id, dep_id)?;
        }

        let task_id = task.id;
        self.inner.task_store.lock().insert(task_id, task);

        let _ = self
            .inner
            .event_bus
            .publish(AgentMessage::TaskQueued { task_id });

        Ok(())
    }

    /// Spawns the background poll loop and returns its `JoinHandle`.
    ///
    /// The loop runs until the handle is aborted.
    pub fn spawn(&self) -> JoinHandle<()> {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            let mut tick_interval = tokio::time::interval(inner.poll_interval);
            loop {
                tick_interval.tick().await;
                poll_and_dispatch(Arc::clone(&inner)).await;
            }
        })
    }
}

async fn poll_and_dispatch(inner: Arc<SchedulerInner>) {
    let ready_task_ids = match inner.dag.ready_tasks() {
        Ok(task_ids) => task_ids,
        Err(dag_error) => {
            tracing::error!(?dag_error, "failed to query ready tasks");
            return;
        }
    };

    if !ready_task_ids.is_empty() {
        let _ = inner.event_bus.publish(AgentMessage::DagUpdated {
            ready_count: ready_task_ids.len(),
        });
    }

    for task_id in ready_task_ids {
        if inner.circuit_breaker.current_state() == CircuitState::Open {
            tracing::debug!(%task_id, "circuit breaker open; deferring dispatch");
            break;
        }

        let claimed = match inner.dag.try_mark_running(task_id) {
            Ok(claimed) => claimed,
            Err(dag_error) => {
                tracing::error!(?dag_error, %task_id, "failed to claim task");
                continue;
            }
        };

        if !claimed {
            continue;
        }

        let task_node = if let Some(node) = inner.task_store.lock().get(&task_id).cloned() {
            node
        } else {
            tracing::error!(%task_id, "task missing from store after claim");
            let _ = inner.dag.mark_failed(task_id);
            continue;
        };

        let agent_kind = task_node.assigned_agent.unwrap_or(AgentKind::Coder);
        let agent = if let Some(agent_arc) = inner.agents.get(&agent_kind).cloned() {
            agent_arc
        } else {
            tracing::error!(?agent_kind, %task_id, "no agent registered for kind");
            let _ = inner.dag.mark_failed(task_id);
            let _ = inner.event_bus.publish(AgentMessage::TaskFailed {
                task_id,
                reason: format!("no agent registered for {agent_kind:?}"),
                retry_count: 0,
            });
            continue;
        };

        inner.circuit_breaker.record_call();

        let agent_context = AgentContext {
            router: Arc::clone(&inner.router),
            tools: inner.tools.clone(),
            session_id: inner.session_id,
        };

        let _ = inner.event_bus.publish(AgentMessage::TaskStarted {
            task_id,
            agent_kind,
        });

        let inner_clone = Arc::clone(&inner);
        tokio::spawn(async move {
            let run_result = agent.run(task_node, agent_context).await;
            apply_dispatch_result(run_result, task_id, &inner_clone).await;
        });
    }
}

async fn apply_dispatch_result(
    run_result: Result<TaskResult, yantra_agents::AgentError>,
    task_id: TaskId,
    inner: &SchedulerInner,
) {
    match run_result {
        Ok(task_result) if task_result.outcome == Outcome::Success => {
            inner.circuit_breaker.probe_succeeded();
            if let Err(dag_error) = inner.dag.mark_complete(task_id) {
                tracing::error!(?dag_error, %task_id, "mark_complete failed");
            }
            inner.retry_counts.lock().remove(&task_id);
            let _ = inner.event_bus.publish(AgentMessage::TaskCompleted {
                task_id,
                decision_id: task_result.decision_id,
            });
        }
        Ok(task_result) => {
            inner.circuit_breaker.probe_failed();
            apply_failure(
                task_id,
                &format!("non-success outcome: {:?}", task_result.outcome),
                inner,
            );
        }
        Err(agent_error) => {
            inner.circuit_breaker.probe_failed();
            apply_failure(task_id, &agent_error.to_string(), inner);
        }
    }
}

fn apply_failure(task_id: TaskId, reason: &str, inner: &SchedulerInner) {
    let next_retry_count = {
        let mut retry_guard = inner.retry_counts.lock();
        let current_count = retry_guard.entry(task_id).or_insert(0);
        *current_count += 1;
        *current_count
    };

    if next_retry_count >= MAX_RETRY_COUNT {
        inner.retry_counts.lock().remove(&task_id);
        if let Err(dag_error) = inner.dag.mark_failed(task_id) {
            tracing::error!(?dag_error, %task_id, "mark_failed failed");
        }
        let _ = inner
            .event_bus
            .publish(AgentMessage::TaskMaxRetries { task_id });
        let _ = inner.human_review_sender.send(task_id);
        tracing::warn!(%task_id, "task sent to human review after max retries");
    } else {
        if let Err(dag_error) = inner.dag.requeue_for_retry(task_id) {
            tracing::error!(?dag_error, %task_id, "requeue_for_retry failed");
        }
        let _ = inner.event_bus.publish(AgentMessage::TaskFailed {
            task_id,
            reason: reason.to_owned(),
            retry_count: next_retry_count,
        });
        tracing::info!(%task_id, next_retry_count, "task requeued for retry");
    }
}
