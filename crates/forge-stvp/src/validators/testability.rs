//! # forge-stvp: Testability Validator
//!
//! Checks that the `success_criterion` answer is specific enough to be
//! converted into an automated test. Vague success criteria ("the code is
//! better", "it should be cleaner") block the agent from writing a meaningful
//! Boolean Exit Gate check.
//!
//! ## Input
//! - `SourceTruth` — answer key checked: `success_criterion`
//! - `ProjectContext` — unused by this validator
//!
//! ## Output
//! - Zero or more `ValidationViolation` values
//!
//! ## Related
//! - `forge-stvp::validation` — Validator trait and ValidationReport
//! - `forge-stvp::validators::consistency` — complementary answer-level check

use std::sync::LazyLock;

use regex::Regex;

use crate::source_truth::SourceTruth;
use crate::validation::{ProjectContext, ValidationViolation, Validator, ViolationSeverity};

static PATTERN_VAGUE_TERM: LazyLock<Regex> = LazyLock::new(|| {
    // SAFETY: This regex pattern is validated at compile time. If it fails, the
    // binary is fundamentally broken and cannot run correctly.
    Regex::new(
        r"(?i)\b(cleaner|better|more efficient|nicer|prettier|improved|good|great|faster|simpler|easier)\b",
    )
    .expect("vague-term pattern is valid")
});

static PATTERN_TESTABLE_VERB: LazyLock<Regex> = LazyLock::new(|| {
    // SAFETY: This regex pattern is validated at compile time. If it fails, the
    // binary is fundamentally broken and cannot run correctly.
    Regex::new(
        r"(?i)\b(returns?|equals?|fails?|succeeds?|contains?|raises?|throws?|completes? in|passes?|matches?|outputs?|emits?|produces?|asserts?|verif(y|ies)|rejects?|accepts?)\b",
    )
    .expect("testable-verb pattern is valid")
});

/// Validates that the success criterion is specific and machine-testable.
pub struct TestabilityValidator;

impl Validator for TestabilityValidator {
    fn validate(&self, truth: &SourceTruth, _context: &ProjectContext) -> Vec<ValidationViolation> {
        let mut violations = Vec::new();

        let success_criterion = match truth.answers.get("success_criterion") {
            Some(answer) if !answer.trim().is_empty() => answer.trim(),
            _ => {
                violations.push(ValidationViolation {
                    validator: "Testability",
                    message: "success_criterion is missing or empty".to_owned(),
                    severity: ViolationSeverity::Error,
                });
                return violations;
            }
        };

        if PATTERN_VAGUE_TERM.is_match(success_criterion)
            && !PATTERN_TESTABLE_VERB.is_match(success_criterion)
        {
            violations.push(ValidationViolation {
                validator: "Testability",
                message: format!(
                    "success_criterion '{success_criterion}' is too vague to be automatically tested; \
                     use observable, measurable language (e.g. 'returns', 'equals', 'fails with')"
                ),
                severity: ViolationSeverity::Error,
            });
        }

        violations
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};

    use super::*;
    use crate::validation::ProjectContext;

    fn make_truth(success_criterion: &str) -> SourceTruth {
        let mut answers = BTreeMap::new();
        if !success_criterion.is_empty() {
            answers.insert("success_criterion".to_owned(), success_criterion.to_owned());
        }
        SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::NewFeature,
            strictness: Strictness::Strict,
            description: "test task".to_owned(),
            created_at: Utc::now(),
            answers,
            augmented_question_ids: Vec::new(),
        }
    }

    fn temp_context() -> ProjectContext {
        let temp_path =
            std::env::temp_dir().join(format!("yantra-testability-test-{}", TaskId::new()));
        std::fs::create_dir_all(&temp_path).unwrap();
        ProjectContext {
            project_root: ProjectRoot::new(temp_path).unwrap(),
        }
    }

    #[test]
    fn vague_criterion_without_testable_verb_produces_error() {
        let source_truth = make_truth("the code is cleaner and better");
        let context = temp_context();
        let violations = TestabilityValidator.validate(&source_truth, &context);
        assert!(
            violations
                .iter()
                .any(|violation| violation.validator == "Testability"
                    && violation.severity == ViolationSeverity::Error),
            "expected Error for vague success criterion"
        );
    }

    #[test]
    fn specific_criterion_with_testable_verb_passes() {
        let source_truth = make_truth("the function returns 200 OK when called with a valid token");
        let context = temp_context();
        let violations = TestabilityValidator.validate(&source_truth, &context);
        assert!(
            !violations
                .iter()
                .any(|violation| violation.severity == ViolationSeverity::Error),
            "specific criterion should not produce an error"
        );
    }

    #[test]
    fn missing_success_criterion_produces_error() {
        let source_truth = make_truth("");
        let context = temp_context();
        let violations = TestabilityValidator.validate(&source_truth, &context);
        assert!(
            violations
                .iter()
                .any(|violation| violation.message.contains("missing")),
            "missing criterion should produce an error"
        );
    }

    #[test]
    fn vague_term_with_testable_verb_passes() {
        let source_truth =
            make_truth("the better path returns HTTP 201 and contains the new task ID");
        let context = temp_context();
        let violations = TestabilityValidator.validate(&source_truth, &context);
        // Has vague term "better" but also has testable verbs "returns", "contains"
        assert!(
            !violations
                .iter()
                .any(|violation| violation.severity == ViolationSeverity::Error),
            "criterion with testable verbs should pass even if it contains a vague term"
        );
    }
}
