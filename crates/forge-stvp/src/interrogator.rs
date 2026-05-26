//! # forge-stvp: Interrogator
//!
//! Drives the STVP questionnaire flow end-to-end. The `Interrogator` classifies
//! the task description, selects a per-class questionnaire, presents each
//! question to the user through a `QuestionnaireUi`, assembles the answers into
//! a `SourceTruth`, and persists it to disk via `source_truth::SourceTruth::store`.
//!
//! ## Input
//! - A natural-language task description
//! - A `QuestionnaireUi` implementation (CLI terminal, mock, or future GUI)
//! - A `ProjectRoot` that determines where `.yantra/source_truth/` is written
//!
//! ## Output
//! - A signed-and-stored `SourceTruth` returned to the caller
//!
//! ## Related
//! - `forge-stvp::classifier` — classifies the description into a `TaskClass`
//! - `forge-stvp::questionnaire` — provides per-class `Vec<Question>` templates
//! - `forge-stvp::source_truth` — serializes and stores the final artifact
//! - `forge-core::truth` — `TruthToken` is issued by the caller after receiving
//!   the `SourceTruth` (token issuance is the orchestrator's responsibility)

use std::collections::BTreeMap;

use chrono::Utc;
use tracing::instrument;
use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};

use crate::classifier::TaskClassifier;
use crate::error::StvpError;
use crate::questionnaire::{questionnaire_for_class, Question};
use crate::source_truth::SourceTruth;

/// Determines the STVP strictness mode for a given task class.
///
/// - Strict: `NewFeature`, `Migration`, `Integration` — all three validators
/// - Light: `BugFix`, `Refactor`, `Exploration` — Validator 2 only
/// - Trust: `Chore`, `Docstring`, `Style` — no validators, no questionnaire
pub fn strictness_for_class(task_class: TaskClass) -> Strictness {
    match task_class {
        TaskClass::NewFeature | TaskClass::Migration | TaskClass::Integration => Strictness::Strict,
        TaskClass::BugFix | TaskClass::Refactor | TaskClass::Exploration => Strictness::Light,
        TaskClass::Chore | TaskClass::Docstring | TaskClass::Style => Strictness::Trust,
    }
}

/// Abstraction over the user-facing prompt layer.
///
/// Implement this trait to plug in a CLI terminal, a web UI, or a test mock.
/// The `prompt` method is called once per question; it must return the raw
/// answer string as typed by the user.
pub trait QuestionnaireUi: Send + Sync {
    /// Presents `question` to the user and returns the raw answer string.
    ///
    /// # Errors
    ///
    /// Returns `StvpError::QuestionnaireAborted` if the UI cannot present the
    /// question or the user signals they want to abort.
    fn prompt(&self, question: &Question) -> Result<String, StvpError>;
}

/// Runs the STVP interrogation flow for a single task description.
pub struct Interrogator {
    project_root: ProjectRoot,
}

impl Interrogator {
    /// Creates an `Interrogator` that writes source-truth artifacts under
    /// `<project_root>/.yantra/source_truth/`.
    pub fn new(project_root: ProjectRoot) -> Self {
        Self { project_root }
    }

    /// Classifies `description`, runs the appropriate questionnaire via `ui`,
    /// and stores the resulting `SourceTruth` to disk.
    ///
    /// # Errors
    ///
    /// - `StvpError::EmptyDescription` — description is blank
    /// - `StvpError::MissingRequiredAnswer` — a required question got no answer
    /// - `StvpError::QuestionnaireAborted` — the UI signalled an abort
    /// - Storage or serialization errors from `source_truth::SourceTruth::store`
    #[instrument(skip(self, ui), fields(description = %description))]
    pub fn ask(
        &self,
        description: &str,
        ui: &dyn QuestionnaireUi,
    ) -> Result<SourceTruth, StvpError> {
        let trimmed_description = description.trim();
        if trimmed_description.is_empty() {
            return Err(StvpError::EmptyDescription);
        }

        let task_class = TaskClassifier::classify(trimmed_description);
        let strictness = strictness_for_class(task_class);
        let questionnaire = questionnaire_for_class(task_class);

        tracing::debug!(
            task_class = ?task_class,
            strictness = ?strictness,
            question_count = questionnaire.len(),
            "questionnaire selected"
        );

        let collected_answers = Self::collect_answers(&questionnaire, ui)?;

        let source_truth = SourceTruth {
            task_id: TaskId::new(),
            task_class,
            strictness,
            description: trimmed_description.to_owned(),
            created_at: Utc::now(),
            answers: collected_answers,
        };

        SourceTruth::store(&self.project_root, &source_truth)?;

        tracing::info!(
            task_id = %source_truth.task_id,
            task_class = ?source_truth.task_class,
            "source truth stored"
        );

        Ok(source_truth)
    }

