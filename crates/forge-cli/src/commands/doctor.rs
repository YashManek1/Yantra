//! # forge-cli::commands::doctor: Runtime Preflight Health Check
//!
//! Runs a suite of preflight checks and prints a pass/fail summary so
//! users can diagnose environment issues before running tasks.
//!
//! ## Input
//! - `.yantra/` directory in the current working directory
//! - `--json` flag for machine-readable output
//!
//! ## Output
//! - One line per check: PASS / FAIL / WARN with a reason
//! - `--json` outputs a JSON array of check results
//!
//! ## Related
//! - `forge-obs` — telemetry field coverage check
//! - `forge-cli::commands::status` — shares CRG staleness logic

use clap::Args;
use serde::Serialize;
use tokio::net::TcpStream;

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Output as JSON instead of a formatted list.
    #[arg(long)]
    pub json: bool,
}

/// Outcome of a single preflight check.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckOutcome {
    /// The check passed without issues.
    Pass,
    /// The check found a non-fatal issue that may degrade functionality.
    Warn,
    /// The check found a fatal issue that must be resolved before running tasks.
    Fail,
}

/// Result of a single named preflight check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    /// Short machine-readable name for the check.
    pub name: String,
    /// Whether the check passed, warned, or failed.
    pub outcome: CheckOutcome,
    /// Human-readable explanation of the outcome.
    pub message: String,
}

impl CheckResult {
    /// Constructs a passing check result.
    fn pass(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            outcome: CheckOutcome::Pass,
            message: message.into(),
        }
    }

    /// Constructs a warning check result.
    fn warn(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            outcome: CheckOutcome::Warn,
            message: message.into(),
        }
    }

    /// Constructs a failing check result.
    fn fail(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            outcome: CheckOutcome::Fail,
            message: message.into(),
        }
    }
}

/// Runs all preflight checks and prints the results.
///
/// # Errors
///
/// Returns `anyhow::Error` if JSON serialization fails. All individual check
/// functions are infallible — they return a `CheckResult` even on error.
pub async fn run_doctor(args: DoctorArgs) -> anyhow::Result<()> {
    let mut check_results: Vec<CheckResult> = Vec::new();

    check_results.push(check_yantra_dir());
    check_results.push(check_crg_database());
    check_results.push(check_sacred_file());
    check_results.push(check_traces_database());
    check_results.push(check_ollama_reachable().await);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&check_results)?);
        return Ok(());
    }

    let pass_count = check_results
        .iter()
        .filter(|check| check.outcome == CheckOutcome::Pass)
        .count();
    let fail_count = check_results
        .iter()
        .filter(|check| check.outcome == CheckOutcome::Fail)
        .count();
    let warn_count = check_results
        .iter()
        .filter(|check| check.outcome == CheckOutcome::Warn)
        .count();

    println!("\nYantra Doctor — Preflight Check");
    println!("{:=<55}", "");
    for check in &check_results {
        let status_badge = match check.outcome {
            CheckOutcome::Pass => "✓ PASS",
            CheckOutcome::Warn => "⚠ WARN",
            CheckOutcome::Fail => "✗ FAIL",
        };
        println!("{status_badge:<8}  {:<30}  {}", check.name, check.message);
    }
    println!("{:=<55}", "");
    println!("{pass_count} passed  {warn_count} warned  {fail_count} failed");
    if fail_count > 0 {
        println!("Run `yantra index .` to fix CRG staleness issues.");
    }
    println!();

    Ok(())
}

/// Checks that the `.yantra/` directory exists in the current working directory.
fn check_yantra_dir() -> CheckResult {
    let yantra_path = std::path::Path::new(".yantra");
    if yantra_path.exists() && yantra_path.is_dir() {
        CheckResult::pass("yantra-dir", ".yantra/ directory exists")
    } else {
        CheckResult::fail(
            "yantra-dir",
            ".yantra/ not found — run `yantra index .` first",
        )
    }
}

/// Checks that `.yantra/crg.sqlite` exists, which is required for CRG operations.
fn check_crg_database() -> CheckResult {
    let crg_path = std::path::Path::new(".yantra").join("crg.sqlite");
    if crg_path.exists() {
        CheckResult::pass("crg-database", ".yantra/crg.sqlite exists")
    } else {
        CheckResult::warn(
            "crg-database",
            ".yantra/crg.sqlite missing — run `yantra index .`",
        )
    }
}

/// Checks that `.yantra/sacred.txt` exists; if absent, default patterns are used.
fn check_sacred_file() -> CheckResult {
    let sacred_path = std::path::Path::new(".yantra").join("sacred.txt");
    if sacred_path.exists() {
        CheckResult::pass("sacred-file", ".yantra/sacred.txt found")
    } else {
        CheckResult::warn(
            "sacred-file",
            ".yantra/sacred.txt not found — using defaults",
        )
    }
}

/// Checks that `.yantra/traces.sqlite` exists, which is created on the first run.
fn check_traces_database() -> CheckResult {
    let traces_path = std::path::Path::new(".yantra").join("traces.sqlite");
    if traces_path.exists() {
        CheckResult::pass("traces-database", ".yantra/traces.sqlite exists")
    } else {
        CheckResult::warn(
            "traces-database",
            ".yantra/traces.sqlite missing (no runs yet)",
        )
    }
}

/// Probes `127.0.0.1:11434` to check whether Ollama is running.
///
/// A failed connection is treated as a warning, not a failure, because Yantra
/// can operate in offline mode without Ollama.
async fn check_ollama_reachable() -> CheckResult {
    match TcpStream::connect("127.0.0.1:11434").await {
        Ok(_) => CheckResult::pass("ollama", "Ollama reachable on 127.0.0.1:11434"),
        Err(_) => CheckResult::warn("ollama", "Ollama not running (offline mode only)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_result_pass_sets_correct_outcome() {
        let check = CheckResult::pass("test-check", "all good");
        assert_eq!(check.outcome, CheckOutcome::Pass);
        assert_eq!(check.name, "test-check");
        assert_eq!(check.message, "all good");
    }

    #[test]
    fn check_result_warn_sets_correct_outcome() {
        let check = CheckResult::warn("partial-check", "degraded");
        assert_eq!(check.outcome, CheckOutcome::Warn);
    }

    #[test]
    fn check_result_fail_sets_correct_outcome() {
        let check = CheckResult::fail("missing-file", "not found");
        assert_eq!(check.outcome, CheckOutcome::Fail);
        assert_eq!(check.name, "missing-file");
    }

    #[tokio::test]
    async fn run_doctor_json_output_is_valid() {
        let args = DoctorArgs { json: true };
        let run_result = run_doctor(args).await;
        assert!(
            run_result.is_ok(),
            "doctor must not fail to run: {run_result:?}"
        );
    }

    #[tokio::test]
    async fn run_doctor_plain_output_does_not_fail() {
        let args = DoctorArgs { json: false };
        let run_result = run_doctor(args).await;
        assert!(
            run_result.is_ok(),
            "doctor plain output must not fail: {run_result:?}"
        );
    }
}
