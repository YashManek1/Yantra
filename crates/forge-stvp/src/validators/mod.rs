//! # forge-stvp: Validator Implementations
//!
//! Houses the three concrete validators that `validation::run_all` selects
//! based on the `Strictness` mode recorded in the `SourceTruth`.
//!
//! ## Validators
//! - `InternalConsistencyValidator` — answer-to-answer contradiction checks
//! - `CodebaseRealityValidator` — filesystem existence and AST parse checks
//! - `TestabilityValidator` — success criterion specificity checks
//!
//! ## Related
//! - `forge-stvp::validation` — Validator trait, run_all(), ValidationReport

pub mod codebase_reality;
pub mod consistency;
pub mod testability;

pub use codebase_reality::CodebaseRealityValidator;
pub use consistency::InternalConsistencyValidator;
pub use testability::TestabilityValidator;
