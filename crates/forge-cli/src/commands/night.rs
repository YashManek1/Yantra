//! # forge-cli: `yantra night` Command
//!
//! Runs the autonomous Night Mode pipeline: Twilight (front-load STVP for all
//! planned tasks, collect decision rules), Night Run (execute the approved DAG
//! without approval gates, checkpoint every 30 s), and Dawn Digest (write
//! `dawn_digest.md` to the project root).
//!
//! The `--dry-run` flag uses the test executor (`DryRunExecutor`) instead of
//! real agents, so the pipeline can be exercised without a live LLM. Tasks are
//! sourced from the `--tasks` argument list or gathered interactively when none
//! are provided.
//!
//! ## Input
//! - `task_descriptions: Vec<String>` — task descriptions (can be empty for interactive mode)
//! - `dry_run: bool` — when `true`, skip LLM calls; use a no-op executor
//! - `router: Arc<Router>` — pre-built model router (Tier 0/1/2/3)
//! - `project_root: ProjectRoot` — workspace root
//!
//! ## Output
//! - `dawn_digest.md` written to the project root on success
//! - Exit summary printed to stdout
//!
//! ## Related
//! - `forge-night::twilight` — Twilight phase; collects STVP truths + decision rules
//! - `forge-night::night_run` — Night Run loop; dispatches tasks via the executor
//! - `forge-cli::commands::agent_runtime` — shared scheduler constructor

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use yantra_core::{DecisionId, ProjectRoot, SessionId, TaskId};
use yantra_night::{
    run_night, run_twilight, DecisionRule, HaltReason, NightError, TaskDisposition, TaskExecutor,
    TaskOutcome, TaskSpec, TwilightUi,
};
use yantra_router::Router;
use yantra_stvp::{Question, QuestionnaireUi, StvpError};

/// `TwilightUi` + `QuestionnaireUi` implementation for the CLI that uses
/// `inquire` to drive STVP questionnaires, decision-rule collection, and plan
/// confirmation.
struct CliTwilightUi;

impl QuestionnaireUi for CliTwilightUi {
    fn prompt(&self, question: &Question) -> Result<String, StvpError> {
        let mut text_prompt = inquire::Text::new(&question.text);
        if let Some(ref placeholder) = question.suggested_answer {
            text_prompt = text_prompt.with_placeholder(placeholder);
        }
        if let Some(ref help) = question.help_text {
            text_prompt = text_prompt.with_help_message(help);
        }
        text_prompt
            .prompt()
            .map_err(|_| StvpError::QuestionnaireAborted)
    }
}

impl TwilightUi for CliTwilightUi {
    fn collect_decision_rules(&self) -> Result<Vec<DecisionRule>, NightError> {
        Ok(Vec::new())
    }

    fn confirm_night_plan(&self, plan_markdown: &str) -> Result<bool, NightError> {
        println!("\n{plan_markdown}");
        let confirmed = inquire::Confirm::new("Proceed with Night Run?")
            .with_default(true)
            .prompt()
            .unwrap_or(false);
        Ok(confirmed)
    }
}

/// Production `TaskExecutor` that dispatches each Night Run task through the
/// shared multi-agent `Scheduler` (Researcher → Coder → Verifier → RedTeam →
/// Committer).
struct ProductionTaskExecutor {
    project_root: PathBuf,
    yantra_dir: PathBuf,
    router: Arc<Router>,
}

#[async_trait]
impl TaskExecutor for ProductionTaskExecutor {
    async fn execute_task(
        &self,
        task_id: TaskId,
        description: &str,
        _session_id: SessionId,
    ) -> TaskOutcome {
        let decision_id = DecisionId::new();
        match self.dispatch_via_scheduler(task_id, description).await {
            Ok(summary) => TaskOutcome {
                task_id,
                disposition: TaskDisposition::Completed,
                cost_usd: 0.0,
                decision_id,
                summary,
            },
            Err(execution_error) => {
                tracing::error!(
                    task_id = %task_id,
                    error = %execution_error,
                    "night executor: task failed"
                );
                TaskOutcome {
                    task_id,
                    disposition: TaskDisposition::Failed {
                        reason: execution_error.to_string(),
                        retry_count: 0,
                    },
                    cost_usd: 0.0,
                    decision_id,
                    summary: format!("failed: {execution_error}"),
                }
            }
        }
    }
}

