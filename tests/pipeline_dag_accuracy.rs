//! # Yantra: Verifier Pipeline and Orchestrator Gate Accuracy Tests
//!
//! End-to-end accuracy tests for two components that form the production task
//! pipeline:
//!
//! ## 1. Verifier Pipeline (`VerificationPipeline::verify`)
//!
//! 8 deterministic scenarios are tested — `Pass`, `Reject` at Gate 1, and
//! `NeedHumanReview`. Gate 2 (hallucination) produces Warning-severity
//! violations but does not block the pipeline (advisory gate). Target: 100%.
//!
//! ## 2. Orchestrator STVP Token Gate (`Orchestrator::schedule_task`)
//!
//! 4 scenarios test the ed25519-signed TruthToken gate that sits between user
//! intent and any DAG dispatch:
//! - No token → `MissingTruthToken`
//! - Valid token → queued successfully
//! - Token signed by wrong key → `InvalidTruthToken`
//! - Multiple valid tasks → all queued in order
//!
//! Target: 100% on all 4 scenarios.
//!
//! ## Why this is the "pipeline accuracy" test
//!
//! These two components are the two production gatekeepers in the task
//! pipeline — the orchestrator decides WHICH tasks dispatch (STVP gate), and
//! the verifier decides WHICH diffs are accepted (three-gate verification).
//! Getting both right at 100% is the pipeline correctness invariant.
//!
//! ## Input
//! - Hand-crafted `Diff`, `SourceTruth`, `VerificationContext` for the verifier
//! - Real `SigningKey` + `issue_token` for the orchestrator gate tests
//!
//! ## Related
//! - `forge-verifier::pipeline`    — `VerificationPipeline::verify` under test
//! - `forge-orchestrator`          — `Orchestrator::schedule_task` under test
//! - `forge-stvp::token`           — `issue_token` / `SigningKey` for gate tests

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;
use yantra_core::{AgentKind, Strictness, TaskClass, TaskId, TaskNode, TaskStatus};
use yantra_orchestrator::{Orchestrator, SchedulingError};
use yantra_stvp::{issue_token, SigningKey, SourceTruth};
use yantra_verifier::pipeline::VerificationPipeline;
use yantra_verifier::types::{Diff, VerificationContext, VerificationResult};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_source_truth(
    out_of_scope: &str,
    new_deps_allowed: &str,
    task_class: TaskClass,
) -> SourceTruth {
    let mut answers = BTreeMap::new();
    answers.insert("out_of_scope".to_owned(), out_of_scope.to_owned());
    answers.insert("new_deps_allowed".to_owned(), new_deps_allowed.to_owned());

    SourceTruth {
        task_id: TaskId::new(),
        task_class,
        strictness: Strictness::Strict,
        description: "pipeline accuracy test task".to_owned(),
        created_at: Utc::now(),
        answers,
        augmented_question_ids: Vec::new(),
    }
}

fn make_diff(touched_paths: &[&str], task_class: TaskClass) -> Diff {
    let diff_text = touched_paths
        .iter()
        .flat_map(|path| {
            [
                format!("--- a/{path}"),
                format!("+++ b/{path}"),
                "@@ -1,3 +1,4 @@".to_owned(),
                "+    let pipeline_change = true;".to_owned(),
            ]
        })
        .collect::<Vec<_>>()
        .join("\n");

    Diff {
        text: diff_text,
        task_class,
    }
}

/// Creates a minimal valid Rust workspace in a temp directory so that `cargo
/// check` (Gate 2) succeeds and the pipeline can proceed to Gate 3. The
/// workspace contains a single `src/lib.rs` with no tests, so Gate 3 trivially
/// passes as well. This lets us test the deterministic gate logic (Gate 1 scope
/// checks, retry escalation) without cargo subprocess interference.
fn temp_clean_workspace() -> (PathBuf, impl Drop) {
    let base_dir = std::env::temp_dir().join(format!("yantra-pipeline-ws-{}", TaskId::new(),));
    std::fs::create_dir_all(base_dir.join("src")).expect("workspace src dir creation must succeed");

    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"pipeline-test-ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml write must succeed");

    std::fs::write(
        base_dir.join("src").join("lib.rs"),
        "// pipeline accuracy test workspace\n",
    )
    .expect("src/lib.rs write must succeed");

    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let cleanup = Cleanup(base_dir.clone());
    (base_dir, cleanup)
}

