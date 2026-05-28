//! # forge-night: Autonomous Night Run Loop
//!
//! Executes the task graph produced by the Twilight phase without human
//! intervention. Runs until all ready tasks are exhausted, the cost budget is
//! exceeded, the circuit breaker opens, or a decision rule fires a hard stop.
//!
//! ## Input
//! - `NightSession` from `run_twilight` (validated goals, signed tokens, rules, policy)
//! - `TaskExecutor` — injects actual agent execution; real or stub for testing
//! - `yantra_dir` — `.yantra/` directory hosting `dag.sqlite` and checkpoints
//! - `project_root` — repository root used for git branch creation
//!
//! ## Output
//! - `NightReport` summarising completed, deferred, and failed tasks, total cost,
//!   run duration, halt reason, and a ready-to-render Dawn Digest
//!
//! ## Related
//! - `forge-night::twilight` — produces the `NightSession` consumed here
//! - `forge-night::checkpointing` — persists task checkpoints after completion
//! - `forge-night::night_branch` — creates per-task git branches
//! - `forge-orchestrator::dag` — SQLite task DAG backing this loop
//! - `forge-orchestrator::circuit_breaker` — rate guard for LLM dispatch

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::future::join_all;
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};
use yantra_core::{hex_encode, AgentKind, DecisionId, SessionId, TaskId, TaskNode, TaskStatus};
use yantra_obs::{WatchdogClient, WatchdogMessage, SILENCE_KILL_THRESHOLD};
use yantra_orchestrator::{CircuitBreaker, TaskDag};

use crate::checkpointing::{Checkpointer, TaskCheckpoint};
use crate::decision_rules::{NightEvent, RuleAction, RuleEngine};
use crate::error::NightError;
use crate::night_branch::{create_night_branch, format_dawn_digest_branch_section, NightBranchRef};
use crate::twilight::{NightSession, ValidatedGoal};

/// Abstraction over agent task execution, injected into `run_night`.
///
/// The production implementation dispatches real agents through the router.
/// Test implementations return synthetic outcomes without LLM calls.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Executes `task_id` and returns a disposition, cost, and one-line summary.
    async fn execute_task(
        &self,
        task_id: TaskId,
        description: &str,
        session_id: SessionId,
    ) -> TaskOutcome;
}

/// How a single task resolved at the end of its execution.
#[derive(Debug, Clone)]
pub enum TaskDisposition {
    /// Task completed successfully — diff verified, tests pass.
    Completed,
    /// Task failed after the given number of retries.
    Failed {
        /// Human-readable explanation of the failure.
        reason: String,
        /// Number of retry attempts already made.
        retry_count: u8,
    },
    /// Task was deferred by a decision rule or low confidence score.
    Deferred {
        /// Human-readable reason for the deferral.
        reason: String,
    },
    /// Task was halted by a safety condition (e.g. security issue detected).
    Halted,
}

/// The result of executing one task during the Night Run.
#[derive(Debug, Clone)]
pub struct TaskOutcome {
    /// Task that was executed.
    pub task_id: TaskId,
    /// How the task resolved.
    pub disposition: TaskDisposition,
    /// Estimated USD cost of the execution.
    pub cost_usd: f32,
    /// Decision identifier written to archaeology records.
    pub decision_id: DecisionId,
    /// One-line human-readable summary of the outcome.
    pub summary: String,
}

/// Why the Night Run loop terminated.
#[derive(Debug, Clone)]
pub enum HaltReason {
    /// All ready tasks ran to completion; the DAG is exhausted.
    AllTasksComplete,
    /// Cumulative task cost crossed the policy ceiling.
    BudgetExceeded {
        /// Total cost in USD when the ceiling was hit.
        total_usd: f32,
    },
    /// The circuit breaker opened and no probe succeeded before session end.
    CircuitOpen,
    /// The watchdog channel closed or sent an explicit kill command.
    WatchdogDead,
    /// A decision rule fired a `HardStop` or `HaltAndDocument` action.
    HardStop {
        /// Whether to emit a push notification to the user.
        notify: bool,
    },
}

