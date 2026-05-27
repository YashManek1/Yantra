//! # forge-stvp: Questionnaire Templates
//!
//! Per-class questionnaire definitions. Each `TaskClass` maps to a `Vec<Question>`
//! that the `Interrogator` presents to the user. Supports a hybrid dynamic structure
//! with base questions (always shown) and trigger-based questions (keyword-matched).
//!
//! ## Input
//! - A `TaskClass` value from the classifier
//! - Task description string for keyword-based dynamic triggers
//!
//! ## Output
//! - `Vec<Question>` containing selected questions with per-question metadata
//!
//! ## Related
//! - `forge-stvp::classifier` — produces the `TaskClass` consumed here
//! - `forge-stvp::interrogator` — drives the questionnaire via `QuestionnaireUi`
//! - `forge-stvp::source_truth` — records question IDs and answers

use yantra_core::TaskClass;

const SACRED_TRIGGER_WORDS: &[&str] = &[
    "auth",
    "jwt",
    "token",
    "password",
    "payment",
    "stripe",
    "migration",
    "crypto",
    "secret",
    "key",
    "certificate",
];

const DEP_TRIGGER_WORDS: &[&str] = &[
    "library",
    "libraries",
    "crate",
    "crates",
    "dependency",
    "dependencies",
    "package",
    "packages",
    "extern",
    "import",
    "dep",
    "deps",
];

const DEADLINE_TRIGGER_WORDS: &[&str] = &[
    "deadline", "date", "release", "deploy", "by", "schedule", "due",
];

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
    /// Visually flags that the question was dynamically augmented via LLM.
    pub augmented: bool,
    /// Optional placeholder or suggested answer.
    pub suggested_answer: Option<String>,
    /// Optional help message shown below the question.
    pub help_text: Option<String>,
}

impl Question {
    /// Creates a base or trigger question.
    pub fn new(
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
            augmented: false,
            suggested_answer: None,
            help_text: None,
        }
    }

    /// Creates an LLM-augmented question.
    pub fn new_augmented(
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
            augmented: true,
            suggested_answer: None,
            help_text: None,
        }
    }

    /// Sets a suggested answer (placeholder) for this question.
    #[must_use]
    pub fn with_suggested_answer(mut self, suggested_answer: impl Into<String>) -> Self {
        self.suggested_answer = Some(suggested_answer.into());
        self
    }

    /// Sets additional help text for this question.
    #[must_use]
    pub fn with_help_text(mut self, help_text: impl Into<String>) -> Self {
        self.help_text = Some(help_text.into());
        self
    }
}

/// Returns the base questionnaire for a given `TaskClass` (always shown).
pub fn base_questions_for_class(task_class: TaskClass) -> Vec<Question> {
    match task_class {
        TaskClass::NewFeature => vec![
            Question::new(
                "primary_files",
                "What files should this new feature touch primarily? (one path per line)",
                AnswerKind::FileList,
                true,
            )
            .with_suggested_answer("src/main.rs")
            .with_help_text(
                "Relative paths to files that will be modified or created (one path per line)",
            ),
            Question::new(
                "success_criterion",
                "What is the success criterion as a testable predicate? \
                 (e.g. \"calling rotate_jwt() returns a new token and invalidates the old one\")",
                AnswerKind::FreeText,
                true,
            )
            .with_suggested_answer(
                "calling rotate_jwt() returns a new token and invalidates the old one",
            )
            .with_help_text("Describe a testable, observable predicate of success"),
            Question::new(
                "out_of_scope",
                "What is explicitly out of scope for this task?",
                AnswerKind::FreeText,
                true,
            )
            .with_suggested_answer("frontend UI changes")
            .with_help_text("Specify any features or changes that are explicitly out of scope"),
        ],
        TaskClass::BugFix => vec![
            Question::new(
                "reproduction_steps",
                "How do I reproduce the bug? (provide exact steps)",
                AnswerKind::FreeText,
                true,
            )
            .with_suggested_answer("cargo test --test failing_login_test")
            .with_help_text("Specify exact commands, conditions, or steps to reproduce the bug"),
            Question::new(
                "expected_behavior",
                "What is the expected behavior once the bug is fixed?",
                AnswerKind::FreeText,
                true,
            )
            .with_suggested_answer("login function returns Ok(user) instead of Err(InvalidToken)")
            .with_help_text("Describe what the code should do after the fix is implemented"),
            Question::new(
                "success_criterion",
                "What is the success criterion? (e.g. \"test_login_reset passes\" or \
                 \"no panic on empty input\")",
                AnswerKind::FreeText,
                true,
            )
            .with_suggested_answer("cargo test passes with 0 failures")
            .with_help_text("What is the observable success condition? (e.g. a passing test name)"),
        ],
        TaskClass::Refactor => skeleton_questionnaire("refactor"),
        TaskClass::Migration => skeleton_questionnaire("migration"),
        TaskClass::Integration => skeleton_questionnaire("integration"),
        TaskClass::Exploration => skeleton_questionnaire("exploration"),
        // Trust-mode classes: no questionnaire.
        TaskClass::Chore | TaskClass::Docstring | TaskClass::Style => vec![],
    }
}

