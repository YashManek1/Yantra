//! # forge-night: End-to-End Night Mode Integration Test
//!
//! Validates the entire Night Mode pipeline including Twilight validation,
//! autonomous task execution under simulated time, postmortem heuristic analysis,
//! and Dawn Digest report generation.
//!
//! ## Input
//! - Simulated `NightSession` configuration with target task descriptions.
//! - Mock `TaskExecutor` mapping descriptions to success, failure, or deferral.
//!
//! ## Output
//! - Assertions verifying the structure and outputs of the digest, skills files,
//!   and postmortem metrics.
//!
//! ## Related
//! - `forge-night::night_run` — core execution loop.
//! - `forge-night::postmortem` — skill generation and mistake logging.
//! - `forge-night::dawn_digest` — markdown report generator.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use yantra_core::{DecisionId, SessionId, Strictness, TaskClass, TaskId};
use yantra_night::{
    generate_digest, run_night, run_postmortem, DecisionRule, DigestContext, DigestLearnings,
    HaltReason, NightSession, SkillAction, SkillUpdate, TaskDisposition, TaskExecutor, TaskOutcome,
    TaskPostmortemData, TaskSpec, ValidatedGoal, TRUST_POLICY,
};
use yantra_stvp::{issue_token, SigningKey, SourceTruth};

fn create_temporary_project_root() -> std::path::PathBuf {
    let project_root_path = std::env::temp_dir().join(format!("yantra-e2e-{}", TaskId::new()));
    std::fs::create_dir_all(project_root_path.join(".yantra")).unwrap();
    project_root_path
}

struct ScenarioExecutor;

#[async_trait]
impl TaskExecutor for ScenarioExecutor {
    async fn execute_task(
        &self,
        task_id: TaskId,
        description: &str,
        _session_id: SessionId,
    ) -> TaskOutcome {
        let (disposition, summary) = if description.contains("forge-core") {
            (
                TaskDisposition::Completed,
                "documenting forge-core completed".to_owned(),
            )
        } else if description.contains("tokenizer") {
            (
                TaskDisposition::Completed,
                "tokenizer tests completed".to_owned(),
            )
        } else if description.contains("orchestrator") {
            (
                TaskDisposition::Deferred {
                    reason: "scope too broad for a single night run".to_owned(),
                },
                "orchestrator refactor deferred".to_owned(),
            )
        } else {
            (TaskDisposition::Completed, "fallback complete".to_owned())
        };

        TaskOutcome {
            task_id,
            disposition,
            cost_usd: 0.01,
            decision_id: DecisionId::new(),
            summary,
        }
    }
}

