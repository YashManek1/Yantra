//! # forge-verifier: Subprocess Execution Helper
//!
//! Thin async wrapper around `tokio::process::Command` used by every gate
//! that needs to spawn an external tool (cargo, mypy, ruff, tsc, eslint).
//!
//! ## Input
//! - Program name, argument slice, and working directory path
//!
//! ## Output
//! - `ProcessOutput` containing stdout, stderr, and exit status
//!
//! ## Related
//! - `forge-verifier::gate2_static_analysis` — primary caller
//! - `forge-verifier::gate3_boolean_exit` — runs the test suite via this helper
//! - `forge-verifier::differential_testing` — applies diffs and runs tests

use std::path::Path;

use tokio::process::Command;

use crate::error::{VerifierError, VerifierResult};

/// Raw output captured from a child process.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    /// Text written to stdout (UTF-8, best-effort).
    pub stdout: String,
    /// Text written to stderr (UTF-8, best-effort).
    pub stderr: String,
    /// `true` when the process exited with code 0.
    pub exit_success: bool,
}

/// Spawns `program` with `args` in `working_dir`, captures stdout and stderr,
/// and returns a `ProcessOutput` regardless of exit code.
///
/// Returns `Err(VerifierError::Subprocess)` only when the process could not be
/// spawned at all (binary not found, permission denied, etc.).
pub async fn spawn_command(
    program: &str,
    args: &[&str],
    working_dir: &Path,
) -> VerifierResult<ProcessOutput> {
    let output = Command::new(program)
        .args(args)
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|io_error| {
            VerifierError::Subprocess(format!("failed to spawn '{program}': {io_error}"))
        })?;

    Ok(ProcessOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_success: output.status.success(),
    })
}
