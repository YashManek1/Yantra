//! # forge-orchestrator: Cognitive DAG Scheduler and Debate Engine
//!
//! Accepts a validated `TruthToken` and schedules a directed acyclic graph of
//! agent tasks, resolving cross-agent conflicts via the Debate Engine and
//! optimising task ordering with the CSP Planner. The Speculation Engine
//! pre-fetches likely subtasks to reduce latency.
//!
//! ## Input
//! - `TruthToken` from `forge-stvp` (task scheduling is gated on this)
//! - `AgentEvent` messages from the multi-agent event bus
//! - Heartbeat signals from `forge-obs` watchdog
//!
//! ## Output
//! - `AgentTask` structs dispatched to specialist agents in `forge-agents`
//! - `DecisionRecord` entries written to `decisions.sqlite`
//! - Live DAG state streamed to `forge-serve` for the Live Canvas
//!
//! ## Related
//! - `forge-stvp` — provides the `TruthToken`; no task runs without one
//! - `forge-agents` — receives and executes dispatched tasks
//! - `forge-obs` — every DAG node is an OTel span; watchdog monitors health

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use yantra_core::TaskNode;
use yantra_stvp::verify_token;

pub mod error;

pub use error::SchedulingError;

/// Orchestrates task execution behind an STVP token gate.
///
/// Every call to `schedule_task` verifies the task's `TruthToken` signature
/// against the session public key before enqueuing the task. Tasks without a
/// token or with an invalid signature are rejected immediately.
///
/// Full DAG scheduling, the Debate Engine, and the CSP Planner are added in
/// the Day 4 wave.
pub struct Orchestrator {
    yantra_dir: PathBuf,
    queue: Mutex<Vec<TaskNode>>,
}

impl Orchestrator {
    /// Creates an orchestrator that verifies tokens against keys in `yantra_dir`.
    ///
    /// `yantra_dir` must contain `session.pub` — the Ed25519 public key written
    /// by `forge-stvp::SigningKey::save`. Typically this is
    /// `<project_root>/.yantra/`.
    pub fn new(yantra_dir: impl AsRef<Path>) -> Self {
        Self {
            yantra_dir: yantra_dir.as_ref().to_owned(),
            queue: Mutex::new(Vec::new()),
        }
    }

    /// Verifies the task's truth token and enqueues the task if valid.
    ///
    /// # Errors
    ///
    /// - `SchedulingError::MissingTruthToken` — task has no token
    /// - `SchedulingError::InvalidTruthToken` — token signature is wrong
    /// - `SchedulingError::TokenVerificationFailed` — key file I/O error
    pub fn schedule_task(&self, task: TaskNode) -> Result<(), SchedulingError> {
        let token =
            task.truth_token
                .as_ref()
                .ok_or_else(|| SchedulingError::MissingTruthToken {
                    task_id: task.id.to_string(),
                })?;

        let is_valid = verify_token(token, &self.yantra_dir).map_err(|source| {
            SchedulingError::TokenVerificationFailed {
                task_id: task.id.to_string(),
                source,
            }
        })?;

        if !is_valid {
            return Err(SchedulingError::InvalidTruthToken {
                task_id: task.id.to_string(),
            });
        }

        let mut queue_guard = self.queue.lock().unwrap();
        let task_id_string = task.id.to_string();
        queue_guard.push(task);

        tracing::info!(task_id = %task_id_string, "task scheduled");

        Ok(())
    }

    /// Returns a snapshot of all tasks currently in the queue.
    pub fn queued_tasks(&self) -> Vec<TaskNode> {
        self.queue.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{
        AgentKind, Strictness, TaskClass, TaskId, TaskNode, TaskStatus, TruthToken,
    };
    use yantra_stvp::{issue_token, SigningKey, SourceTruth};

    use super::*;

    fn temp_yantra_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yantra-orch-test-{}", TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_truth(task_id: TaskId) -> SourceTruth {
        SourceTruth {
            task_id,
            task_class: TaskClass::NewFeature,
            strictness: Strictness::Strict,
            description: "add JWT rotation".to_owned(),
            created_at: Utc::now(),
            answers: BTreeMap::new(),
        }
    }

    fn task_node_without_token(task_id: TaskId) -> TaskNode {
        TaskNode {
            id: task_id,
            description: "add JWT rotation".to_owned(),
            status: TaskStatus::Pending,
            class: TaskClass::NewFeature,
            dependencies: Vec::new(),
            assigned_agent: Some(AgentKind::Coder),
            truth_token: None,
            parent_decision_id: None,
        }
    }

    fn task_node_with_token(task_id: TaskId, token: TruthToken) -> TaskNode {
        TaskNode {
            id: task_id,
            description: "add JWT rotation".to_owned(),
            status: TaskStatus::Pending,
            class: TaskClass::NewFeature,
            dependencies: Vec::new(),
            assigned_agent: Some(AgentKind::Coder),
            truth_token: Some(token),
            parent_decision_id: None,
        }
    }

    #[test]
    fn schedule_task_without_token_returns_missing_error() {
        let yantra_dir = temp_yantra_dir();
        let orchestrator = Orchestrator::new(&yantra_dir);
        let task_id = TaskId::new();
        let task = task_node_without_token(task_id);

        let error = orchestrator.schedule_task(task).unwrap_err();
        assert!(
            matches!(error, SchedulingError::MissingTruthToken { .. }),
            "expected MissingTruthToken, got: {error:?}"
        );
    }

    #[test]
    fn schedule_task_with_valid_token_succeeds() {
        let yantra_dir = temp_yantra_dir();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).expect("key generated");
        let task_id = TaskId::new();
        let truth = sample_truth(task_id);
        let token = issue_token(&truth, &signing_key).expect("token issued");

        let orchestrator = Orchestrator::new(&yantra_dir);
        let task = task_node_with_token(task_id, token);
        orchestrator
            .schedule_task(task)
            .expect("scheduling must succeed");

        let queued = orchestrator.queued_tasks();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, task_id);
    }

    #[test]
    fn schedule_task_with_invalid_signature_returns_error() {
        let yantra_dir_a = temp_yantra_dir();
        let yantra_dir_b = temp_yantra_dir();

        let signing_key_a = SigningKey::load_or_generate(&yantra_dir_a).expect("key A generated");
        SigningKey::load_or_generate(&yantra_dir_b).expect("key B generated");

        let task_id = TaskId::new();
        let truth = sample_truth(task_id);
        let token = issue_token(&truth, &signing_key_a).expect("token issued with key A");

        let orchestrator = Orchestrator::new(&yantra_dir_b);
        let task = task_node_with_token(task_id, token);
        let error = orchestrator.schedule_task(task).unwrap_err();
        assert!(
            matches!(error, SchedulingError::InvalidTruthToken { .. }),
            "expected InvalidTruthToken, got: {error:?}"
        );
    }

    #[test]
    fn multiple_valid_tasks_are_all_queued() {
        let yantra_dir = temp_yantra_dir();
        let signing_key = SigningKey::load_or_generate(&yantra_dir).expect("key generated");
        let orchestrator = Orchestrator::new(&yantra_dir);

        for _ in 0..3 {
            let task_id = TaskId::new();
            let truth = sample_truth(task_id);
            let token = issue_token(&truth, &signing_key).expect("token issued");
            orchestrator
                .schedule_task(task_node_with_token(task_id, token))
                .expect("scheduling succeeds");
        }

        assert_eq!(orchestrator.queued_tasks().len(), 3);
    }
}
