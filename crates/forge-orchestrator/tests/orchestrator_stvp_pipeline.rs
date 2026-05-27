//! # forge-orchestrator integration tests: STVP pipeline
//!
//! End-to-end tests for the orchestrator STVP gate using the public API only.
//! These tests cover token issuance through scheduling — no LLM required.
//!
//! ## Input
//! - `SourceTruth` constructed in-process
//! - `SigningKey` generated in a temp directory
//!
//! ## Output
//! - Pass/fail assertions on `Orchestrator::schedule_task` and the testability
//!   validator
//!
//! ## Related
//! - `forge-orchestrator::Orchestrator` — the system under test
//! - `forge-stvp::run_all` — validator suite invoked before scheduling

use std::collections::BTreeMap;

use chrono::Utc;
use yantra_core::{AgentKind, ProjectRoot, Strictness, TaskClass, TaskId, TaskNode, TaskStatus};
use yantra_orchestrator::{Orchestrator, SchedulingError};
use yantra_stvp::{issue_token, run_all, ProjectContext, SigningKey, SourceTruth};

fn temp_yantra_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("yantra-orch-it-{label}-{}", TaskId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_truth(task_id: TaskId, success_criterion: &str) -> SourceTruth {
    let mut answers = BTreeMap::new();
    answers.insert("success_criterion".to_owned(), success_criterion.to_owned());

    SourceTruth {
        task_id,
        task_class: TaskClass::NewFeature,
        strictness: Strictness::Strict,
        description: "add JWT refresh token rotation".to_owned(),
        created_at: Utc::now(),
        answers,
        augmented_question_ids: Vec::new(),
    }
}

fn task_node_for(truth: &SourceTruth, token: yantra_core::TruthToken) -> TaskNode {
    TaskNode {
        id: truth.task_id,
        description: truth.description.clone(),
        status: TaskStatus::Pending,
        class: truth.task_class,
        dependencies: Vec::new(),
        assigned_agent: Some(AgentKind::Coder),
        truth_token: Some(token),
        parent_decision_id: None,
    }
}

/// A well-formed truth token allows scheduling.
#[test]
fn full_pipeline_stvp_to_orchestrator_succeeds() {
    let yantra_dir = temp_yantra_dir("success");
    let signing_key = SigningKey::load_or_generate(&yantra_dir).expect("key generated");

    let task_id = TaskId::new();
    let truth = make_truth(task_id, "returns HTTP 200 when the JWT token is valid");

    let project_root = ProjectRoot::new(&yantra_dir).expect("project root");
    let validation_context = ProjectContext {
        project_root: project_root.clone(),
    };
    let validation_report = run_all(&truth, &validation_context);
    assert!(
        validation_report.pass,
        "testable criterion must pass validation: {:?}",
        validation_report.violations
    );

    let truth_token = issue_token(&truth, &signing_key).expect("token issued");
    let task = task_node_for(&truth, truth_token);

    let orchestrator = Orchestrator::new(&yantra_dir);
    orchestrator
        .schedule_task(task)
        .expect("task with valid token must be scheduled");

    let queued = orchestrator.queued_tasks();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, task_id);
}

/// A vague success criterion triggers a testability violation before scheduling.
#[test]
fn vague_criterion_fails_testability_validator() {
    let yantra_dir = temp_yantra_dir("vague");
    let project_root = ProjectRoot::new(&yantra_dir).expect("project root");
    let validation_context = ProjectContext { project_root };

    let task_id = TaskId::new();
    let truth = make_truth(task_id, "the code should be cleaner and better");

    let validation_report = run_all(&truth, &validation_context);

    assert!(
        !validation_report.pass,
        "vague criterion must fail validation"
    );

    let testability_violations: Vec<_> = validation_report
        .violations
        .iter()
        .filter(|violation| violation.validator == "Testability")
        .collect();

    assert!(
        !testability_violations.is_empty(),
        "expected at least one Testability violation"
    );
}

/// A task submitted without a truth token is rejected with MissingTruthToken.
#[test]
fn task_without_token_is_rejected_at_orchestrator() {
    let yantra_dir = temp_yantra_dir("notoken");

    let task_id = TaskId::new();
    let task = TaskNode {
        id: task_id,
        description: "add JWT rotation".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: Vec::new(),
        assigned_agent: Some(AgentKind::Coder),
        truth_token: None,
        parent_decision_id: None,
    };

    let orchestrator = Orchestrator::new(&yantra_dir);
    let scheduling_error = orchestrator.schedule_task(task).unwrap_err();

    assert!(
        matches!(scheduling_error, SchedulingError::MissingTruthToken { .. }),
        "expected MissingTruthToken, got: {scheduling_error:?}"
    );
}

/// A token signed with a different key is rejected with InvalidTruthToken.
#[test]
fn token_signed_by_wrong_key_is_rejected() {
    let signing_dir = temp_yantra_dir("wrongkey-sign");
    let verifying_dir = temp_yantra_dir("wrongkey-verify");

    let signing_key = SigningKey::load_or_generate(&signing_dir).expect("signing key");
    SigningKey::load_or_generate(&verifying_dir).expect("verifying key");

    let task_id = TaskId::new();
    let truth = make_truth(task_id, "returns HTTP 200 when the JWT token is valid");
    let truth_token = issue_token(&truth, &signing_key).expect("token issued");
    let task = task_node_for(&truth, truth_token);

    let orchestrator = Orchestrator::new(&verifying_dir);
    let scheduling_error = orchestrator.schedule_task(task).unwrap_err();

    assert!(
        matches!(scheduling_error, SchedulingError::InvalidTruthToken { .. }),
        "expected InvalidTruthToken, got: {scheduling_error:?}"
    );
}
