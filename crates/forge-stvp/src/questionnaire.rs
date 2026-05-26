//! # forge-stvp: Questionnaire Templates
//!
//! Per-class questionnaire definitions. Each `TaskClass` maps to a `Vec<Question>`
//! that the `Interrogator` presents to the user via a `QuestionnaireUi`. The two
//! fully implemented classes are `NewFeature` (7 questions) and `BugFix`
//! (7 questions). All other classes have a skeleton with one generic question.
//!
//! ## Input
//! - A `TaskClass` value from the classifier
//!
//! ## Output
//! - `Vec<Question>` with per-question metadata including `AnswerKind` and
//!   whether the question is required
//!
//! ## Related
//! - `forge-stvp::classifier` — produces the `TaskClass` consumed here
//! - `forge-stvp::interrogator` — drives the questionnaire via `QuestionnaireUi`
//! - `forge-stvp::source_truth` — records question IDs and answers

use yantra_core::TaskClass;

/// Kind of answer expected for a questionnaire question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerKind {
    /// Multi-line free-form text.
    FreeText,
    /// Exactly "yes" or "no".
    YesNo,
    /// A newline-separated list of file paths.
    FileList,
    /// An ISO-8601 date-time string, or empty if not applicable.
    DateTime,
    /// An integer in a stated range (the range is conveyed in the question text).
    Number,
    /// One of the given strings.
    OneOf(Vec<String>),
}

/// A single question presented to the user during STVP interrogation.
#[derive(Debug, Clone)]
pub struct Question {
    /// Stable identifier used as the key in `SourceTruth::answers`.
    pub id: String,
    /// Human-readable prompt shown to the user.
    pub text: String,
    /// Expected shape of the answer.
    pub answer_kind: AnswerKind,
    /// When `true`, a blank answer causes the interrogator to return an error.
    pub required: bool,
}

impl Question {
    fn new(
        id: impl Into<String>,
        text: impl Into<String>,
        answer_kind: AnswerKind,
        required: bool,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            answer_kind,
            required,
        }
    }
}

/// Returns the questionnaire for a given `TaskClass`.
///
/// `NewFeature` and `BugFix` are fully specified with 7 questions each.
/// Light-mode classes (`Refactor`, `Migration`, `Integration`, `Exploration`)
/// return a single generic success-criterion question. Trust-mode classes
/// (`Chore`, `Docstring`, `Style`) return an empty questionnaire.
pub fn questionnaire_for_class(task_class: TaskClass) -> Vec<Question> {
    match task_class {
        TaskClass::NewFeature => new_feature_questionnaire(),
        TaskClass::BugFix => bug_fix_questionnaire(),
        TaskClass::Refactor => skeleton_questionnaire("refactor"),
        TaskClass::Migration => skeleton_questionnaire("migration"),
        TaskClass::Integration => skeleton_questionnaire("integration"),
        TaskClass::Exploration => skeleton_questionnaire("exploration"),
        // Trust-mode classes: no questionnaire — the spec guarantees no validators run.
        TaskClass::Chore | TaskClass::Docstring | TaskClass::Style => vec![],
    }
}

fn new_feature_questionnaire() -> Vec<Question> {
    vec![
        Question::new(
            "primary_files",
            "What files should this new feature touch primarily? (one path per line)",
            AnswerKind::FileList,
            true,
        ),
        Question::new(
            "pattern_files",
            "Which existing files contain patterns to mirror for this feature? (one path per line, leave blank if none)",
            AnswerKind::FileList,
            false,
        ),
        Question::new(
            "new_deps_allowed",
            "Are new external dependencies allowed for this feature? (yes/no, default: no)",
            AnswerKind::YesNo,
            true,
        ),
        Question::new(
            "success_criterion",
            "What is the success criterion as a testable predicate? \
             (e.g. \"calling rotate_jwt() returns a new token and invalidates the old one\")",
            AnswerKind::FreeText,
            true,
        ),
        Question::new(
            "out_of_scope",
            "What is explicitly out of scope for this task?",
            AnswerKind::FreeText,
            true,
        ),
        Question::new(
            "sacred_files",
            "Are any sacred files (auth, payments, migrations, crypto) involved? \
             If so, list them. (leave blank if none)",
            AnswerKind::FileList,
            false,
        ),
        Question::new(
            "deploy_deadline",
            "What is the deploy deadline, if any? (ISO-8601 date-time, e.g. 2026-06-01T18:00:00Z, \
             or leave blank)",
            AnswerKind::DateTime,
            false,
        ),
    ]
}

