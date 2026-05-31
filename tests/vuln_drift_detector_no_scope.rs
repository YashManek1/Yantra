//! # Drift Detector Scope Tests
//!
//! Verifies that positive scope allowlists are enforced, that omitting scope
//! options produces warnings, and that warnings do not block the verification pipeline.
//!
//! ## Input
//! - Source truths with various scope definitions
//! - Diffs touching different files
//!
//! ## Output
//! - Validation and verification result outcomes
//!
//! ## Related
//! - `forge-stvp::drift` — drift detector
//! - `forge-verifier::gate1_truth_drift` — drift gate

use chrono::Utc;
use std::collections::BTreeMap;
use yantra_core::{Strictness, TaskClass, TaskId};
use yantra_stvp::drift::{DriftKind, TruthDriftDetector};
use yantra_stvp::SourceTruth;
use yantra_verifier::{
    Diff, ValidationResult, VerificationContext, VerificationPipeline, VerificationResult,
};

fn make_source_truth(answers: &[(&str, &str)]) -> SourceTruth {
    SourceTruth {
        task_id: TaskId::new(),
        task_class: TaskClass::NewFeature,
        strictness: Strictness::Strict,
        description: "test task".to_owned(),
        created_at: Utc::now(),
        answers: answers
            .iter()
            .map(|&(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>(),
        augmented_question_ids: Vec::new(),
    }
}

fn make_diff(touched_files: &[&str]) -> Diff {
    let diff_text = touched_files
        .iter()
        .flat_map(|path| {
            [
                format!("--- a/{path}"),
                format!("+++ b/{path}"),
                "@@ -1,1 +1,2 @@".to_owned(),
                "+// new comment".to_owned(),
            ]
        })
        .collect::<Vec<_>>()
        .join("\n");

    Diff {
        text: diff_text,
        task_class: TaskClass::NewFeature,
    }
}

#[test]
fn test_no_scope_defined_triggers_warning_violation() {
    // If neither out_of_scope nor in_scope_paths is set
    let source_truth = make_source_truth(&[]);
    let diff = make_diff(&["src/lib.rs"]);

    let violations = TruthDriftDetector::check(&source_truth, &diff.text);
    assert!(!violations.is_empty());
    assert_eq!(violations[0].severity, "warning");
    assert_eq!(violations[0].kind, DriftKind::ConstraintDrift);
}

#[test]
fn test_positive_allowlist_matches_correctly() {
    // Touching a file that is in the in_scope_paths allowlist
    let source_truth = make_source_truth(&[("in_scope_paths", "src/auth/**")]);
    let diff = make_diff(&["src/auth/token.rs"]);

    let violations = TruthDriftDetector::check(&source_truth, &diff.text);
    assert!(
        violations.is_empty(),
        "Valid allowlisted path must not trigger drift"
    );
}

#[test]
fn test_positive_allowlist_blocks_non_matching() {
    // Touching a file that is not in the in_scope_paths allowlist
    let source_truth = make_source_truth(&[("in_scope_paths", "src/auth/**")]);
    let diff = make_diff(&["src/payments.rs"]);

    let violations = TruthDriftDetector::check(&source_truth, &diff.text);
    assert!(!violations.is_empty());
    assert_eq!(violations[0].severity, "error");
    assert_eq!(violations[0].kind, DriftKind::ScopeDrift);
    assert!(violations[0]
        .message
        .contains("outside the positive allowlist"));
}

#[tokio::test]
async fn test_pipeline_warning_does_not_block_verification() {
    // Verify that a warning-severity violation (e.g. no scope defined)
    // is passed by VerificationPipeline without rejecting.
    let source_truth = make_source_truth(&[]);
    let diff = make_diff(&["src/lib.rs"]);
    let context = VerificationContext {
        project_root: std::env::temp_dir(),
        is_sacred_diff: false,
        crg_db_path: None,
        lsp_bridge: None,
    };

    let result = VerificationPipeline::verify(&diff, &source_truth, &context, 0)
        .await
        .unwrap();

    // In our simplified test setup, the static analysis/tests may not execute,
    // but the Gate 1 Truth Drift check will NOT block.
    // Let's assert it didn't reject at TruthDrift stage.
    if let VerificationResult::Reject { stage, .. } = result {
        assert_ne!(
            stage,
            yantra_verifier::VerificationStage::TruthDrift,
            "TruthDrift should not reject on warnings"
        );
    }
}
