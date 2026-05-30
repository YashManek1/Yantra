//! # forge-verifier: Shared Types
//!
//! Core data types shared across the three-gate verification pipeline:
//! `Diff`, `Violation`, `ValidationResult`, `VerificationContext`, and
//! `VerificationResult`.
//!
//! ## Input
//! - Consumed by every gate module and the pipeline coordinator
//!
//! ## Output
//! - `VerificationResult` returned to the orchestrator
//!
//! ## Related
//! - `forge-verifier::pipeline` — assembles the final `VerificationResult`
//! - `forge-verifier::gate1_truth_drift` — returns `ValidationResult`
//! - `forge-verifier::gate2_static_analysis` — produces `Vec<Violation>`
//! - `forge-verifier::hallucination` — uses `lsp_bridge` for Layer 2

use std::path::PathBuf;
use std::sync::Arc;

use yantra_core::TaskClass;
use yantra_lsp::LspBridge;

/// A proposed diff produced by a Coder or Refactorer agent.
#[derive(Debug, Clone)]
pub struct Diff {
    /// Raw unified-diff text (`--- a/…` / `+++ b/…` format).
    pub text: String,
    /// Task class that governs how strictly the diff is checked.
    pub task_class: TaskClass,
}

/// Indicates how severely a violation should block the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationSeverity {
    /// Blocks the pipeline; the diff must be revised.
    Error,
    /// Advisory; the pipeline continues but the Coder sees the warning.
    Warning,
}

/// A single issue found by any verification gate.
#[derive(Debug, Clone)]
pub struct Violation {
    /// File where the violation was found, if determinable.
    pub file_path: Option<String>,
    /// Line number within that file.
    pub line: Option<u32>,
    /// Column number within that line.
    pub column: Option<u32>,
    /// Human-readable description of the violation.
    pub message: String,
    /// Severity determining whether the pipeline is blocked.
    pub severity: ViolationSeverity,
    /// Machine-readable rule identifier (e.g., `"clippy::unused_variable"`).
    pub rule_id: Option<String>,
}

/// Outcome of a single gate check.
#[derive(Debug, Clone)]
pub enum ValidationResult {
    /// All checks passed.
    Pass,
    /// One or more violations were found.
    Fail(Vec<Violation>),
}

/// Everything a gate needs to locate and run tools against the project.
#[derive(Clone)]
pub struct VerificationContext {
    /// Root of the project being verified.
    pub project_root: PathBuf,
    /// `true` when the diff touches at least one sacred file.
    pub is_sacred_diff: bool,
    /// Path to the CRG SQLite database, used by the hallucination gate.
    pub crg_db_path: Option<PathBuf>,
    /// Optional LSP bridge for Layer 2 hallucination verification (go-to-definition
    /// cross-check). When `None`, the hallucination gate skips L2 and reports only
    /// the L1 CRG cross-check results.
    pub lsp_bridge: Option<Arc<LspBridge>>,
}

impl std::fmt::Debug for VerificationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerificationContext")
            .field("project_root", &self.project_root)
            .field("is_sacred_diff", &self.is_sacred_diff)
            .field("crg_db_path", &self.crg_db_path)
            .field(
                "lsp_bridge",
                &self.lsp_bridge.as_ref().map(|_| "<LspBridge>"),
            )
            .finish()
    }
}

/// Which gate rejected the diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationStage {
    /// Gate 1: Truth Drift Detector.
    TruthDrift,
    /// Gate 2: Static Analysis (clippy / mypy / tsc).
    StaticAnalysis,
    /// Gate 3: Boolean Exit Gate (test suite).
    BooleanExit,
    /// Hallucination cross-check (symbol / LSP / self-consistency).
    Hallucination,
}

/// Final verdict returned by `VerificationPipeline::verify`.
#[derive(Debug, Clone)]
pub enum VerificationResult {
    /// All gates passed; the diff may proceed to the Committer.
    Pass,
    /// A gate blocked the diff.
    Reject {
        /// The gate that produced the rejection.
        stage: VerificationStage,
        /// Structured violations for the Coder's revision loop.
        violations: Vec<Violation>,
    },
    /// The task exceeded `MAX_RETRIES` and must be handed to a human.
    NeedHumanReview {
        /// Why automated verification gave up.
        reason: String,
    },
}