    fn collect_answers(
        questionnaire: &[Question],
        ui: &dyn QuestionnaireUi,
    ) -> Result<BTreeMap<String, String>, StvpError> {
        let mut collected_answers: BTreeMap<String, String> = BTreeMap::new();

        for question in questionnaire {
            let raw_answer = ui.prompt(question)?;
            let trimmed_answer = raw_answer.trim().to_owned();

            if question.required && trimmed_answer.is_empty() {
                return Err(StvpError::MissingRequiredAnswer {
                    question_id: question.id.clone(),
                });
            }

            collected_answers.insert(question.id.clone(), trimmed_answer);
        }

        Ok(collected_answers)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};

    use super::*;
    use crate::questionnaire::Question;

    struct MockUi {
        answers: HashMap<String, String>,
    }

    impl MockUi {
        fn new(answers: impl IntoIterator<Item = (&'static str, &'static str)>) -> Self {
            Self {
                answers: answers
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
            }
        }
    }

    impl QuestionnaireUi for MockUi {
        fn prompt(&self, question: &Question) -> Result<String, StvpError> {
            Ok(self.answers.get(&question.id).cloned().unwrap_or_default())
        }
    }

    fn temp_project_root() -> ProjectRoot {
        let temp_path =
            std::env::temp_dir().join(format!("yantra-interrogator-test-{}", TaskId::new()));
        std::fs::create_dir_all(&temp_path).unwrap();
        ProjectRoot::new(temp_path).unwrap()
    }

    #[test]
    fn ask_returns_error_for_empty_description() {
        let project_root = temp_project_root();
        let interrogator = Interrogator::new(project_root);
        let mock_ui = MockUi::new([]);
        assert!(matches!(
            interrogator.ask("   ", &mock_ui),
            Err(StvpError::EmptyDescription)
        ));
    }

    #[test]
    fn ask_returns_error_when_required_answer_is_blank() {
        let project_root = temp_project_root();
        let interrogator = Interrogator::new(project_root);
        let mock_ui = MockUi::new([("primary_files", "")]);
        let result = interrogator.ask("add JWT rotation to auth service", &mock_ui);
        assert!(
            matches!(result, Err(StvpError::MissingRequiredAnswer { ref question_id }) if question_id == "primary_files"),
            "expected MissingRequiredAnswer for primary_files, got: {result:?}"
        );
    }

    #[test]
    fn strictness_for_new_feature_is_strict() {
        assert_eq!(
            strictness_for_class(TaskClass::NewFeature),
            Strictness::Strict
        );
    }

    #[test]
    fn strictness_for_bug_fix_is_light() {
        assert_eq!(strictness_for_class(TaskClass::BugFix), Strictness::Light);
    }

    #[test]
    fn strictness_for_docstring_is_trust() {
        assert_eq!(
            strictness_for_class(TaskClass::Docstring),
            Strictness::Trust
        );
    }

    #[test]
    fn ask_classifies_description_correctly() {
        let project_root = temp_project_root();
        let interrogator = Interrogator::new(project_root);
        let mock_ui = MockUi::new([
            ("primary_files", "src/auth.rs"),
            ("new_deps_allowed", "no"),
            ("success_criterion", "token is rotated"),
            ("out_of_scope", "UI changes"),
        ]);
        let source_truth = interrogator
            .ask("implement JWT rotation", &mock_ui)
            .expect("interrogation succeeds");
        assert_eq!(source_truth.task_class, TaskClass::NewFeature);
        assert_eq!(source_truth.strictness, Strictness::Strict);
    }
}