/// Evaluates keyword matches on the task description to trigger additional questions.
pub fn trigger_questions_for_task(task_class: TaskClass, task_description: &str) -> Vec<Question> {
    let mut trigger_questions = Vec::new();
    let lower_description = task_description.to_lowercase();

    // 1. Universal Trigger: Sacred files check
    let has_sacred_keywords = SACRED_TRIGGER_WORDS
        .iter()
        .any(|word| lower_description.contains(word));
    if has_sacred_keywords {
        trigger_questions.push(Question::new(
            "sacred_files",
            "Are any sacred files (auth, payments, migrations, crypto) involved? \
             If so, list them. (leave blank if none)",
            AnswerKind::FileList,
            false,
        )
        .with_suggested_answer("src/main.rs")
        .with_help_text("Paths to sensitive security or migration files involved in the edit (leave blank if none)"));
    }

    // 2. Class-specific Triggers
    match task_class {
        TaskClass::NewFeature => {
            // External dependencies allowed?
            let has_dep_keywords = DEP_TRIGGER_WORDS
                .iter()
                .any(|word| lower_description.contains(word));
            if has_dep_keywords {
                trigger_questions.push(Question::new(
                    "new_deps_allowed",
                    "Are new external dependencies allowed for this feature? (yes/no, default: no)",
                    AnswerKind::YesNo,
                    true,
                )
                .with_suggested_answer("no")
                .with_help_text("Specify 'yes' or 'no' if any new external crate dependencies will be added"));
            }

            // Pattern mirroring files?
            if lower_description.contains("pattern")
                || lower_description.contains("mirror")
                || lower_description.contains("copy")
            {
                trigger_questions.push(Question::new(
                    "pattern_files",
                    "Which existing files contain patterns to mirror for this feature? (one path per line, leave blank if none)",
                    AnswerKind::FileList,
                    false,
                )
                .with_suggested_answer("src/auth.rs")
                .with_help_text("Relative paths to existing source files to mirror patterns from (leave blank if none)"));
            }

            // Deploy deadline?
            let has_deadline_keywords = DEADLINE_TRIGGER_WORDS
                .iter()
                .any(|word| lower_description.contains(word));
            if has_deadline_keywords {
                trigger_questions.push(Question::new(
                    "deploy_deadline",
                    "What is the deploy deadline, if any? (ISO-8601 date-time, e.g. 2026-06-01T18:00:00Z, or leave blank)",
                    AnswerKind::DateTime,
                    false,
                )
                .with_suggested_answer("2026-06-01T18:00:00Z")
                .with_help_text("ISO-8601 date-time string for deploy deadline (leave blank if none)"));
            }
        }
        TaskClass::BugFix => {
            // For bug fixes, trigger suspected files, regression, and confidence questions
            trigger_questions.push(Question::new(
                "suspected_files",
                "Which files do you suspect are involved? (one path per line, leave blank if unsure)",
                AnswerKind::FileList,
                false,
            )
            .with_suggested_answer("src/main.rs")
            .with_help_text("Paths to files you suspect are causing the bug (one path per line)"));

            trigger_questions.push(Question::new(
                "regression_info",
                "Is this a regression? If so, approximately when or in which commit was it introduced? \
                 (leave blank if not a regression)",
                AnswerKind::FreeText,
                false,
            )
            .with_suggested_answer("broken since commit f1a3b8e")
            .with_help_text("Details about when the regression was introduced (leave blank if not a regression)"));

            trigger_questions.push(Question::new(
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
            ));
        }
        _ => {}
    }

    trigger_questions
}