#[tokio::test]
async fn three_task_night_run_two_completed_one_deferred() {
    let project_root_path = create_temporary_project_root();
    let yantra_directory_path = project_root_path.join(".yantra");
    let skills_directory_path = project_root_path.join("skills");
    let memory_db_path = yantra_directory_path.join("memory.sqlite");

    let signing_key = SigningKey::load_or_generate(&yantra_directory_path).unwrap();

    let descriptions = vec![
        "document the public API of the forge-core crate".to_owned(),
        "write unit tests for the tokenizer module".to_owned(),
        "refactor the entire orchestrator into microservices".to_owned(),
    ];

    let mut task_ids = Vec::new();
    let mut validated_goals = Vec::new();

    for description in descriptions {
        let task_id = TaskId::new();
        task_ids.push(task_id);
        let source_truth = SourceTruth {
            task_id,
            task_class: TaskClass::Chore,
            strictness: Strictness::Trust,
            description: description.clone(),
            created_at: Utc::now(),
            answers: BTreeMap::new(),
            augmented_question_ids: Vec::new(),
        };
        let truth_token = issue_token(&source_truth, &signing_key).unwrap();
        validated_goals.push(ValidatedGoal {
            spec: TaskSpec::new(description),
            source_truth,
            truth_token,
        });
    }

    let night_session = NightSession {
        session_id: SessionId::new(),
        validated_goals,
        decision_rules: Vec::<DecisionRule>::new(),
        policy: TRUST_POLICY,
        night_plan_markdown: "# Plan\n".to_owned(),
    };

    let task_executor = Arc::new(ScenarioExecutor);

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30 * 60)).await;
    let night_report = run_night(
        &night_session,
        &yantra_directory_path,
        &project_root_path,
        task_executor,
    )
    .await
    .unwrap();
    tokio::time::resume();

    assert_eq!(night_report.completed_task_ids.len(), 2);
    assert_eq!(night_report.deferred_task_ids.len(), 1);
    assert!(night_report.failed_task_ids.is_empty());
    assert!(matches!(
        night_report.halt_reason,
        HaltReason::AllTasksComplete
    ));

    let mut task_postmortem_records = Vec::new();
    for goal in &night_session.validated_goals {
        let task_id = goal.truth_token.task_id;
        let is_completed = night_report.completed_task_ids.contains(&task_id);
        let is_deferred = night_report.deferred_task_ids.contains(&task_id);
        if is_deferred {
            continue;
        }
        task_postmortem_records.push(TaskPostmortemData {
            task_id,
            description: goal.spec.description.clone(),
            task_class: goal.source_truth.task_class,
            retry_count: 0,
            completed: is_completed,
        });
    }

    let postmortem_outcome = run_postmortem(
        &task_postmortem_records,
        &skills_directory_path,
        &memory_db_path,
    )
    .unwrap();

    assert_eq!(postmortem_outcome.first_pass_completions, 2);
    assert_eq!(postmortem_outcome.skill_entries_written.len(), 2);

    let first_skill_file_path =
        skills_directory_path.join("document-the-public-api-of-the-forge-core-crate.md");
    let second_skill_file_path =
        skills_directory_path.join("write-unit-tests-for-the-tokenizer-module.md");

    assert!(first_skill_file_path.exists());
    assert!(second_skill_file_path.exists());

    let first_skill_content = std::fs::read_to_string(&first_skill_file_path).unwrap();
    let second_skill_content = std::fs::read_to_string(&second_skill_file_path).unwrap();

    assert!(first_skill_content.contains("night-mode-first-pass:true"));
    assert!(second_skill_content.contains("night-mode-first-pass:true"));

    let mut deferred_task_reasons = std::collections::HashMap::new();
    let deferred_task_id = task_ids[2];
    deferred_task_reasons.insert(
        deferred_task_id,
        "scope too broad for a single night run".to_owned(),
    );

    let digest_learnings = DigestLearnings {
        skills_updated: vec![
            SkillUpdate {
                name: "document-the-public-api-of-the-forge-core-crate".to_owned(),
                action: SkillAction::Added,
            },
            SkillUpdate {
                name: "write-unit-tests-for-the-tokenizer-module".to_owned(),
                action: SkillAction::Added,
            },
        ],
        mistake_records_added: 0,
    };

    let digest_context = DigestContext {
        metrics: yantra_night::DigestMetrics::default(),
        learnings: digest_learnings,
        deferred_task_reasons,
        halted_security_summaries: Vec::new(),
    };

    let dawn_digest_report = generate_digest(
        &night_session,
        &night_report,
        &digest_context,
        &yantra_directory_path,
    )
    .unwrap();

    assert!(dawn_digest_report.output_path.exists());

    let dawn_report_content = dawn_digest_report.content;
    assert!(dawn_report_content.contains("Completed (2)"));
    assert!(dawn_report_content.contains("Deferred (1)"));
    assert!(dawn_report_content.contains("Halted (0)"));
    assert!(dawn_report_content.contains("forge-core"));
    assert!(dawn_report_content.contains("tokenizer"));
    assert!(dawn_report_content.contains("scope too broad"));
    assert!(dawn_report_content.contains("What I Learned"));
    assert!(dawn_report_content.contains("document-the-public-api-of-the-forge-core-crate"));
    assert!(dawn_report_content.contains("write-unit-tests-for-the-tokenizer-module"));
}
