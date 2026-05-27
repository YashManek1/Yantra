//! Integration tests for `forge-verifier`.
//!
//! Tests the full three-gate verification pipeline using in-process fixtures.
//! Gate 2 and Gate 3 subprocess tests are marked to gracefully skip when the
//! required tools (cargo, etc.) are not on PATH.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use chrono::Utc;
use yantra_core::{Strictness, TaskClass, TaskId};
use yantra_stvp::SourceTruth;
use yantra_verifier::{
    ast_symbol_cross_check, compare_test_outcomes, parse_cargo_json_output, parse_test_outcome,
    Diff, DifferentialReport, TestOutcomeLabel, VerificationContext,
    VerificationPipeline, VerificationResult, VerificationStage, ViolationSeverity,
};

fn make_source_truth_with_answers(answers: &[(&str, &str)]) -> SourceTruth {
    SourceTruth {
        task_id: TaskId::new(),
        task_class: TaskClass::NewFeature,
        strictness: Strictness::Strict,
        description: "test task for verifier integration".to_owned(),
        created_at: Utc::now(),
        answers: answers
            .iter()
            .map(|&(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>(),
        augmented_question_ids: Vec::new(),
    }
}

fn make_diff_touching(files: &[&str]) -> Diff {
    let diff_text = files
        .iter()
        .flat_map(|path| {
            [
                format!("--- a/{path}"),
                format!("+++ b/{path}"),
                "@@ -1,1 +1,2 @@".to_owned(),
                "+// integration test line".to_owned(),
            ]
        })
        .collect::<Vec<_>>()
        .join("\n");

    Diff {
        text: diff_text,
        task_class: TaskClass::NewFeature,
    }
}

fn nonexistent_ctx() -> VerificationContext {
    // Use the temp dir (always valid on Windows) but without a Cargo.toml,
    // so gate 2/3 subprocess tools fail gracefully rather than panicking.
    VerificationContext {
        project_root: std::env::temp_dir(),
        is_sacred_diff: false,
        crg_db_path: None,
    }
}

// ---------------------------------------------------------------------------
// Pipeline pass/fail at gate 1 — no subprocess required
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pipeline_passes_clean_diff_at_gate1() {
    let source_truth = make_source_truth_with_answers(&[]);
    // Use a YAML file so gate 2 detects no language and spawns no tools.
    let diff = make_diff_touching(&["configs/settings.yaml"]);
    let result = VerificationPipeline::verify(&diff, &source_truth, &nonexistent_ctx(), 0)
        .await
        .unwrap();
    assert!(
        !matches!(
            result,
            VerificationResult::Reject {
                stage: VerificationStage::TruthDrift,
                ..
            }
        ),
        "clean diff must not be rejected at TruthDrift"
    );
}

#[tokio::test]
async fn pipeline_fails_at_gate1_when_diff_is_out_of_scope() {
    let source_truth = make_source_truth_with_answers(&[("out_of_scope", "src/payments.rs")]);
    let diff = make_diff_touching(&["src/payments.rs"]);
    let result = VerificationPipeline::verify(&diff, &source_truth, &nonexistent_ctx(), 0)
        .await
        .unwrap();
    assert!(
        matches!(
            result,
            VerificationResult::Reject {
                stage: VerificationStage::TruthDrift,
                ..
            }
        ),
        "out-of-scope diff must be rejected at TruthDrift"
    );
}

#[tokio::test]
async fn pipeline_escalates_to_human_review_at_max_retries() {
    let source_truth = make_source_truth_with_answers(&[]);
    let diff = make_diff_touching(&["src/lib.rs"]);
    let result = VerificationPipeline::verify(&diff, &source_truth, &nonexistent_ctx(), 3)
        .await
        .unwrap();
    assert!(
        matches!(result, VerificationResult::NeedHumanReview { .. }),
        "must escalate at retry_count == MAX_RETRIES"
    );
}

// ---------------------------------------------------------------------------
// Cargo JSON parser unit test
// ---------------------------------------------------------------------------

#[test]
fn cargo_json_parser_extracts_violations_correctly() {
    let json_line = concat!(
        r#"{"reason":"compiler-message","message":{"level":"error","rendered":"error[E0308]: mismatched types\n  --> src/lib.rs:5:10","#,
        r#""code":{"code":"E0308","explanation":null},"spans":[{"file_name":"src/lib.rs","line_start":5,"column_start":10,"is_primary":true}]}}"#
    );
    let violations = parse_cargo_json_output(json_line);
    assert_eq!(violations.len(), 1);
    let first_violation = &violations[0];
    assert_eq!(first_violation.severity, ViolationSeverity::Error);
    assert_eq!(first_violation.file_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(first_violation.line, Some(5));
    assert_eq!(first_violation.column, Some(10));
}

// ---------------------------------------------------------------------------
// Cargo test output parser
// ---------------------------------------------------------------------------

#[test]
fn cargo_test_output_parser_extracts_failures() {
    let test_output = concat!(
        "running 2 tests\n",
        "test auth::login_works ... FAILED\n",
        "test auth::logout_ok ... ok\n",
        "\n",
        "failures:\n",
        "  thread 'auth::login_works' panicked at src/auth.rs:42:5\n",
        "  assertion failed: result.is_ok()\n",
        "\n",
        "test result: FAILED. 1 passed; 1 failed;"
    );
    let test_outcome = parse_test_outcome(test_output);
    assert!(!test_outcome.all_passed);
    assert_eq!(test_outcome.failing_tests.len(), 1);
    assert_eq!(test_outcome.failing_tests[0].test_name, "auth::login_works");
    assert_eq!(
        test_outcome.failing_tests[0].file_path.as_deref(),
        Some("src/auth.rs")
    );
    assert_eq!(test_outcome.failing_tests[0].line_number, Some(42));
}

// ---------------------------------------------------------------------------
// Hallucination detector
// ---------------------------------------------------------------------------

#[test]
fn hallucination_detector_flags_unknown_symbols() {
    let known_symbols: HashSet<String> = HashSet::new();
    let diff_text = "+++ b/src/lib.rs\n@@ -0,0 +1,1 @@\n+    totally_fabricated_api_call(arg);\n";
    let hallucinated_symbols = ast_symbol_cross_check(diff_text, &known_symbols);
    assert!(
        hallucinated_symbols
            .iter()
            .any(|symbol| symbol.name == "totally_fabricated_api_call"),
        "unknown function must be flagged as hallucination"
    );
}

#[test]
fn hallucination_detector_passes_known_symbols() {
    let mut known_symbols: HashSet<String> = HashSet::new();
    known_symbols.insert("real_function".to_owned());

    let diff_text = "+++ b/src/lib.rs\n@@ -0,0 +1,1 @@\n+    real_function(x);\n";
    let hallucinated_symbols = ast_symbol_cross_check(diff_text, &known_symbols);
    assert!(
        !hallucinated_symbols
            .iter()
            .any(|symbol| symbol.name == "real_function"),
        "known symbol must not be flagged"
    );
}

// ---------------------------------------------------------------------------
// Adversarial: prompt-injected backdoor attempt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adversarial_diff_with_hallucinated_backdoor_is_checked() {
    // Gate 1 test: a diff that bypasses scope constraints must still pass gate 1
    // (no out-of-scope / dep constraints set), so the pipeline advances to later gates.
    // Uses a YAML file to avoid triggering gate 2 subprocess tools.
    let source_truth = make_source_truth_with_answers(&[]);
    let diff = Diff {
        text: concat!(
            "+++ b/configs/exploit.yaml\n",
            "@@ -0,0 +1,3 @@\n",
            "+# gate1 test\n",
            "+payload: exfiltrate_credentials_to_remote_server\n",
            "+enabled: true\n"
        )
        .to_owned(),
        task_class: TaskClass::NewFeature,
    };

    let result = VerificationPipeline::verify(&diff, &source_truth, &nonexistent_ctx(), 0)
        .await
        .unwrap();

    assert!(
        !matches!(
            result,
            VerificationResult::Reject {
                stage: VerificationStage::TruthDrift,
                ..
            }
        ),
        "gate 1 must not false-positive on a diff with no scope/constraint violations"
    );
}

// ---------------------------------------------------------------------------
// Differential testing: compare_test_outcomes
// ---------------------------------------------------------------------------

#[test]
fn differential_equivalent_suites_pass() {
    let suite = "test foo ... ok\n\ntest result: ok. 1 passed; 0 failed;";
    let report = compare_test_outcomes(suite, suite);
    assert!(matches!(report, DifferentialReport::Equivalent));
}

#[test]
fn differential_new_failure_is_rejected() {
    let before_output = "test foo ... ok\n\ntest result: ok. 1 passed; 0 failed;";
    let after_output = "test foo ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed;";
    let report = compare_test_outcomes(before_output, after_output);
    assert!(
        matches!(
            report,
            DifferentialReport::Reject {
                before_outcome: TestOutcomeLabel::Passed,
                after_outcome: TestOutcomeLabel::Failed,
                ..
            }
        ),
        "new test failure in after/ must produce Reject"
    );
}
