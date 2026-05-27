//! # forge-stvp: Internal Consistency Validator
//!
//! Checks that the questionnaire answers within a `SourceTruth` do not
//! contradict each other. Contradictions caught here indicate a mis-specified
//! task that would likely produce an incorrect agent execution plan.
//!
//! ## Input
//! - `SourceTruth` — the artifact to validate
//! - `ProjectContext` — unused by this validator (consistency is answer-only)
//!
//! ## Output
//! - Zero or more `ValidationViolation` values
//!
//! ## Related
//! - `forge-stvp::validation` — Validator trait and ValidationReport
//! - `forge-stvp::validators::codebase_reality` — filesystem-based validator

use std::sync::LazyLock;

use regex::Regex;

use crate::source_truth::SourceTruth;
use crate::validation::{ProjectContext, ValidationViolation, Validator, ViolationSeverity};

static PATTERN_ADD_DEPENDENCY: LazyLock<Regex> = LazyLock::new(|| {
    // SAFETY: This regex pattern is validated at compile time. If it fails, the
    // binary is fundamentally broken and cannot run correctly.
    Regex::new(
        r"(?i)\b(add(ing)?|instal(l(ing)?)?|import(ing)?|includ(e|ing)?|bring(ing)?|introduc(e|ing)?)\b.{0,60}\b(dep(endency|endencies)?|library|libraries|crate|package|mod(ule)?)\b",
    )
    .expect("dependency pattern is valid")
});

/// Validates that questionnaire answers are internally consistent.
pub struct InternalConsistencyValidator;

impl Validator for InternalConsistencyValidator {
    fn validate(&self, truth: &SourceTruth, _context: &ProjectContext) -> Vec<ValidationViolation> {
        let mut violations = Vec::new();

        check_dep_contradiction(truth, &mut violations);
        check_scope_file_overlap(truth, &mut violations);
        check_deadline_in_past(truth, &mut violations);

        violations
    }
}

fn check_dep_contradiction(truth: &SourceTruth, violations: &mut Vec<ValidationViolation>) {
    let deps_disallowed = truth
        .answers
        .get("new_deps_allowed")
        .is_some_and(|answer| answer.trim().eq_ignore_ascii_case("no"));

    if deps_disallowed && PATTERN_ADD_DEPENDENCY.is_match(&truth.description) {
        violations.push(ValidationViolation {
            validator: "InternalConsistency",
            message: "description mentions adding a dependency but new_deps_allowed is 'no'"
                .to_owned(),
            severity: ViolationSeverity::Error,
        });
    }
}

fn check_scope_file_overlap(truth: &SourceTruth, violations: &mut Vec<ValidationViolation>) {
    let out_of_scope = match truth.answers.get("out_of_scope") {
        Some(answer) if !answer.trim().is_empty() => answer.trim().to_owned(),
        _ => return,
    };

    let pattern_files = match truth.answers.get("pattern_files") {
        Some(answer) if !answer.trim().is_empty() => answer.trim().to_owned(),
        _ => return,
    };

    let out_of_scope_files: Vec<&str> = out_of_scope
        .split([',', '\n'])
        .map(str::trim)
        .filter(|file_path| !file_path.is_empty())
        .collect();

    let pattern_file_list: Vec<&str> = pattern_files
        .split([',', '\n'])
        .map(str::trim)
        .filter(|file_path| !file_path.is_empty())
        .collect();

    for out_of_scope_file in &out_of_scope_files {
        if pattern_file_list.contains(out_of_scope_file) {
            violations.push(ValidationViolation {
                validator: "InternalConsistency",
                message: format!(
                    "file '{out_of_scope_file}' appears in both pattern_files and out_of_scope"
                ),
                severity: ViolationSeverity::Error,
            });
        }
    }
}

