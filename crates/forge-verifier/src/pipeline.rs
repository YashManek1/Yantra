//! # forge-verifier: Verification Pipeline
//!
//! Runs the three gates (Truth Drift → Static Analysis → Boolean Exit +
//! Hallucination) in sequence. The first gate that produces an `Error`-severity
//! violation short-circuits the pipeline and returns a `Reject` result.
//!
//! After `MAX_RETRIES` failed attempts the pipeline escalates to
//! `NeedHumanReview` so that a human can inspect the task.
//!
//! ## Input
//! - `Diff` — proposed change from the Coder agent
//! - `SourceTruth` — the STVP-signed task specification
//! - `VerificationContext` — project root and CRG db path
//! - `retry_count` — how many times this diff has already been through the pipeline
//!
//! ## Output
//! - `VerificationResult::Pass` — all gates passed; proceed to Committer
//! - `VerificationResult::Reject { stage, violations }` — structured feedback
//! - `VerificationResult::NeedHumanReview { reason }` — retries exhausted
//!
//! ## Related
//! - `forge-verifier::gate1_truth_drift` — gate 1
//! - `forge-verifier::gate2_static_analysis` — gate 2
//! - `forge-verifier::gate3_boolean_exit` — gate 3
//! - `forge-verifier::hallucination` — companion to gate 3
//! - `forge-orchestrator` — calls `VerificationPipeline::verify` after each diff

use yantra_core::TaskClass;
use yantra_stvp::SourceTruth;

use crate::differential_testing::{check_differential, DifferentialReport};
use crate::error::VerifierResult;
use crate::gate1_truth_drift::TruthDriftGate;
use crate::gate2_static_analysis::StaticAnalysisGate;
use crate::gate3_boolean_exit::BooleanExitGate;
use crate::hallucination::check_hallucination;
use crate::types::{
    Diff, ValidationResult, VerificationContext, VerificationResult, VerificationStage, Violation,
    ViolationSeverity,
};

/// Maximum number of verification attempts before escalating to human review.
const MAX_RETRIES: u32 = 3;

/// Orchestrates the three verification gates for a single diff.
pub struct VerificationPipeline;

impl VerificationPipeline {
    /// Runs all verification gates in order.
    ///
    /// `retry_count` is the number of times this specific diff (or a previous
    /// revision of it) has already failed verification.  When `retry_count >=
    /// MAX_RETRIES` the pipeline immediately returns `NeedHumanReview`.
    pub async fn verify(
        diff: &Diff,
        truth: &SourceTruth,
        ctx: &VerificationContext,
        retry_count: u32,
    ) -> VerifierResult<VerificationResult> {
        if retry_count >= MAX_RETRIES {
            return Ok(VerificationResult::NeedHumanReview {
                reason: format!(
                    "task reached maximum of {MAX_RETRIES} verification retries without passing"
                ),
            });
        }

        // Gate 1: Truth Drift
        match TruthDriftGate::check(truth, diff)? {
            ValidationResult::Fail(drift_violations) => {
                let has_blocking = drift_violations
                    .iter()
                    .any(|v| v.severity == ViolationSeverity::Error);
                if has_blocking {
                    return Ok(VerificationResult::Reject {
                        stage: VerificationStage::TruthDrift,
                        violations: drift_violations,
                    });
                }
            }
            ValidationResult::Pass => {}
        }

        // Gate 2: Static Analysis
        match StaticAnalysisGate::check(diff, ctx).await? {
            ValidationResult::Fail(static_analysis_violations) => {
                return Ok(VerificationResult::Reject {
                    stage: VerificationStage::StaticAnalysis,
                    violations: static_analysis_violations,
                });
            }
            ValidationResult::Pass => {}
        }

        // Hallucination cross-check (advisory — only blocks on Error severity)
        let hallucination_violations = check_hallucination(diff, ctx).await?;
        let blocking_hallucinations: Vec<Violation> = hallucination_violations
            .into_iter()
            .filter(|violation| violation.severity == ViolationSeverity::Error)
            .collect();

        if !blocking_hallucinations.is_empty() {
            return Ok(VerificationResult::Reject {
                stage: VerificationStage::Hallucination,
                violations: blocking_hallucinations,
            });
        }

        // Gate 3: Boolean Exit Gate
        match BooleanExitGate::check(ctx).await? {
            ValidationResult::Fail(test_violations) => {
                return Ok(VerificationResult::Reject {
                    stage: VerificationStage::BooleanExit,
                    violations: test_violations,
                });
            }
            ValidationResult::Pass => {}
        }

        // Differential testing for refactor-class diffs
        if diff.task_class == TaskClass::Refactor {
            let input_count = if ctx.is_sacred_diff { 1000 } else { 100 };
            let differential_report =
                check_differential(diff, &ctx.project_root, input_count).await;

            match differential_report {
                DifferentialReport::Reject {
                    diverging_test,
                    before_outcome,
                    after_outcome,
                } => {
                    return Ok(VerificationResult::Reject {
                        stage: VerificationStage::BooleanExit,
                        violations: vec![Violation {
                            file_path: None,
                            line: None,
                            column: None,
                            message: format!(
                                "differential test '{diverging_test}' diverged: \
                                 before={before_outcome:?}, after={after_outcome:?}"
                            ),
                            severity: ViolationSeverity::Error,
                            rule_id: Some("differential::divergence".to_owned()),
                        }],
                    });
                }
                DifferentialReport::Error { reason } => {
                    tracing::warn!("differential testing infrastructure error: {reason}");
                }
                DifferentialReport::Equivalent => {}
            }
        }

        Ok(VerificationResult::Pass)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

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
                    "+// new comment".to_owned(),
                ]
            })
            .collect::<Vec<_>>()
            .join("\n");

        Diff {
            text: diff_text,
            task_class: TaskClass::NewFeature,
        }
    }

    fn no_subprocess_ctx() -> VerificationContext {
        VerificationContext {
            project_root: PathBuf::from("/nonexistent"),
            is_sacred_diff: false,
            crg_db_path: None,
            lsp_bridge: None,
        }
    }

    #[tokio::test]
    async fn pipeline_rejects_immediately_at_max_retries() {
        let source_truth = make_source_truth(&[]);
        let diff = make_diff(&["src/lib.rs"]);
        let result = VerificationPipeline::verify(&diff, &source_truth, &no_subprocess_ctx(), 3)
            .await
            .unwrap();
        assert!(matches!(result, VerificationResult::NeedHumanReview { .. }));
    }

    #[tokio::test]
    async fn pipeline_fails_at_gate1_on_scope_drift() {
        let source_truth = make_source_truth(&[("out_of_scope", "src/payments.rs")]);
        let diff = make_diff(&["src/payments.rs"]);
        let result = VerificationPipeline::verify(&diff, &source_truth, &no_subprocess_ctx(), 0)
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
            "expected TruthDrift rejection"
        );
    }
}