impl ProductionTaskExecutor {
    async fn dispatch_via_scheduler(
        &self,
        task_id: TaskId,
        description: &str,
    ) -> anyhow::Result<String> {
        use yantra_core::{AgentKind, ProjectRoot, TaskClass, TaskNode, TaskStatus};

        let dag_session_id = SessionId::from_uuid(task_id.as_uuid());
        let night_project_root = ProjectRoot::new(self.project_root.clone())
            .context("failed to resolve project root for night task")?;
        let scheduler = super::agent_runtime::build_scheduler(
            &night_project_root,
            &self.yantra_dir,
            dag_session_id,
            self.router.clone(),
        )
        .context("failed to build agent scheduler for night task")?;

        let researcher_task_id = TaskId::new();
        let coder_task_id = TaskId::new();
        let verifier_task_id = TaskId::new();
        let red_team_task_id = TaskId::new();
        let committer_task_id = TaskId::new();
        let night_task_class = TaskClass::BugFix;

        let task_nodes = [
            TaskNode {
                id: researcher_task_id,
                description: format!("Research implementation details for: {description}"),
                status: TaskStatus::Pending,
                class: night_task_class,
                dependencies: Vec::new(),
                assigned_agent: Some(AgentKind::Researcher),
                truth_token: None,
                parent_decision_id: None,
            },
            TaskNode {
                id: coder_task_id,
                description: description.to_owned(),
                status: TaskStatus::Pending,
                class: night_task_class,
                dependencies: vec![researcher_task_id],
                assigned_agent: Some(AgentKind::Coder),
                truth_token: None,
                parent_decision_id: None,
            },
            TaskNode {
                id: verifier_task_id,
                description: format!("Verify changes from coder task {coder_task_id}"),
                status: TaskStatus::Pending,
                class: night_task_class,
                dependencies: vec![coder_task_id],
                assigned_agent: Some(AgentKind::IntegrityChecker),
                truth_token: None,
                parent_decision_id: None,
            },
            TaskNode {
                id: red_team_task_id,
                description: format!("Security audit for coder task {coder_task_id}"),
                status: TaskStatus::Pending,
                class: night_task_class,
                dependencies: vec![verifier_task_id],
                assigned_agent: Some(AgentKind::RedTeam),
                truth_token: None,
                parent_decision_id: None,
            },
            TaskNode {
                id: committer_task_id,
                description: format!("Commit verified changes from coder task {coder_task_id}"),
                status: TaskStatus::Pending,
                class: night_task_class,
                dependencies: vec![red_team_task_id],
                assigned_agent: Some(AgentKind::Committer),
                truth_token: None,
                parent_decision_id: None,
            },
        ];

        for task_node in task_nodes {
            scheduler
                .register_task(task_node)
                .context("failed to register night sub-task in scheduler")?;
        }

        let scheduler_results = scheduler
            .run_to_completion()
            .await
            .context("night task scheduler run failed")?;

        let all_completed = scheduler_results
            .iter()
            .all(|result| !result.summary.starts_with("failed"));

        if all_completed {
            Ok(format!("completed: {description}"))
        } else {
            anyhow::bail!("one or more sub-tasks failed for night task: {description}")
        }
    }
}

/// No-op `TaskExecutor` for `--dry-run`. Returns immediate success without LLM calls.
struct DryRunExecutor;