fn check_deadline_in_past(truth: &SourceTruth, violations: &mut Vec<ValidationViolation>) {
    let deadline_text = match truth.answers.get("deploy_deadline") {
        Some(answer) if !answer.trim().is_empty() => answer.trim(),
        _ => return,
    };

    if let Ok(deadline) = chrono::DateTime::parse_from_rfc3339(deadline_text) {
        if deadline < chrono::Utc::now() {
            violations.push(ValidationViolation {
                validator: "InternalConsistency",
                message: format!("deploy_deadline '{deadline_text}' is in the past"),
                severity: ViolationSeverity::Warning,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};

    use super::*;
    use crate::validation::ProjectContext;

    fn make_truth(answers: &[(&str, &str)]) -> SourceTruth {
        SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::NewFeature,
            strictness: Strictness::Strict,
            description: "implement rate limiting".to_owned(),
            created_at: Utc::now(),
            answers: answers
                .iter()
                .map(|&(key, value)| (key.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>(),
            augmented_question_ids: Vec::new(),
        }
    }

    fn temp_context() -> ProjectContext {
        let temp_path =
            std::env::temp_dir().join(format!("yantra-consistency-test-{}", TaskId::new()));
        std::fs::create_dir_all(&temp_path).unwrap();
        ProjectContext {
            project_root: ProjectRoot::new(temp_path).unwrap(),
        }
    }

    #[test]
    fn dep_contradiction_flagged_when_deps_disallowed_but_description_mentions_adding() {
        let source_truth = SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::NewFeature,
            strictness: Strictness::Strict,
            description: "add the tokio crate as a dependency for async support".to_owned(),
            created_at: Utc::now(),
            answers: [("new_deps_allowed".to_owned(), "no".to_owned())]
                .into_iter()
                .collect(),
            augmented_question_ids: Vec::new(),
        };
        let context = temp_context();
        let violations = InternalConsistencyValidator.validate(&source_truth, &context);
        assert!(
            violations
                .iter()
                .any(|violation| violation.validator == "InternalConsistency"
                    && violation.severity == ViolationSeverity::Error),
            "expected Error violation for dep contradiction"
        );
    }

    #[test]
    fn no_violation_when_deps_disallowed_and_description_unrelated() {
        let source_truth = make_truth(&[("new_deps_allowed", "no")]);
        let context = temp_context();
        let violations = InternalConsistencyValidator.validate(&source_truth, &context);
        assert!(violations
            .iter()
            .all(|violation| violation.validator != "InternalConsistency"
                || violation.message.contains("dep")),);
        // Specifically: should NOT have dep contradiction error
        assert!(!violations
            .iter()
            .any(|violation| violation.message.contains("dependency")
                && violation.severity == ViolationSeverity::Error),);
    }

    #[test]
    fn scope_file_overlap_flagged_when_file_in_both_lists() {
        let source_truth = make_truth(&[
            ("pattern_files", "src/auth.rs, src/lib.rs"),
            ("out_of_scope", "src/auth.rs"),
        ]);
        let context = temp_context();
        let violations = InternalConsistencyValidator.validate(&source_truth, &context);
        assert!(
            violations
                .iter()
                .any(|violation| violation.message.contains("src/auth.rs")
                    && violation.severity == ViolationSeverity::Error),
            "expected Error for file appearing in both pattern_files and out_of_scope"
        );
    }

    #[test]
    fn past_deadline_produces_warning() {
        let source_truth = make_truth(&[("deploy_deadline", "2020-01-01T00:00:00Z")]);
        let context = temp_context();
        let violations = InternalConsistencyValidator.validate(&source_truth, &context);
        assert!(
            violations
                .iter()
                .any(|violation| violation.severity == ViolationSeverity::Warning
                    && violation.message.contains("past")),
            "expected Warning for past deadline"
        );
    }

    #[test]
    fn future_deadline_produces_no_violation() {
        let source_truth = make_truth(&[("deploy_deadline", "2099-12-31T23:59:59Z")]);
        let context = temp_context();
        let violations = InternalConsistencyValidator.validate(&source_truth, &context);
        assert!(!violations
            .iter()
            .any(|violation| violation.message.contains("past")),);
    }
}