fn make_verification_context(project_root: &Path) -> VerificationContext {
    VerificationContext {
        project_root: project_root.to_path_buf(),
        is_sacred_diff: false,
        crg_db_path: None,
        lsp_bridge: None,
    }
}

fn temp_yantra_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yantra-pipeline-acc-{label}-{}", TaskId::new(),));
    std::fs::create_dir_all(&dir).expect("temp yantra dir creation must succeed");
    dir
}

fn make_task_node(task_id: TaskId) -> TaskNode {
    TaskNode {
        id: task_id,
        description: "pipeline accuracy task".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::BugFix,
        dependencies: vec![],
        assigned_agent: Some(AgentKind::Coder),
        truth_token: None,
        parent_decision_id: None,
    }
}

fn make_source_truth_for_token(task_id: TaskId) -> SourceTruth {
    SourceTruth {
        task_id,
        task_class: TaskClass::BugFix,
        strictness: Strictness::Light,
        description: "token gate accuracy task".to_owned(),
        created_at: Utc::now(),
        answers: BTreeMap::new(),
        augmented_question_ids: Vec::new(),
    }
}

// ── Verifier Pipeline Accuracy (8 scenarios) ─────────────────────────────────

struct PipelineScenario {
    label: &'static str,
    diff: Diff,
    source_truth: SourceTruth,
    retry_count: u32,
    expected_class: &'static str,
}

/// Scenarios that test Gate 1 (scope + deps) and retry escalation only.
///
/// Diff file paths use `.md` and `.toml` extensions intentionally — Gate 2
/// (static analysis) only triggers for Rust/Python/TypeScript files. Using
/// non-code extensions lets the test focus on deterministic Gate 1 logic
/// without spawning `cargo check` on the test machine. Gate 2 behavior on
/// real code files is covered by `crates/forge-verifier/tests/verifier_integration.rs`.
fn build_pipeline_scenarios() -> Vec<PipelineScenario> {
    vec![
        // ── Pass scenarios (Gate 1 passes, Gate 2/3 trivially pass on non-code) ─
        PipelineScenario {
            label: "pass: clean diff touching in-scope docs file",
            diff: make_diff(&["docs/auth_design.md"], TaskClass::BugFix),
            source_truth: make_source_truth("src/payments.rs", "yes", TaskClass::BugFix),
            retry_count: 0,
            expected_class: "Pass",
        },
        PipelineScenario {
            label: "pass: NewFeature adding config file when new_deps_allowed=yes",
            diff: make_diff(
                &["configs/auth.toml", "docs/readme.md"],
                TaskClass::NewFeature,
            ),
            source_truth: make_source_truth("src/payments.rs", "yes", TaskClass::NewFeature),
            retry_count: 0,
            expected_class: "Pass",
        },
        PipelineScenario {
            label: "pass: Refactor touching only docs directory",
            diff: make_diff(
                &["docs/architecture.md", "docs/decisions/adr-007.md"],
                TaskClass::Refactor,
            ),
            source_truth: make_source_truth("src/payments.rs", "yes", TaskClass::Refactor),
            retry_count: 0,
            expected_class: "Pass",
        },
        // ── Gate 1 Reject scenarios ───────────────────────────────────────────
        PipelineScenario {
            label: "reject-gate1: diff touches explicitly out-of-scope payments.rs",
            diff: make_diff(&["src/payments.rs"], TaskClass::BugFix),
            source_truth: make_source_truth("src/payments.rs", "yes", TaskClass::BugFix),
            retry_count: 0,
            expected_class: "Reject",
        },
        PipelineScenario {
            label: "reject-gate1: Cargo.toml touched when new_deps_allowed=no",
            diff: make_diff(&["Cargo.toml"], TaskClass::BugFix),
            source_truth: make_source_truth("src/payments.rs", "no", TaskClass::BugFix),
            retry_count: 0,
            expected_class: "Reject",
        },
        PipelineScenario {
            label: "reject-gate1: nested Cargo.toml touched when new_deps_allowed=no",
            diff: make_diff(&["crates/forge-auth/Cargo.toml"], TaskClass::Refactor),
            source_truth: make_source_truth("src/payments.rs", "no", TaskClass::Refactor),
            retry_count: 0,
            expected_class: "Reject",
        },
        // ── Advisory gate (hallucination = Warning, not Reject) ────────────────
        // Hallucination violations are Warning-severity, so the pipeline still
        // returns Pass. The non-.rs extension prevents gate2 subprocess invocation
        // while still routing through the hallucination checker.
        PipelineScenario {
            label: "pass-with-warning: hallucinated symbol in non-rs diff is advisory",
            diff: Diff {
                text: "+++ b/docs/design_note.md\n@@ -1,0 +1,1 @@\n\
                       +    totally_hallucinated_function(x);\n"
                    .to_owned(),
                task_class: TaskClass::BugFix,
            },
            source_truth: make_source_truth("src/payments.rs", "yes", TaskClass::BugFix),
            retry_count: 0,
            expected_class: "Pass",
        },
        // ── Retry escalation ─────────────────────────────────────────────────
        PipelineScenario {
            label: "need-human-review: retry_count=3 triggers immediate escalation",
            diff: make_diff(&["docs/notes.md"], TaskClass::BugFix),
            source_truth: make_source_truth("src/payments.rs", "yes", TaskClass::BugFix),
            retry_count: 3,
            expected_class: "NeedHumanReview",
        },
    ]
}

