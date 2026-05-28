//! # forge-verifier: Error Types
//!
//! Typed error enum for all failure modes in the three-gate verification
//! pipeline. Uses `thiserror` so callers can pattern-match on cause.
//!
//! ## Input
//! - Wrapped errors from subprocess execution, I/O, CRG, and diffing
//!
//! ## Output
//! - `VerifierError` variants with structured messages
//!
//! ## Related
//! - `forge-verifier::pipeline` — produces these errors on gate failure
//! - `forge-verifier::subprocess` — wraps I/O errors here

use thiserror::Error;

/// All error cases produced by the forge-verifier pipeline.
#[derive(Debug, Error)]
pub enum VerifierError {
    /// A spawned subprocess (`cargo test`, `cargo clippy`, etc.) failed to
    /// launch or exited with a non-zero status.
    ///
    /// The inner string includes the command name, exit code, and stderr output
    /// so the caller can surface exactly what broke without re-running.
    #[error("subprocess failed: {0}")]
    Subprocess(String),

    /// A filesystem operation required by the verification pipeline failed.
    ///
    /// Common causes: the project root is read-only, a diff file could not be
    /// written to a temp directory, or a directory scan encountered a
    /// permissions error.
    #[error("I/O error during verification: {0}")]
    Io(String),

    /// The CRG SQLite database could not be opened or queried during Truth
    /// Drift detection.
    ///
    /// Ensure `yantra index .` has been run and `.yantra/crg.sqlite` exists.
    #[error("CRG database error during Truth Drift check: {0}")]
    Crg(String),

    /// An internal error occurred in the differential testing gate.
    ///
    /// This indicates a bug in the verifier's diffing logic rather than a
    /// problem with the code under review.
    #[error("differential testing gate internal error: {0}")]
    Differential(String),
}

impl From<std::io::Error> for VerifierError {
    fn from(io_error: std::io::Error) -> Self {
        Self::Io(io_error.to_string())
    }
}

/// Convenience alias used throughout the verifier.
pub type VerifierResult<T> = Result<T, VerifierError>;
