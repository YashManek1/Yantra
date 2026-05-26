//! Integration tests for `forge-stvp`.
//!
//! Exercises the full interrogation flow from raw description string through
//! questionnaire, `SourceTruth` assembly, disk storage, and hash-verified
//! reload — using a `MockQuestionnaireUi` that feeds deterministic canned answers.

use std::collections::HashMap;

use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};
use yantra_stvp::{
    Interrogator, Question, QuestionnaireUi, SourceTruth, StvpError, TaskClassifier,
};

/// Deterministic UI that returns pre-loaded answers keyed by question ID.
///
/// Questions with no entry in `answers` return an empty string.
struct MockQuestionnaireUi {
    answers: HashMap<String, String>,
}

impl MockQuestionnaireUi {
    fn new(pairs: &[(&str, &str)]) -> Self {
        Self {
            answers: pairs
                .iter()
                .map(|(question_id, answer)| ((*question_id).to_string(), (*answer).to_string()))
                .collect(),
        }
    }
}

impl QuestionnaireUi for MockQuestionnaireUi {
    fn prompt(&self, question: &Question) -> Result<String, StvpError> {
        Ok(self.answers.get(&question.id).cloned().unwrap_or_default())
    }
}

fn temp_project_root(label: &str) -> ProjectRoot {
    let temp_path = std::env::temp_dir().join(format!("yantra-stvp-it-{label}-{}", TaskId::new()));
    std::fs::create_dir_all(&temp_path).unwrap();
    ProjectRoot::new(temp_path).unwrap()
}

#[test]
fn new_feature_flow_produces_correct_source_truth() {
    let project_root = temp_project_root("new-feature");
    let interrogator = Interrogator::new(project_root.clone());

    let mock_ui = MockQuestionnaireUi::new(&[
        ("primary_files", "src/auth/jwt.rs\nsrc/auth/rotation.rs"),
        ("pattern_files", "src/auth/token.rs"),
        ("new_deps_allowed", "no"),
        (
            "success_criterion",
            "calling rotate_jwt() returns a new token and invalidates the old one",
        ),
        ("out_of_scope", "UI changes, metrics dashboards"),
        ("sacred_files", "src/auth/jwt.rs"),
        ("deploy_deadline", ""),
    ]);

    let source_truth = interrogator
        .ask("add JWT rotation to auth service", &mock_ui)
        .expect("interrogation succeeds for a well-answered new-feature task");

    assert_eq!(source_truth.task_class, TaskClass::NewFeature);
    assert_eq!(source_truth.strictness, Strictness::Strict);
    assert_eq!(source_truth.description, "add JWT rotation to auth service");
    assert_eq!(
        source_truth
            .answers
            .get("primary_files")
            .map(String::as_str),
        Some("src/auth/jwt.rs\nsrc/auth/rotation.rs")
    );
    assert_eq!(
        source_truth
            .answers
            .get("new_deps_allowed")
            .map(String::as_str),
        Some("no")
    );
    assert!(source_truth
        .answers
        .get("success_criterion")
        .unwrap()
        .contains("rotate_jwt"));
}

#[test]
fn new_feature_truth_round_trips_through_disk() {
    let project_root = temp_project_root("disk-roundtrip");
    let interrogator = Interrogator::new(project_root.clone());

    let mock_ui = MockQuestionnaireUi::new(&[
        ("primary_files", "src/feature.rs"),
        ("new_deps_allowed", "yes"),
        ("success_criterion", "feature_test passes"),
        ("out_of_scope", "nothing"),
    ]);

    let stored_truth = interrogator
        .ask("implement the new feature", &mock_ui)
        .expect("interrogation succeeds");

    let loaded_truth =
        SourceTruth::load(&project_root, stored_truth.task_id).expect("load succeeds after store");

    assert_eq!(stored_truth, loaded_truth);
}