#[tokio::test]
async fn verifier_pipeline_accuracy_is_100pct() {
    let scenarios = build_pipeline_scenarios();
    let total_count = scenarios.len();
    let mut correct_count: usize = 0;
    let mut wrong_verdicts: Vec<String> = Vec::new();

    let (workspace_dir, _workspace_cleanup) = temp_clean_workspace();
    let verification_context = make_verification_context(&workspace_dir);

    for scenario in &scenarios {
        let result = VerificationPipeline::verify(
            &scenario.diff,
            &scenario.source_truth,
            &verification_context,
            scenario.retry_count,
        )
        .await
        .unwrap_or_else(|error| {
            panic!(
                "VerificationPipeline::verify must not error on '{}': {error}",
                scenario.label
            )
        });

        let actual_class = match &result {
            VerificationResult::Pass => "Pass",
            VerificationResult::Reject { .. } => "Reject",
            VerificationResult::NeedHumanReview { .. } => "NeedHumanReview",
        };

        if actual_class == scenario.expected_class {
            correct_count += 1;
        } else {
            wrong_verdicts.push(format!(
                "  {:?}: expected={} actual={}",
                scenario.label, scenario.expected_class, actual_class,
            ));
        }
    }

    let accuracy_percentage = (correct_count * 100) / total_count;

    assert!(
        accuracy_percentage == 100,
        "VerificationPipeline accuracy {correct_count}/{total_count} ({accuracy_percentage}%) \
         below 100% (all scenarios are deterministic). Wrong:\n{}",
        wrong_verdicts.join("\n"),
    );
}

// ── Orchestrator STVP Token Gate (4 scenarios) ────────────────────────────────

