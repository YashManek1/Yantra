//! # forge-stvp: Source-Truth Validation Protocol
//!
//! Refuses to let any task proceed without a cryptographically signed
//! `TruthToken`. The `Interrogator` classifies the incoming task, runs a
//! tailored questionnaire, builds `SOURCE_TRUTH.yaml`, and stores it with
//! SHA-256 content-hash verification.
//!
//! ## Input
//! - A natural-language task description from the user or Night Mode planner
//! - User answers to the dynamic questionnaire (Strict: 5-7 q, Light: 3 q,
//!   Trust: none)
//!
//! ## Output
//! - `SourceTruth` — validated task specification stored in
//!   `.yantra/source_truth/{task_id}.yaml`
//! - `TruthToken` issuance is the orchestrator's responsibility after receiving
//!   the `SourceTruth` (this crate produces the source truth; the orchestrator
//!   signs the token)
//!
//! ## Related
//! - `forge-crg` — provides `existing_truth_refs` for Validator 2 (v2)
//! - `forge-memory` — Truth Vault stores and retrieves signed artifacts (v2)
//! - `forge-orchestrator` — verifies the token before scheduling any task
//! - `forge-verifier` — Truth Drift Detector checks diffs against the token

pub mod classifier;
pub mod error;
pub mod interrogator;
pub mod questionnaire;
pub mod source_truth;

pub use classifier::TaskClassifier;
pub use error::StvpError;
pub use interrogator::{strictness_for_class, Interrogator, QuestionnaireUi};
pub use questionnaire::{AnswerKind, Question};
pub use source_truth::SourceTruth;
