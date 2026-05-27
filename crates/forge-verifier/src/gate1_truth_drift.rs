//! # forge-verifier: Gate 1 — Truth Drift Detector
//!
//! Wraps `yantra_stvp::TruthDriftDetector` to check whether a proposed diff
//! stays within the constraints recorded in the task's `SourceTruth`. This
//! gate is synchronous and must complete in <50 ms.
//!
//! ## Input
//! - `SourceTruth` — the validated task specification from STVP
//! - `Diff` — the agent's proposed change
//!
//! ## Output
//! - `ValidationResult::Pass` — diff is within scope and constraints
//! - `ValidationResult::Fail(violations)` — scope or constraint drift found
//!
//! ## Related
//! - `yantra_stvp::drift` — provides `TruthDriftDetector` and `DriftViolation`
//! - `forge-verifier::pipeline` — calls this as the first gate

use yantra_stvp::drift::{DriftKind, TruthDriftDetector};
use yantra_stvp::SourceTruth;

use crate::error::VerifierResult;
use crate::types::{Diff, ValidationResult, Violation, ViolationSeverity};

/// Gate 1: wraps `TruthDriftDetector` and converts its output to `Violation`s.
pub struct TruthDriftGate;

impl TruthDriftGate {
    /// Checks `diff` against `truth` and returns a pass/fail verdict.
    pub fn check(truth: &SourceTruth, diff: &Diff) -> VerifierResult<ValidationResult> {
        let drift_violations = TruthDriftDetector::check(truth, &diff.text);

        if drift_violations.is_empty() {
            return Ok(ValidationResult::Pass);
        }

        let violations = drift_violations
            .into_iter()
            .map(|drift_violation| Violation {
                file_path: drift_violation.file_path,
                line: None,
                column: None,
                message: drift_violation.message,
                severity: ViolationSeverity::Error,
                rule_id: Some(match drift_violation.kind {
                    DriftKind::ScopeDrift => "truth_drift::scope".to_owned(),
                    DriftKind::ConstraintDrift => "truth_drift::constraint".to_owned(),
                }),
            })
            .collect();

        Ok(ValidationResult::Fail(violations))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{Strictness, TaskClass, TaskId};

    use super::*;

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
                    "+new line".to_owned(),
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
    fn clean_diff_passes_gate1() {
        let source_truth = make_source_truth(&[("out_of_scope", "src/payments.rs")]);
        let diff = make_diff(&["src/auth.rs"]);
        let result = TruthDriftGate::check(&source_truth, &diff).unwrap();
        assert!(matches!(result, ValidationResult::Pass));
    }

    #[test]
    fn out_of_scope_file_fails_gate1_with_scope_drift_rule_id() {
        let source_truth = make_source_truth(&[("out_of_scope", "src/payments.rs")]);
        let diff = make_diff(&["src/payments.rs"]);
        let result = TruthDriftGate::check(&source_truth, &diff).unwrap();
        let ValidationResult::Fail(violations) = result else {
            panic!("expected Fail");
        };
        assert!(violations
            .iter()
            .any(|violation| violation.rule_id.as_deref() == Some("truth_drift::scope")));
    }

    #[test]
    fn dependency_manifest_fails_gate1_with_constraint_drift_rule_id() {
        let source_truth = make_source_truth(&[("new_deps_allowed", "no")]);
        let diff = make_diff(&["Cargo.toml"]);
        let result = TruthDriftGate::check(&source_truth, &diff).unwrap();
        let ValidationResult::Fail(violations) = result else {
            panic!("expected Fail");
        };
        assert!(violations
            .iter()
            .any(|violation| violation.rule_id.as_deref() == Some("truth_drift::constraint")));
    }

    #[test]
    fn all_violations_have_error_severity() {
        let source_truth =
            make_source_truth(&[("out_of_scope", "src/foo.rs"), ("new_deps_allowed", "no")]);
        let diff = make_diff(&["src/foo.rs", "Cargo.toml"]);
        let ValidationResult::Fail(violations) =
            TruthDriftGate::check(&source_truth, &diff).unwrap()
        else {
            panic!("expected Fail");
        };
        assert!(violations
            .iter()
            .all(|violation| violation.severity == ViolationSeverity::Error));
    }
}