/// Backwards compatibility helper returning the legacy questionnaire with all potentials.
pub fn questionnaire_for_class(task_class: TaskClass) -> Vec<Question> {
    match task_class {
        TaskClass::NewFeature => {
            let mut questionnaire_list = base_questions_for_class(task_class);
            questionnaire_list.push(Question::new(
                "pattern_files",
                "Which existing files contain patterns to mirror for this feature? (one path per line, leave blank if none)",
                AnswerKind::FileList,
                false,
            ));
            questionnaire_list.push(Question::new(
                "new_deps_allowed",
                "Are new external dependencies allowed for this feature? (yes/no, default: no)",
                AnswerKind::YesNo,
                true,
            ));
            questionnaire_list.push(Question::new(
                "sacred_files",
                "Are any sacred files (auth, payments, migrations, crypto) involved? \
                 If so, list them. (leave blank if none)",
                AnswerKind::FileList,
                false,
            ));
            questionnaire_list.push(Question::new(
                "deploy_deadline",
                "What is the deploy deadline, if any? (ISO-8601 date-time, e.g. 2026-06-01T18:00:00Z, or leave blank)",
                AnswerKind::DateTime,
                false,
            ));

            // Re-order to match original test expectations exactly:
            // 0: primary_files, 1: pattern_files, 2: new_deps_allowed, 3: success_criterion, 4: out_of_scope, 5: sacred_files, 6: deploy_deadline
            vec![
                questionnaire_list[0].clone(), // primary_files
                questionnaire_list[3].clone(), // pattern_files
                questionnaire_list[4].clone(), // new_deps_allowed
                questionnaire_list[1].clone(), // success_criterion
                questionnaire_list[2].clone(), // out_of_scope
                questionnaire_list[5].clone(), // sacred_files
                questionnaire_list[6].clone(), // deploy_deadline
            ]
        }
        TaskClass::BugFix => {
            let mut questionnaire_list = base_questions_for_class(task_class);
            questionnaire_list.push(Question::new(
                "suspected_files",
                "Which files do you suspect are involved? (one path per line, leave blank if unsure)",
                AnswerKind::FileList,
                false,
            ));
            questionnaire_list.push(Question::new(
                "regression_info",
                "Is this a regression? If so, approximately when or in which commit was it introduced? \
                 (leave blank if not a regression)",
                AnswerKind::FreeText,
                false,
            ));
            questionnaire_list.push(Question::new(
                "sacred_files",
                "Are any sacred files (auth, payments, migrations, crypto) involved? \
                 If so, list them. (leave blank if none)",
                AnswerKind::FileList,
                false,
            ));
            questionnaire_list.push(Question::new(
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
            ));

            // Re-order to match original test expectations exactly:
            // 0: reproduction_steps, 1: expected_behavior, 2: suspected_files, 3: regression_info, 4: success_criterion, 5: sacred_files, 6: reproduction_confidence
            vec![
                questionnaire_list[0].clone(), // reproduction_steps
                questionnaire_list[1].clone(), // expected_behavior
                questionnaire_list[3].clone(), // suspected_files
                questionnaire_list[4].clone(), // regression_info
                questionnaire_list[2].clone(), // success_criterion
                questionnaire_list[5].clone(), // sacred_files
                questionnaire_list[6].clone(), // reproduction_confidence
            ]
        }
        TaskClass::Refactor => skeleton_questionnaire("refactor"),
        TaskClass::Migration => skeleton_questionnaire("migration"),
        TaskClass::Integration => skeleton_questionnaire("integration"),
        TaskClass::Exploration => skeleton_questionnaire("exploration"),
        TaskClass::Chore | TaskClass::Docstring | TaskClass::Style => vec![],
    }
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

    #[test]
    fn triggers_for_jwt_rotation_fire_sacred_and_dep() {
        let sacred_feature = trigger_questions_for_task(
            TaskClass::NewFeature,
            "add JWT rotation utilizing standard libraries",
        );
        let trigger_ids: Vec<&str> = sacred_feature.iter().map(|q| q.id.as_str()).collect();
        assert!(trigger_ids.contains(&"sacred_files"));
        assert!(trigger_ids.contains(&"new_deps_allowed"));
    }
}