#[test]
fn bug_fix_flow_produces_light_strictness() {
    let project_root = temp_project_root("bug-fix");
    let interrogator = Interrogator::new(project_root);

    let mock_ui = MockQuestionnaireUi::new(&[
        (
            "reproduction_steps",
            "1. Open settings\n2. Click Save\n3. App crashes",
        ),
        (
            "expected_behavior",
            "Settings save without crashing the app",
        ),
        ("success_criterion", "test_settings_save passes"),
        ("reproduction_confidence", "4"),
    ]);

    let source_truth = interrogator
        .ask("fix crash when opening settings", &mock_ui)
        .expect("interrogation succeeds for a well-answered bug-fix task");

    assert_eq!(source_truth.task_class, TaskClass::BugFix);
    assert_eq!(source_truth.strictness, Strictness::Light);
    assert!(source_truth
        .answers
        .get("reproduction_steps")
        .unwrap()
        .contains("crashes"));
    assert_eq!(
        source_truth
            .answers
            .get("reproduction_confidence")
            .map(String::as_str),
        Some("4")
    );
}

#[test]
fn empty_description_is_rejected() {
    let project_root = temp_project_root("empty-desc");
    let interrogator = Interrogator::new(project_root);
    let mock_ui = MockQuestionnaireUi::new(&[]);
    assert!(matches!(
        interrogator.ask("", &mock_ui),
        Err(StvpError::EmptyDescription)
    ));
}

#[test]
fn missing_required_answer_is_rejected() {
    let project_root = temp_project_root("missing-answer");
    let interrogator = Interrogator::new(project_root);
    let mock_ui = MockQuestionnaireUi::new(&[("primary_files", "")]);
    let result = interrogator.ask("add a new endpoint", &mock_ui);
    assert!(
        matches!(result, Err(StvpError::MissingRequiredAnswer { ref question_id }) if question_id == "primary_files"),
        "expected MissingRequiredAnswer for primary_files, got {result:?}"
    );
}

#[test]
fn classifier_correctly_identifies_all_fixture_classes() {
    let class_fixtures: &[(&str, TaskClass)] = &[
        ("add JWT rotation", TaskClass::NewFeature),
        ("fix the login crash", TaskClass::BugFix),
        ("refactor the router", TaskClass::Refactor),
        ("migrate from diesel to sqlx", TaskClass::Migration),
        ("integrate Stripe", TaskClass::Integration),
        ("explore tracing options", TaskClass::Exploration),
        ("add docstrings to auth module", TaskClass::Docstring),
        ("format the codebase", TaskClass::Style),
    ];

    for &(description, expected_class) in class_fixtures {
        assert_eq!(
            TaskClassifier::classify(description),
            expected_class,
            "wrong class for {description:?}"
        );
    }
}

#[test]
fn question_answer_kind_is_accessible() {
    use yantra_stvp::AnswerKind;

    let question = Question {
        id: "test_q".to_owned(),
        text: "Is this a test?".to_owned(),
        answer_kind: AnswerKind::YesNo,
        required: true,
    };

    assert_eq!(question.answer_kind, AnswerKind::YesNo);
    assert!(question.required);
}

#[test]
fn source_truth_yaml_is_stable_across_serializations() {
    let project_root = temp_project_root("yaml-stable");
    let interrogator = Interrogator::new(project_root);

    let mock_ui = MockQuestionnaireUi::new(&[
        ("primary_files", "src/lib.rs"),
        ("new_deps_allowed", "no"),
        ("success_criterion", "all tests pass"),
        ("out_of_scope", "nothing extra"),
    ]);

    let source_truth = interrogator
        .ask("implement rate limiting", &mock_ui)
        .expect("interrogation succeeds");

    let yaml_first = source_truth.to_yaml().expect("first serialization");
    let yaml_second = source_truth.to_yaml().expect("second serialization");
    assert_eq!(
        yaml_first, yaml_second,
        "YAML output must be deterministic across calls"
    );
}
