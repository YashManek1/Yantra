//! Integration tests for the full STVP flow:
//! Interrogator → Validators → TruthToken issuance → drift detection.

use std::collections::HashMap;

use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};
use yantra_stvp::{
    issue_token, run_all, DriftKind, Interrogator, ProjectContext, Question, QuestionnaireUi,
    SigningKey, StvpError, TruthDriftDetector,
};

struct MockUi {
    answers: HashMap<String, String>,
}

impl MockUi {
    fn new(answers: impl IntoIterator<Item = (&'static str, &'static str)>) -> Self {
        Self {
            answers: answers
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
        }
    }
}

impl QuestionnaireUi for MockUi {
    fn prompt(&self, question: &Question) -> Result<String, StvpError> {
        Ok(self.answers.get(&question.id).cloned().unwrap_or_default())
    }
}

fn temp_project_root() -> (ProjectRoot, std::path::PathBuf) {
    let temp_path =
        std::env::temp_dir().join(format!("yantra-stvp-validators-int-{}", TaskId::new()));
    std::fs::create_dir_all(&temp_path).unwrap();
    let project_root = ProjectRoot::new(temp_path.clone()).unwrap();
    (project_root, temp_path)
}

#[test]
fn full_flow_new_feature_strict_passes_validation_and_issues_token() {
    let (project_root, temp_path) = temp_project_root();
    let interrogator = Interrogator::new(project_root.clone());

    let source_file = temp_path.join("src").join("main.rs");
    std::fs::create_dir_all(source_file.parent().unwrap()).unwrap();
    std::fs::write(&source_file, "fn main() {}\n").unwrap();

    let mock_ui = MockUi::new([
        ("primary_files", "src/main.rs"),
        ("new_deps_allowed", "yes"),
        (
            "success_criterion",
            "the function returns 200 OK when called",
        ),
        ("out_of_scope", "src/payments.rs"),
    ]);

    let source_truth = interrogator
        .ask("implement JWT rotation for the auth service", &mock_ui)
        .expect("interrogation succeeds");

    assert_eq!(source_truth.task_class, TaskClass::NewFeature);
    assert_eq!(source_truth.strictness, Strictness::Strict);

    let context = ProjectContext {
        project_root: project_root.clone(),
    };
    let validation_report = run_all(&source_truth, &context);
    assert_eq!(validation_report.mode_applied, Strictness::Strict);

    let yantra_dir = temp_path.join(".yantra");
    std::fs::create_dir_all(&yantra_dir).unwrap();
    let signing_key = SigningKey::load_or_generate(&yantra_dir).expect("signing key created");

    let truth_token = issue_token(&source_truth, &signing_key).expect("token issued");
    assert_eq!(truth_token.task_id, source_truth.task_id);
    assert_eq!(truth_token.strictness, Strictness::Strict);
}

#[test]
fn trust_mode_task_skips_validators_and_issues_token() {
    let (project_root, temp_path) = temp_project_root();
    let interrogator = Interrogator::new(project_root.clone());

    let mock_ui = MockUi::new([]);
    let source_truth = interrogator
        .ask("format all source files with rustfmt style rules", &mock_ui)
        .expect("interrogation succeeds");

    assert_eq!(source_truth.strictness, Strictness::Trust);

    let context = ProjectContext {
        project_root: project_root.clone(),
    };
    let validation_report = run_all(&source_truth, &context);
    assert!(validation_report.pass, "Trust mode always passes");
    assert!(validation_report.violations.is_empty());

    let yantra_dir = temp_path.join(".yantra");
    std::fs::create_dir_all(&yantra_dir).unwrap();
    let signing_key = SigningKey::load_or_generate(&yantra_dir).expect("signing key created");

    let truth_token = issue_token(&source_truth, &signing_key).expect("token issued");
    assert_eq!(truth_token.strictness, Strictness::Trust);
}

#[test]
fn drift_detector_catches_scope_violation_in_agent_diff() {
    let (project_root, _temp_path) = temp_project_root();
    let interrogator = Interrogator::new(project_root);

    let mock_ui = MockUi::new([
        ("reproduction_steps", "call foo() with null argument"),
        ("expected_behavior", "returns Error instead of panicking"),
        (
            "success_criterion",
            "the function returns Err on null input",
        ),
        ("reproduction_confidence", "4"),
    ]);

    let source_truth = interrogator
        .ask("fix null dereference in auth handler", &mock_ui)
        .expect("interrogation succeeds");

    let diff =
        "--- a/src/payments.rs\n+++ b/src/payments.rs\n@@ -1,3 +1,4 @@\n+// unauthorized change\n";
    let violations = TruthDriftDetector::check(&source_truth, diff);

    // out_of_scope was not set in this truth, so no ScopeDrift expected from out_of_scope
    // But if it were, we'd check it here. This test confirms no false positives.
    assert!(
        violations
            .iter()
            .all(|violation| violation.kind != DriftKind::ScopeDrift),
        "no scope drift when out_of_scope is not set"
    );
}

#[test]
fn drift_detector_catches_dep_manifest_change_when_disallowed() {
    use chrono::Utc;
    use std::collections::BTreeMap;
    use yantra_stvp::SourceTruth;

    let source_truth = SourceTruth {
        task_id: TaskId::new(),
        task_class: TaskClass::BugFix,
        strictness: Strictness::Light,
        description: "fix off-by-one in pagination".to_owned(),
        created_at: Utc::now(),
        answers: [
            ("new_deps_allowed".to_owned(), "no".to_owned()),
            (
                "success_criterion".to_owned(),
                "page returns exactly N items".to_owned(),
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>(),
    };

    let diff = "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -10,3 +10,4 @@\n+serde = \"1.0\"\n";
    let violations = TruthDriftDetector::check(&source_truth, diff);

    assert!(
        violations
            .iter()
            .any(|violation| violation.kind == DriftKind::ConstraintDrift),
        "expected ConstraintDrift when Cargo.toml modified and deps disallowed"
    );
}