#[async_trait]
impl TaskExecutor for DryRunExecutor {
    async fn execute_task(
        &self,
        task_id: TaskId,
        description: &str,
        _session_id: SessionId,
    ) -> TaskOutcome {
        tracing::info!(task_id = %task_id, "dry-run executor: simulating task completion");
        TaskOutcome {
            task_id,
            disposition: TaskDisposition::Completed,
            cost_usd: 0.0,
            decision_id: DecisionId::new(),
            summary: format!("dry-run completed: {description}"),
        }
    }
}

/// Executes the full `yantra night` pipeline.
///
/// # Errors
///
/// Returns `anyhow::Error` on STVP failure, DAG failure, or I/O failure when
/// writing the Dawn Digest.
pub async fn night_command(
    task_descriptions: Vec<String>,
    dry_run: bool,
    router: Arc<Router>,
    project_root: ProjectRoot,
) -> anyhow::Result<()> {
    let yantra_dir: PathBuf = project_root.as_path().join(".yantra");
    std::fs::create_dir_all(&yantra_dir).context("failed to create .yantra directory")?;

    let signing_key = yantra_stvp::SigningKey::load_or_generate(&yantra_dir)
        .context("failed to load or generate STVP signing key")?;

    let task_specs: Vec<TaskSpec> = if task_descriptions.is_empty() {
        println!("No tasks specified. Enter tasks interactively (blank line to finish):");
        let mut gathered_specs = Vec::new();
        loop {
            let task_description = inquire::Text::new("Task (blank to finish):")
                .prompt()
                .unwrap_or_default();
            if task_description.trim().is_empty() {
                break;
            }
            gathered_specs.push(TaskSpec::new(task_description));
        }
        if gathered_specs.is_empty() {
            anyhow::bail!("no tasks provided; Night Mode requires at least one task");
        }
        gathered_specs
    } else {
        task_descriptions.into_iter().map(TaskSpec::new).collect()
    };

    println!(
        "▸ Twilight — validating {} task(s) via STVP...",
        task_specs.len()
    );

    let twilight_ui = CliTwilightUi;
    let night_session = run_twilight(
        task_specs,
        project_root.clone(),
        &signing_key,
        &twilight_ui,
        yantra_night::NIGHT_POLICY,
    )
    .await
    .context("Twilight phase failed")?;

    println!(
        "✓ Twilight complete — {} task(s) authorised.",
        night_session.validated_goals.len()
    );

    let task_executor: Arc<dyn TaskExecutor> = if dry_run {
        println!("(dry-run: no LLM calls)");
        Arc::new(DryRunExecutor)
    } else {
        Arc::new(ProductionTaskExecutor {
            project_root: project_root.as_path().to_path_buf(),
            yantra_dir: yantra_dir.clone(),
            router,
        })
    };

    println!("▸ Night Run starting...");
    let night_report = run_night(
        &night_session,
        &yantra_dir,
        project_root.as_path(),
        task_executor,
    )
    .await
    .context("Night Run failed")?;

    let digest_path = project_root.as_path().join("dawn_digest.md");
    std::fs::write(&digest_path, &night_report.dawn_digest_markdown)
        .context("failed to write dawn_digest.md")?;

    println!("\n✓ Night Run complete.");
    println!(
        "  Completed: {}  Deferred: {}  Failed: {}  Cost: ${:.4}",
        night_report.completed_task_ids.len(),
        night_report.deferred_task_ids.len(),
        night_report.failed_task_ids.len(),
        night_report.total_cost_usd,
    );
    println!("  Dawn Digest → {}", digest_path.display());

    match night_report.halt_reason {
        HaltReason::AllTasksComplete => {}
        HaltReason::BudgetExceeded { total_usd } => {
            println!("  Halt: budget exceeded (${total_usd:.4})");
        }
        HaltReason::CircuitOpen => {
            println!("  Halt: circuit breaker opened");
        }
        HaltReason::WatchdogDead => {
            println!("  Halt: watchdog channel closed");
        }
        HaltReason::HardStop { notify } => {
            println!("  Halt: hard stop (notify={notify})");
        }
    }

    Ok(())
}
