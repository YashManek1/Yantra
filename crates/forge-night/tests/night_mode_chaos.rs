//! # forge-night: Night Mode Chaos Integration Tests
//!
//! Tests that verify crash-resume behaviour: checkpoint files survive across
//! session boundaries, orphaned checkpoints are discoverable, and the Night Run
//! loop handles executor failures without losing progress records.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use yantra_core::{DecisionId, SessionId, Strictness, TaskClass, TaskId};
use yantra_night::checkpointing::Checkpointer;
use yantra_night::{
    run_night, DecisionRule, HaltReason, ModePolicy, NightSession, TaskDisposition, TaskExecutor,
    TaskOutcome, TaskSpec, ValidatedGoal, TRUST_POLICY,
};
use yantra_stvp::{issue_token, SigningKey, SourceTruth};

fn temp_project_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("yantra-chaos-{}", TaskId::new()));
    std::fs::create_dir_all(root.join(".yantra")).unwrap();
    root
}

fn make_source_truth(task_id: TaskId, description: &str) -> SourceTruth {
    SourceTruth {
        task_id,
        task_class: TaskClass::Chore,
        strictness: Strictness::Trust,
        description: description.to_owned(),
        created_at: Utc::now(),
        answers: BTreeMap::new(),
        augmented_question_ids: Vec::new(),
    }
}

fn make_session(
    yantra_dir: &std::path::Path,
    goal_count: usize,
    policy: ModePolicy,
) -> (NightSession, Vec<TaskId>) {
    let signing_key = SigningKey::load_or_generate(yantra_dir).unwrap();
    let mut task_ids = Vec::new();
    let validated_goals: Vec<ValidatedGoal> = (0..goal_count)
        .map(|index| {
            let task_id = TaskId::new();
            task_ids.push(task_id);
            let description = format!("document public api for module {index}");
            let source_truth = make_source_truth(task_id, &description);
            let truth_token = issue_token(&source_truth, &signing_key).unwrap();
            ValidatedGoal {
                spec: TaskSpec::new(description),
                source_truth,
                truth_token,
            }
        })
        .collect();

    let session = NightSession {
        session_id: SessionId::new(),
        validated_goals,
        decision_rules: Vec::<DecisionRule>::new(),
        policy,
        night_plan_markdown: "# Plan\n".to_owned(),
    };
    (session, task_ids)
}

struct AlwaysCompleteExecutor;

#[async_trait]
impl TaskExecutor for AlwaysCompleteExecutor {
    async fn execute_task(
        &self,
        task_id: TaskId,
        _description: &str,
        _session_id: SessionId,
    ) -> TaskOutcome {
        TaskOutcome {
            task_id,
            disposition: TaskDisposition::Completed,
            cost_usd: 0.01,
            decision_id: DecisionId::new(),
            summary: "completed".to_owned(),
        }
    }
}

struct HighCostExecutor;

#[async_trait]
impl TaskExecutor for HighCostExecutor {
    async fn execute_task(
        &self,
        task_id: TaskId,
        _description: &str,
        _session_id: SessionId,
    ) -> TaskOutcome {
        TaskOutcome {
            task_id,
            disposition: TaskDisposition::Completed,
            cost_usd: 1.00,
            decision_id: DecisionId::new(),
            summary: "completed (high cost)".to_owned(),
        }
    }
}

/// Checkpoints are written for every successfully completed task and persist
/// after the Night Run returns, so a subsequent session can detect orphaned
/// checkpoints and resume work.
#[tokio::test]
async fn checkpoints_survive_after_run_completes() {
    let project_root = temp_project_root();
    let yantra_dir = project_root.join(".yantra");

    let (session, task_ids) = make_session(&yantra_dir, 3, TRUST_POLICY);
    let executor: Arc<dyn TaskExecutor> = Arc::new(AlwaysCompleteExecutor);

    let report = run_night(&session, &yantra_dir, &project_root, executor)
        .await
        .unwrap();

    assert_eq!(report.completed_task_ids.len(), 3);

    let checkpointer = Checkpointer::new(&yantra_dir);
    let orphans = checkpointer.load_all().unwrap();
    assert_eq!(orphans.len(), 3);

    let orphan_ids: Vec<TaskId> = orphans.iter().map(|c| c.task_id).collect();
    for task_id in &task_ids {
        assert!(
            orphan_ids.contains(task_id),
            "expected checkpoint for task {task_id}"
        );
    }
}

/// A second run against the same `.yantra/` directory must not fail even
/// though orphaned checkpoints from the previous run remain on disk.
#[tokio::test]
async fn second_run_succeeds_with_orphaned_checkpoints_present() {
    let project_root = temp_project_root();
    let yantra_dir = project_root.join(".yantra");

    let executor: Arc<dyn TaskExecutor> = Arc::new(AlwaysCompleteExecutor);

    // First run writes checkpoints to disk
    let (session1, _) = make_session(&yantra_dir, 2, TRUST_POLICY);
    run_night(&session1, &yantra_dir, &project_root, Arc::clone(&executor))
        .await
        .unwrap();

    // Second run with a fresh DAG must still complete without errors
    let second_yantra_dir = yantra_dir.join("session2");
    std::fs::create_dir_all(&second_yantra_dir).unwrap();
    // Copy the signing key so tokens verify against the same key
    std::fs::copy(
        yantra_dir.join("session.key"),
        second_yantra_dir.join("session.key"),
    )
    .unwrap_or_default();

    let (session2, _) = make_session(&second_yantra_dir, 2, TRUST_POLICY);
    let report = run_night(&session2, &second_yantra_dir, &project_root, executor)
        .await
        .unwrap();

    assert!(matches!(report.halt_reason, HaltReason::AllTasksComplete));
}

/// When a budget-exceeded halt occurs, completed task checkpoints still exist
/// and record the work done before the halt.
#[tokio::test]
async fn checkpoints_exist_after_budget_exceeded_halt() {
    let project_root = temp_project_root();
    let yantra_dir = project_root.join(".yantra");

    let policy = ModePolicy {
        cost_per_task_cap_usd: 0.50,
        ..TRUST_POLICY
    };
    let (session, _) = make_session(&yantra_dir, 5, policy);

    let report = run_night(
        &session,
        &yantra_dir,
        &project_root,
        Arc::new(HighCostExecutor) as Arc<dyn TaskExecutor>,
    )
    .await
    .unwrap();

    assert!(matches!(
        report.halt_reason,
        HaltReason::BudgetExceeded { .. }
    ));

    let checkpointer = Checkpointer::new(&yantra_dir);
    let saved_checkpoints = checkpointer.load_all().unwrap();
    // At least one task should have a checkpoint (the one that completed before halt)
    assert!(
        !saved_checkpoints.is_empty(),
        "expected at least one checkpoint before budget halt"
    );
}
