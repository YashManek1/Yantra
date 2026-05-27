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
    /// A spawned subprocess failed to launch or produced unexpected output.
    #[error("subprocess error: {0}")]
    Subprocess(String),

    /// An I/O operation failed (file copy, diff write, directory scan).
    #[error("I/O error: {0}")]
    Io(String),

    /// The CRG SQLite database could not be opened or queried.
    #[error("CRG error: {0}")]
    Crg(String),

    /// Differential testing encountered an internal error.
    #[error("differential testing error: {0}")]
    Differential(String),
}

impl From<std::io::Error> for VerifierError {
    fn from(io_error: std::io::Error) -> Self {
        Self::Io(io_error.to_string())
    }
}

/// Convenience alias used throughout the verifier.
pub type VerifierResult<T> = Result<T, VerifierError>;