fn bug_fix_questionnaire() -> Vec<Question> {
    vec![
        Question::new(
            "reproduction_steps",
            "How do I reproduce the bug? (provide exact steps)",
            AnswerKind::FreeText,
            true,
        ),
        Question::new(
            "expected_behavior",
            "What is the expected behavior once the bug is fixed?",
            AnswerKind::FreeText,
            true,
        ),
        Question::new(
            "suspected_files",
            "Which files do you suspect are involved? (one path per line, leave blank if unsure)",
            AnswerKind::FileList,
            false,
        ),
        Question::new(
            "regression_info",
            "Is this a regression? If so, approximately when or in which commit was it introduced? \
             (leave blank if not a regression)",
            AnswerKind::FreeText,
            false,
        ),
        Question::new(
            "success_criterion",
            "What is the success criterion? (e.g. \"test_login_reset passes\" or \
             \"no panic on empty input\")",
            AnswerKind::FreeText,
            true,
        ),
        Question::new(
            "sacred_files",
            "Are any sacred files (auth, payments, migrations, crypto) involved? \
             If so, list them. (leave blank if none)",
            AnswerKind::FileList,
            false,
        ),
        Question::new(
            "reproduction_confidence",
            "How confident are you in your reproduction steps? (1 = very uncertain, 5 = always reproduces)",
            AnswerKind::OneOf(vec![
                "1".to_owned(),
                "2".to_owned(),
                "3".to_owned(),
                "4".to_owned(),
                "5".to_owned(),
            ]),
            true,
        ),
    ]
}

fn skeleton_questionnaire(class_label: &str) -> Vec<Question> {
    vec![Question::new(
        "success_criterion",
        format!(
            "What is the success criterion for this {class_label} task? \
             (describe a testable, observable outcome)"
        ),
        AnswerKind::FreeText,
        true,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_feature_questionnaire_has_seven_questions() {
        let questionnaire = questionnaire_for_class(TaskClass::NewFeature);
        assert_eq!(questionnaire.len(), 7);
    }

    #[test]
    fn bug_fix_questionnaire_has_seven_questions() {
        let questionnaire = questionnaire_for_class(TaskClass::BugFix);
        assert_eq!(questionnaire.len(), 7);
    }

    #[test]
    fn new_feature_required_questions_are_correct() {
        let questionnaire = questionnaire_for_class(TaskClass::NewFeature);
        let required_ids: Vec<&str> = questionnaire
            .iter()
            .filter(|question| question.required)
            .map(|question| question.id.as_str())
            .collect();
        assert!(required_ids.contains(&"primary_files"));
        assert!(required_ids.contains(&"new_deps_allowed"));
        assert!(required_ids.contains(&"success_criterion"));
        assert!(required_ids.contains(&"out_of_scope"));
        assert!(!required_ids.contains(&"sacred_files"));
        assert!(!required_ids.contains(&"deploy_deadline"));
    }

    #[test]
    fn bug_fix_required_questions_are_correct() {
        let questionnaire = questionnaire_for_class(TaskClass::BugFix);
        let required_ids: Vec<&str> = questionnaire
            .iter()
            .filter(|question| question.required)
            .map(|question| question.id.as_str())
            .collect();
        assert!(required_ids.contains(&"reproduction_steps"));
        assert!(required_ids.contains(&"expected_behavior"));
        assert!(required_ids.contains(&"success_criterion"));
        assert!(required_ids.contains(&"reproduction_confidence"));
        assert!(!required_ids.contains(&"suspected_files"));
        assert!(!required_ids.contains(&"regression_info"));
    }

    #[test]
    fn light_mode_skeleton_classes_return_one_success_criterion_question() {
        for task_class in [
            TaskClass::Refactor,
            TaskClass::Migration,
            TaskClass::Integration,
            TaskClass::Exploration,
        ] {
            let questionnaire = questionnaire_for_class(task_class);
            assert_eq!(
                questionnaire.len(),
                1,
                "{task_class:?} should have exactly one skeleton question"
            );
            assert_eq!(questionnaire[0].id, "success_criterion");
        }
    }

    #[test]
    fn trust_mode_classes_return_empty_questionnaire() {
        for task_class in [TaskClass::Chore, TaskClass::Docstring, TaskClass::Style] {
            let questionnaire = questionnaire_for_class(task_class);
            assert!(
                questionnaire.is_empty(),
                "{task_class:?} is Trust-mode and must return no questions"
            );
        }
    }

    #[test]
    fn all_question_ids_are_non_empty() {
        for task_class in [TaskClass::NewFeature, TaskClass::BugFix] {
            let questionnaire = questionnaire_for_class(task_class);
            for question in &questionnaire {
                assert!(
                    !question.id.is_empty(),
                    "empty id in {task_class:?} questionnaire"
                );
                assert!(!question.text.is_empty(), "empty text for {}", question.id);
            }
        }
    }
}