/// Summary produced at the end of a Night Run, used to render the Dawn Digest.
#[derive(Debug)]
pub struct NightReport {
    /// Session identifier carried from the `NightSession`.
    pub session_id: SessionId,
    /// Task IDs that completed successfully.
    pub completed_task_ids: Vec<TaskId>,
    /// Task IDs deferred by policy, low confidence, or a decision rule.
    pub deferred_task_ids: Vec<TaskId>,
    /// Task IDs that permanently failed.
    pub failed_task_ids: Vec<TaskId>,
    /// Total USD spent across all tasks.
    pub total_cost_usd: f32,
    /// Wall-clock duration of the Night Run.
    pub run_duration: Duration,
    /// Full Markdown content for the Dawn Digest.
    pub dawn_digest_markdown: String,
    /// Reason the loop terminated.
    pub halt_reason: HaltReason,
    /// Per-task retry counts: 0 for first-pass completions, >0 for tasks that
    /// reported internal retries before the outcome was recorded.
    pub task_retry_counts: HashMap<TaskId, u8>,
}

/// Internal disposition computed after applying decision rules to an outcome.
enum EffectiveAction {
    Complete,
    Fail,
    Defer,
    HardStop { notify: bool },
}

/// Runs the autonomous Night loop until all tasks complete, the budget is
/// exhausted, or a safety condition stops execution.
///
/// `executor` drives actual agent calls and is injected to allow testing
/// without real LLM infrastructure. `yantra_dir` hosts the task DAG and
/// checkpoints. `project_root` is the repository root for git branches.
///
/// # Errors
///
/// Returns `NightError::DagError` when the task DAG cannot be opened or
/// updated, and `NightError::CheckpointIo` on checkpoint persistence failure.
#[instrument(skip_all, fields(session_id = %session.session_id))]
pub async fn run_night(
    session: &NightSession,
    yantra_dir: &Path,
    project_root: &Path,
    executor: Arc<dyn TaskExecutor>,
) -> Result<NightReport, NightError> {
    let run_start = std::time::Instant::now();

    let description_map: HashMap<TaskId, String> = session
        .validated_goals
        .iter()
        .map(|goal| (goal.truth_token.task_id, goal.spec.description.clone()))
        .collect();

    let dag = Arc::new(
        TaskDag::open(yantra_dir)
            .map_err(|dag_error| NightError::DagError(dag_error.to_string()))?,
    );

    for goal in &session.validated_goals {
        let task_node = task_node_from_goal(goal);
        let signature_hex = hex_encode(&goal.truth_token.signature);
        dag.add_task(&task_node, &signature_hex)
            .map_err(|dag_error| NightError::DagError(dag_error.to_string()))?;
    }

    let circuit_breaker = CircuitBreaker::new(60);
    let checkpointer = Checkpointer::new(yantra_dir);

    let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel::<WatchdogMessage>(16);
    let watchdog_client = WatchdogClient::new(heartbeat_tx);
    let watchdog_killed = Arc::new(AtomicBool::new(false));

    {
        let kill_flag = Arc::clone(&watchdog_killed);
        tokio::spawn(async move {
            loop {
                match tokio::time::timeout(SILENCE_KILL_THRESHOLD, heartbeat_rx.recv()).await {
                    Ok(Some(WatchdogMessage::Heartbeat)) => {}
                    Ok(Some(WatchdogMessage::RequestSnapshot)) => {}
                    Ok(Some(WatchdogMessage::KillCommand) | None) | Err(_) => {
                        kill_flag.store(true, Ordering::Release);
                        break;
                    }
                }
            }
        });
    }

    let mut completed_task_ids: Vec<TaskId> = Vec::new();
    let mut deferred_task_ids: Vec<TaskId> = Vec::new();
    let mut failed_task_ids: Vec<TaskId> = Vec::new();
    let mut task_retry_counts: HashMap<TaskId, u8> = HashMap::new();
    let mut total_cost_usd: f32 = 0.0;
    let mut halt_reason = HaltReason::AllTasksComplete;
    let mut night_branches: Vec<(NightBranchRef, String)> = Vec::new();

    'night_loop: loop {
        if watchdog_killed.load(Ordering::Acquire) {
            halt_reason = HaltReason::WatchdogDead;
            break;
        }

        let ready_task_ids = dag
            .ready_tasks()
            .map_err(|dag_error| NightError::DagError(dag_error.to_string()))?;

        if ready_task_ids.is_empty() {
            break;
        }

        let mut breaker_open = false;
        let handles: Vec<_> = ready_task_ids
            .iter()
            .filter_map(|&task_id| {
                if breaker_open {
                    return None;
                }
                let claimed = dag.try_mark_running(task_id).unwrap_or(false);
                if !claimed {
                    return None;
                }
                if !circuit_breaker.try_dispatch() {
                    breaker_open = true;
                    let _ = dag.requeue_for_retry(task_id);
                    return None;
                }
                let description = description_map.get(&task_id)?.clone();
                let executor_ref = Arc::clone(&executor);
                let session_id = session.session_id;
                Some(tokio::spawn(async move {
                    executor_ref
                        .execute_task(task_id, &description, session_id)
                        .await
                }))
            })
            .collect();

        if breaker_open && handles.is_empty() {
            warn!("circuit breaker open; pausing dispatch for 5 seconds");
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        let outcomes: Vec<TaskOutcome> = join_all(handles)
            .await
            .into_iter()
            .filter_map(std::result::Result::ok)
            .collect();

        for outcome in outcomes {
            total_cost_usd += outcome.cost_usd;

            let night_event = derive_night_event(&outcome);
            let rule_action = night_event
                .as_ref()
                .and_then(|event| RuleEngine::resolve(event, &session.decision_rules));
            let effective_action = determine_effective_action(rule_action.as_ref(), &outcome);

            match effective_action {
                EffectiveAction::Complete => {
                    dag.mark_complete(outcome.task_id)
                        .map_err(|dag_error| NightError::DagError(dag_error.to_string()))?;

                    if let Ok(branch_ref) = create_night_branch(project_root, outcome.task_id) {
                        let task_description = description_map
                            .get(&outcome.task_id)
                            .cloned()
                            .unwrap_or_default();
                        night_branches.push((branch_ref, task_description));
                    }

                    let checkpoint = TaskCheckpoint::completed(outcome.task_id, &outcome.summary);
                    let _ = checkpointer.save(&checkpoint);

                    task_retry_counts.insert(outcome.task_id, 0);
                    completed_task_ids.push(outcome.task_id);
                    info!(task_id = %outcome.task_id, summary = %outcome.summary, "task completed");
                }
                EffectiveAction::Fail => {
                    dag.mark_failed(outcome.task_id)
                        .map_err(|dag_error| NightError::DagError(dag_error.to_string()))?;
                    let retry_count_value = match &outcome.disposition {
                        TaskDisposition::Failed { retry_count, .. } => *retry_count,
                        _ => 0,
                    };
                    task_retry_counts.insert(outcome.task_id, retry_count_value);
                    failed_task_ids.push(outcome.task_id);
                    warn!(task_id = %outcome.task_id, summary = %outcome.summary, "task failed");
                }
                EffectiveAction::Defer => {
                    deferred_task_ids.push(outcome.task_id);
                    info!(task_id = %outcome.task_id, summary = %outcome.summary, "task deferred");
                }
                EffectiveAction::HardStop { notify } => {
                    halt_reason = HaltReason::HardStop { notify };
                    break 'night_loop;
                }
            }
        }

        if watchdog_client.heartbeat().await.is_err() {
            halt_reason = HaltReason::WatchdogDead;
            break;
        }

        if total_cost_usd >= session.policy.cost_per_task_cap_usd {
            halt_reason = HaltReason::BudgetExceeded {
                total_usd: total_cost_usd,
            };
            break;
        }
    }

    let run_duration = run_start.elapsed();
    let dawn_digest = render_dawn_digest(
        session.session_id,
        &completed_task_ids,
        &deferred_task_ids,
        &failed_task_ids,
        total_cost_usd,
        run_duration,
        &halt_reason,
        &night_branches,
        &description_map,
    );

    Ok(NightReport {
        session_id: session.session_id,
        completed_task_ids,
        deferred_task_ids,
        failed_task_ids,
        task_retry_counts,
        total_cost_usd,
        run_duration,
        dawn_digest_markdown: dawn_digest,
        halt_reason,
    })
}