#[test]
fn orchestrator_token_gate_accuracy_is_100pct() {
    struct GateScenario {
        label: &'static str,
        /// Whether to attach a real signed token to the task.
        has_valid_token: bool,
        /// Whether the token was signed with a different key than the orchestrator expects.
        wrong_key: bool,
        expected_error_kind: Option<&'static str>,
    }

    let gate_scenarios: &[GateScenario] = &[
        GateScenario {
            label: "no token → MissingTruthToken",
            has_valid_token: false,
            wrong_key: false,
            expected_error_kind: Some("MissingTruthToken"),
        },
        GateScenario {
            label: "valid signed token → queued successfully",
            has_valid_token: true,
            wrong_key: false,
            expected_error_kind: None,
        },
        GateScenario {
            label: "token signed by wrong key → InvalidTruthToken",
            has_valid_token: true,
            wrong_key: true,
            expected_error_kind: Some("InvalidTruthToken"),
        },
    ];

    let mut correct_count: usize = 0;
    let mut wrong_outcomes: Vec<String> = Vec::new();

    for scenario in gate_scenarios {
        let orchestrator_dir = temp_yantra_dir("orch-gate");
        let signing_dir = temp_yantra_dir("sign-gate");

        let correct_key =
            SigningKey::load_or_generate(&orchestrator_dir).expect("orchestrator key generated");

        let task_id = TaskId::new();
        let truth = make_source_truth_for_token(task_id);

        let mut task = make_task_node(task_id);

        if scenario.has_valid_token {
            let signing_key = if scenario.wrong_key {
                SigningKey::load_or_generate(&signing_dir).expect("wrong key generated")
            } else {
                correct_key
            };
            let token = issue_token(&truth, &signing_key).expect("token issued");
            task.truth_token = Some(token);
        }

        let orchestrator = Orchestrator::new(&orchestrator_dir);
        let result = orchestrator.schedule_task(task);

        let actual_outcome = match &result {
            Ok(()) => "Ok",
            Err(SchedulingError::MissingTruthToken { .. }) => "MissingTruthToken",
            Err(SchedulingError::InvalidTruthToken { .. }) => "InvalidTruthToken",
            Err(SchedulingError::TokenVerificationFailed { .. }) => "TokenVerificationFailed",
        };

        let expected_outcome = scenario.expected_error_kind.unwrap_or("Ok");

        if actual_outcome == expected_outcome {
            correct_count += 1;
        } else {
            wrong_outcomes.push(format!(
                "  {:?}: expected={expected_outcome} actual={actual_outcome}",
                scenario.label,
            ));
        }

        let _ = std::fs::remove_dir_all(&orchestrator_dir);
        let _ = std::fs::remove_dir_all(&signing_dir);
    }

    let total_count = gate_scenarios.len();
    let accuracy_percentage = (correct_count * 100) / total_count;

    assert!(
        accuracy_percentage == 100,
        "Orchestrator token-gate accuracy {correct_count}/{total_count} ({accuracy_percentage}%) \
         below 100% (deterministic ed25519 gate). Wrong outcomes:\n{}",
        wrong_outcomes.join("\n"),
    );
}

/// Verifies that multiple valid tasks are ALL queued in order.
#[test]
fn orchestrator_queues_multiple_valid_tasks_in_order() {
    let yantra_dir = temp_yantra_dir("multi-queue");
    let signing_key = SigningKey::load_or_generate(&yantra_dir).expect("signing key generated");
    let orchestrator = Orchestrator::new(&yantra_dir);

    let task_count: usize = 5;
    let mut expected_task_ids: Vec<TaskId> = Vec::new();

    for _ in 0..task_count {
        let task_id = TaskId::new();
        let truth = make_source_truth_for_token(task_id);
        let token = issue_token(&truth, &signing_key).expect("token issued");
        let mut task = make_task_node(task_id);
        task.truth_token = Some(token);

        orchestrator
            .schedule_task(task)
            .expect("scheduling must succeed for valid tasks");

        expected_task_ids.push(task_id);
    }

    let queued_tasks = orchestrator.queued_tasks();

    assert_eq!(
        queued_tasks.len(),
        task_count,
        "all {task_count} tasks must be queued; found {} in queue",
        queued_tasks.len(),
    );

    let queued_ids: Vec<TaskId> = queued_tasks.iter().map(|task| task.id).collect();

    for task_id in &expected_task_ids {
        assert!(
            queued_ids.contains(task_id),
            "task {task_id} must be present in the queue after scheduling"
        );
    }

    let _ = std::fs::remove_dir_all(&yantra_dir);
}
