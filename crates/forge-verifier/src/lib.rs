//! # forge-verifier: Three-Gate Diff Verification
//!
//! Every diff produced by a Coder agent passes three sequential gates before
//! it can be handed to the Committer: Truth Drift Detection (did the diff
//! stay within `STVP` scope?), Static Analysis (`clippy`/`mypy`/`tsc`), and the
//! Boolean Exit Gate (all tests pass, no hallucinated symbols).
//!
//! ## Input
//! - A proposed diff from a Coder or Refactorer agent
//! - The `SourceTruth` that governs the current task
//! - `VerificationContext` carrying project root and CRG db path
//!
//! ## Output
//! - `VerificationResult::Pass` — diff is forwarded to the Committer
//! - `VerificationResult::Reject { stage, violations }` — diff is returned to
//!   the originating agent for revision, with structured failure violations
//! - `VerificationResult::NeedHumanReview { reason }` — retries exhausted
//!
//! ## Related
//! - `forge-stvp` — provides `SourceTruth` and `TruthDriftDetector`
//! - `forge-crg` — graph queries power the hallucination cross-check
//! - `forge-lsp` — LSP diagnostics feed the static-analysis gate

pub mod differential_testing;
pub mod error;
pub mod gate1_truth_drift;
pub mod gate2_static_analysis;
pub mod gate3_boolean_exit;
pub mod hallucination;
pub mod pipeline;
pub mod subprocess;
pub mod types;

pub use differential_testing::{
    check_differential, compare_test_outcomes, DifferentialReport, TestOutcomeLabel,
};
pub use error::{VerifierError, VerifierResult};
pub use gate1_truth_drift::TruthDriftGate;
pub use gate2_static_analysis::{
    detect_analysis_languages, parse_cargo_json_output, parse_eslint_json_output,
    parse_mypy_output, parse_ruff_json_output, parse_tsc_output, AnalysisLanguage,
    StaticAnalysisGate,
};
pub use gate3_boolean_exit::{parse_test_outcome, BooleanExitGate, FailingTest, TestOutcome};
pub use hallucination::{
    ast_symbol_cross_check, check_hallucination, hallucinated_symbols_to_violations,
    load_crg_symbol_names, HallucinatedSymbol,
};
pub use pipeline::VerificationPipeline;
pub use types::{
    Diff, ValidationResult, VerificationContext, VerificationResult, VerificationStage, Violation,
    ViolationSeverity,
};