fn task_node_from_goal(goal: &ValidatedGoal) -> TaskNode {
    TaskNode {
        id: goal.truth_token.task_id,
        description: goal.spec.description.clone(),
        status: TaskStatus::Pending,
        class: goal.source_truth.task_class,
        dependencies: Vec::new(),
        assigned_agent: Some(AgentKind::Coder),
        truth_token: Some(goal.truth_token.clone()),
        parent_decision_id: None,
    }
}

fn derive_night_event(outcome: &TaskOutcome) -> Option<NightEvent> {
    match &outcome.disposition {
        TaskDisposition::Failed { retry_count, .. } => Some(NightEvent::TestFailure {
            retry_count: *retry_count,
        }),
        TaskDisposition::Halted => Some(NightEvent::SecurityIssueDetected),
        TaskDisposition::Completed | TaskDisposition::Deferred { .. } => None,
    }
}

fn determine_effective_action(
    rule_action: Option<&RuleAction>,
    outcome: &TaskOutcome,
) -> EffectiveAction {
    match rule_action {
        Some(RuleAction::HardStop { notify }) => EffectiveAction::HardStop { notify: *notify },
        Some(RuleAction::HaltAndDocument) => EffectiveAction::HardStop { notify: false },
        Some(RuleAction::TagWipAndDefer | RuleAction::Wait { .. }) => EffectiveAction::Defer,
        Some(RuleAction::SkipTask | RuleAction::RebaseAttempt { .. }) => EffectiveAction::Fail,
        None => match &outcome.disposition {
            TaskDisposition::Completed => EffectiveAction::Complete,
            TaskDisposition::Failed { .. } => EffectiveAction::Fail,
            TaskDisposition::Deferred { .. } => EffectiveAction::Defer,
            TaskDisposition::Halted => EffectiveAction::HardStop { notify: false },
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn render_dawn_digest(
    session_id: SessionId,
    completed: &[TaskId],
    deferred: &[TaskId],
    failed: &[TaskId],
    total_cost_usd: f32,
    run_duration: Duration,
    halt_reason: &HaltReason,
    night_branches: &[(NightBranchRef, String)],
    description_map: &HashMap<TaskId, String>,
) -> String {
    let halt_description = match halt_reason {
        HaltReason::AllTasksComplete => "All tasks completed successfully.".to_owned(),
        HaltReason::BudgetExceeded { total_usd } => {
            format!("Budget cap exceeded at ${total_usd:.4} USD.")
        }
        HaltReason::CircuitOpen => "Circuit breaker opened — LLM rate limit hit.".to_owned(),
        HaltReason::WatchdogDead => "Watchdog channel closed unexpectedly.".to_owned(),
        HaltReason::HardStop { notify } => {
            format!(
                "Hard stop triggered{}.",
                if *notify { " (user notified)" } else { "" }
            )
        }
    };

    let duration_mins = run_duration.as_secs() / 60;
    let duration_secs = run_duration.as_secs() % 60;

    let mut digest = format!(
        "# Dawn Digest — Session {session_id}\n\n\
        **Halt reason:** {halt_description}\n\
        **Duration:** {duration_mins}m {duration_secs}s\n\
        **Total cost:** ${total_cost_usd:.4} USD\n\n\
        ## Summary\n\n\
        | Status    | Count |\n\
        |-----------|-------|\n\
        | Completed | {completed_count} |\n\
        | Deferred  | {deferred_count} |\n\
        | Failed    | {failed_count} |\n\n",
        completed_count = completed.len(),
        deferred_count = deferred.len(),
        failed_count = failed.len(),
    );

    if !completed.is_empty() {
        digest.push_str("## Completed Tasks\n\n");
        for task_id in completed {
            let description = description_map
                .get(task_id)
                .map_or("(unknown)", String::as_str);
            digest.push_str(&format!("- `{task_id}` — {description}\n"));
        }
        digest.push('\n');
    }

    if !deferred.is_empty() {
        digest.push_str("## Deferred Tasks\n\n");
        for task_id in deferred {
            let description = description_map
                .get(task_id)
                .map_or("(unknown)", String::as_str);
            digest.push_str(&format!("- `{task_id}` — {description}\n"));
        }
        digest.push('\n');
    }

    if !failed.is_empty() {
        digest.push_str("## Failed Tasks\n\n");
        for task_id in failed {
            let description = description_map
                .get(task_id)
                .map_or("(unknown)", String::as_str);
            digest.push_str(&format!("- `{task_id}` — {description}\n"));
        }
        digest.push('\n');
    }

    digest.push_str(&format_dawn_digest_branch_section(night_branches));

    digest
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Utc;
    use yantra_core::{SessionId, Strictness, TaskClass, TaskId};
    use yantra_stvp::{issue_token, SigningKey, SourceTruth};

    use super::*;
    use crate::mode_policy::TRUST_POLICY;
    use crate::twilight::{NightSession, TaskSpec, ValidatedGoal};

    fn temp_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
        let project_root =
            std::env::temp_dir().join(format!("yantra-night-run-test-{}", TaskId::new()));
        let yantra_dir = project_root.join(".yantra");
        std::fs::create_dir_all(&yantra_dir).unwrap();
        (project_root, yantra_dir)
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
        policy: crate::mode_policy::ModePolicy,
    ) -> NightSession {
        let signing_key = SigningKey::load_or_generate(yantra_dir).unwrap();
        let validated_goals: Vec<ValidatedGoal> = (0..goal_count)
            .map(|index| {
                let task_id = TaskId::new();
                let description = format!("document public api module {index}");
                let source_truth = make_source_truth(task_id, &description);
                let truth_token = issue_token(&source_truth, &signing_key).unwrap();
                ValidatedGoal {
                    spec: TaskSpec::new(description),
                    source_truth,
                    truth_token,
                }
            })
            .collect();

        NightSession {
            session_id: SessionId::new(),
            validated_goals,
            decision_rules: Vec::new(),
            policy,
            night_plan_markdown: "# Night Plan\n".to_owned(),
        }
    }

    struct AlwaysCompleteExecutor {
        cost_per_task: f32,
    }

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
                cost_usd: self.cost_per_task,
                decision_id: DecisionId::new(),
                summary: "completed successfully".to_owned(),
            }
        }
    }

    struct AlwaysFailExecutor;

    #[async_trait]
    impl TaskExecutor for AlwaysFailExecutor {
        async fn execute_task(
            &self,
            task_id: TaskId,
            _description: &str,
            _session_id: SessionId,
        ) -> TaskOutcome {
            TaskOutcome {
                task_id,
                disposition: TaskDisposition::Failed {
                    reason: "test agent failure".to_owned(),
                    retry_count: 3,
                },
                cost_usd: 0.01,
                decision_id: DecisionId::new(),
                summary: "failed after 3 retries".to_owned(),
            }
        }
    }

    #[tokio::test]
    async fn all_tasks_complete_when_executor_always_succeeds() {
        let (project_root, yantra_dir) = temp_dirs();
        let session = make_session(&yantra_dir, 3, TRUST_POLICY);
        let executor = Arc::new(AlwaysCompleteExecutor {
            cost_per_task: 0.01,
        });

        let report = run_night(&session, &yantra_dir, &project_root, executor)
            .await
            .unwrap();

        assert_eq!(report.completed_task_ids.len(), 3);
        assert_eq!(report.failed_task_ids.len(), 0);
        assert!(matches!(report.halt_reason, HaltReason::AllTasksComplete));
    }

    #[tokio::test]
    async fn budget_cap_halts_run_when_cost_exceeds_policy() {
        let (project_root, yantra_dir) = temp_dirs();
        let policy = crate::mode_policy::ModePolicy {
            cost_per_task_cap_usd: 0.50,
            ..TRUST_POLICY
        };
        let session = make_session(&yantra_dir, 5, policy);

        // Each task costs $1.00; first task takes total to $1.00 ≥ $0.50 cap
        let executor = Arc::new(AlwaysCompleteExecutor {
            cost_per_task: 1.00,
        });
        let report = run_night(&session, &yantra_dir, &project_root, executor)
            .await
            .unwrap();

        assert!(matches!(
            report.halt_reason,
            HaltReason::BudgetExceeded { .. }
        ));
        assert!(report.total_cost_usd >= 0.50);
    }

    #[tokio::test]
    async fn failed_tasks_are_recorded_separately_from_completed() {
        let (project_root, yantra_dir) = temp_dirs();
        let session = make_session(&yantra_dir, 2, TRUST_POLICY);
        let executor = Arc::new(AlwaysFailExecutor);

        let report = run_night(&session, &yantra_dir, &project_root, executor)
            .await
            .unwrap();

        assert_eq!(report.failed_task_ids.len(), 2);
        assert_eq!(report.completed_task_ids.len(), 0);
    }

    #[tokio::test]
    async fn soak_run_completes_many_tasks_with_compressed_time() {
        tokio::time::pause();

        let (project_root, yantra_dir) = temp_dirs();
        let session = make_session(&yantra_dir, 20, TRUST_POLICY);
        let executor = Arc::new(AlwaysCompleteExecutor {
            cost_per_task: 0.001,
        });

        let report = run_night(&session, &yantra_dir, &project_root, executor)
            .await
            .unwrap();

        assert_eq!(report.completed_task_ids.len(), 20);
        assert!(matches!(report.halt_reason, HaltReason::AllTasksComplete));

        tokio::time::resume();
    }

    #[tokio::test]
    async fn dawn_digest_contains_session_header_and_summary_table() {
        let (project_root, yantra_dir) = temp_dirs();
        let session = make_session(&yantra_dir, 1, TRUST_POLICY);
        let executor = Arc::new(AlwaysCompleteExecutor {
            cost_per_task: 0.01,
        });

        let report = run_night(&session, &yantra_dir, &project_root, executor)
            .await
            .unwrap();

        assert!(report.dawn_digest_markdown.contains("Dawn Digest"));
        assert!(report.dawn_digest_markdown.contains("Completed"));
        assert!(report.dawn_digest_markdown.contains("Total cost"));
    }

    #[tokio::test]
    async fn checkpoints_are_written_for_completed_tasks() {
        let (project_root, yantra_dir) = temp_dirs();
        let session = make_session(&yantra_dir, 3, TRUST_POLICY);
        let executor = Arc::new(AlwaysCompleteExecutor {
            cost_per_task: 0.01,
        });

        let report = run_night(&session, &yantra_dir, &project_root, executor)
            .await
            .unwrap();

        assert_eq!(report.completed_task_ids.len(), 3);

        let checkpointer = Checkpointer::new(&yantra_dir);
        let saved_checkpoints = checkpointer.load_all().unwrap();
        assert_eq!(saved_checkpoints.len(), 3);
    }
}
