//! # forge-stvp: Validation Framework
//!
//! Defines the `Validator` trait, `ValidationViolation`, `ValidationReport`,
//! and `ProjectContext`. The `run_all` function selects and runs the correct
//! set of validators based on the `Strictness` mode recorded in the
//! `SourceTruth`.
//!
//! ## Input
//! - `SourceTruth` — the artifact to be validated
//! - `ProjectContext` — access to the filesystem for reality checks
//!
//! ## Output
//! - `ValidationReport` — pass/fail flag plus all violations found
//!
//! ## Related
//! - `forge-stvp::validators` — concrete validator implementations
//! - `forge-stvp::source_truth` — provides the `SourceTruth` type
//! - `forge-stvp::interrogator` — calls `run_all` after questionnaire

use yantra_core::{ProjectRoot, Strictness};

use crate::source_truth::SourceTruth;
use crate::validators::codebase_reality::CodebaseRealityValidator;
use crate::validators::consistency::InternalConsistencyValidator;
use crate::validators::testability::TestabilityValidator;

/// Severity level assigned to a `ValidationViolation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationSeverity {
    /// The violation must be resolved before the task can proceed.
    Error,
    /// The violation is informational; the task may proceed.
    Warning,
}

/// A single issue found by a validator.
#[derive(Debug, Clone)]
pub struct ValidationViolation {
    /// Short name of the validator that produced this violation.
    pub validator: &'static str,
    /// Human-readable description of the problem.
    pub message: String,
    /// Whether the violation is blocking.
    pub severity: ViolationSeverity,
}

/// Aggregated result of running all applicable validators against a
/// `SourceTruth`.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// `true` when no error-severity violations were found.
    pub pass: bool,
    /// All violations collected from every validator that ran.
    pub violations: Vec<ValidationViolation>,
    /// Strictness mode that was applied.
    pub mode_applied: Strictness,
}

/// Project-level context injected into validators that need filesystem access.
pub struct ProjectContext {
    /// Root of the project being validated.
    pub project_root: ProjectRoot,
}

/// A stateless validator that inspects a `SourceTruth` and returns any
/// violations it finds.
pub trait Validator: Send + Sync {
    /// Inspects `truth` using `context` and returns every violation found.
    ///
    /// Validators never return `Err` — all failures are represented as
    /// violations with an appropriate `ViolationSeverity`.
    fn validate(&self, truth: &SourceTruth, context: &ProjectContext) -> Vec<ValidationViolation>;
}

/// Selects and runs the validators appropriate for `truth.strictness`.
///
/// - `Trust` → no validators run; report always passes
/// - `Light` → only `CodebaseRealityValidator` (V2)
/// - `Strict` → all three validators
pub fn run_all(truth: &SourceTruth, context: &ProjectContext) -> ValidationReport {
    match truth.strictness {
        Strictness::Trust => ValidationReport {
            pass: true,
            violations: Vec::new(),
            mode_applied: Strictness::Trust,
        },
        Strictness::Light => {
            let violations = CodebaseRealityValidator.validate(truth, context);
            let has_errors = violations
                .iter()
                .any(|violation| violation.severity == ViolationSeverity::Error);
            ValidationReport {
                pass: !has_errors,
                violations,
                mode_applied: Strictness::Light,
            }
        }
        Strictness::Strict => {
            let mut violations = Vec::new();
            violations.extend(InternalConsistencyValidator.validate(truth, context));
            violations.extend(CodebaseRealityValidator.validate(truth, context));
            violations.extend(TestabilityValidator.validate(truth, context));
            let has_errors = violations
                .iter()
                .any(|violation| violation.severity == ViolationSeverity::Error);
            ValidationReport {
                pass: !has_errors,
                violations,
                mode_applied: Strictness::Strict,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};

    use super::*;
    use crate::source_truth::SourceTruth;

    fn make_truth(strictness: Strictness, answers: &[(&str, &str)]) -> SourceTruth {
        SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::NewFeature,
            strictness,
            description: "implement rate limiting".to_owned(),
            created_at: Utc::now(),
            answers: answers
                .iter()
                .map(|&(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn temp_context() -> ProjectContext {
        let temp_path = std::env::temp_dir().join(format!("yantra-val-test-{}", TaskId::new()));
        std::fs::create_dir_all(&temp_path).unwrap();
        ProjectContext {
            project_root: ProjectRoot::new(temp_path).unwrap(),
        }
    }

    #[test]
    fn trust_mode_always_passes_with_no_violations() {
        let source_truth = make_truth(Strictness::Trust, &[]);
        let project_context = temp_context();
        let report = run_all(&source_truth, &project_context);
        assert!(report.pass);
        assert!(report.violations.is_empty());
        assert_eq!(report.mode_applied, Strictness::Trust);
    }

    #[test]
    fn light_mode_runs_only_reality_validator() {
        let source_truth = make_truth(Strictness::Light, &[("success_criterion", "better")]);
        let project_context = temp_context();
        let report = run_all(&source_truth, &project_context);
        assert_eq!(report.mode_applied, Strictness::Light);
        for violation in &report.violations {
            assert_eq!(
                violation.validator, "CodebaseReality",
                "Light mode must only run CodebaseReality, got: {}",
                violation.validator
            );
        }
    }

    #[test]
    fn strict_mode_flags_testability_violation() {
        let source_truth = make_truth(
            Strictness::Strict,
            &[("success_criterion", "the code is cleaner and better")],
        );
        let project_context = temp_context();
        let report = run_all(&source_truth, &project_context);
        assert_eq!(report.mode_applied, Strictness::Strict);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.validator == "Testability"),
            "Strict mode should run Testability validator"
        );
    }
}
