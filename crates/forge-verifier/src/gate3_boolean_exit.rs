//! # forge-verifier: Gate 3 — Boolean Exit Gate
//!
//! Runs the project's test suite via `cargo test` and issues a binary
//! pass/fail verdict. On failure, failing test names and error messages are
//! extracted and returned to the Coder for revision.
//!
//! ## Input
//! - `VerificationContext` — project root where the test suite lives
//!
//! ## Output
//! - `ValidationResult::Pass` — all tests passed
//! - `ValidationResult::Fail(violations)` — one or more tests failed
//!
//! ## Related
//! - `forge-verifier::subprocess` — spawns `cargo test`
//! - `forge-verifier::pipeline` — calls this as gate 3 (after static analysis)

use std::sync::LazyLock;

use regex::Regex;

use crate::error::VerifierResult;
use crate::subprocess::spawn_command;
use crate::types::{ValidationResult, VerificationContext, Violation, ViolationSeverity};

static PATTERN_FAILING_TEST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^test (.+) \.\.\. FAILED$").expect("failing test pattern is valid")
});

static PATTERN_PANIC_LOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*thread '.+' panicked at (.+?):(\d+):\d+")
        .expect("panic location pattern is valid")
});

/// A single failing test case extracted from cargo test output.
#[derive(Debug, Clone)]
pub struct FailingTest {
    /// Fully qualified test name as printed by cargo test.
    pub test_name: String,
    /// Source file where the panic occurred, if determinable.
    pub file_path: Option<String>,
    /// Line number of the panic site.
    pub line_number: Option<u32>,
    /// First panic message or assertion failure text.
    pub error_message: String,
}

/// Binary result of running the test suite.
#[derive(Debug, Clone)]
pub struct TestOutcome {
    /// `true` when all tests passed (exit code 0).
    pub all_passed: bool,
    /// Failing tests with their error messages.
    pub failing_tests: Vec<FailingTest>,
}

/// Gate 3: runs the full test suite and returns a pass/fail verdict.
pub struct BooleanExitGate;

impl BooleanExitGate {
    /// Executes `cargo test` in the project root and returns a verdict.
    pub async fn check(ctx: &VerificationContext) -> VerifierResult<ValidationResult> {
        let test_output =
            spawn_command("cargo", &["test", "--", "--nocapture"], &ctx.project_root).await?;

        let combined_output = format!("{}\n{}", test_output.stdout, test_output.stderr);
        let test_outcome = parse_test_outcome(&combined_output);

        if test_outcome.all_passed {
            return Ok(ValidationResult::Pass);
        }

        let violations = test_outcome
            .failing_tests
            .into_iter()
            .map(|failing_test| Violation {
                file_path: failing_test.file_path,
                line: failing_test.line_number,
                column: None,
                message: format!(
                    "test '{}' failed: {}",
                    failing_test.test_name, failing_test.error_message
                ),
                severity: ViolationSeverity::Error,
                rule_id: Some("gate3::test_failure".to_owned()),
            })
            .collect();

        Ok(ValidationResult::Fail(violations))
    }
}

/// Parses the text output of `cargo test` into a `TestOutcome`.
pub fn parse_test_outcome(test_output: &str) -> TestOutcome {
    let mut failing_test_names: Vec<String> = Vec::new();

    for output_line in test_output.lines() {
        if let Some(captures) = PATTERN_FAILING_TEST.captures(output_line) {
            failing_test_names.push(captures[1].to_owned());
        }
    }

    let all_passed = failing_test_names.is_empty() && test_output.contains("test result: ok");

    let mut failing_tests: Vec<FailingTest> = failing_test_names
        .into_iter()
        .map(|test_name| {
            let panic_location = extract_panic_location(test_output);
            FailingTest {
                test_name,
                file_path: panic_location.as_ref().map(|(path, _)| path.clone()),
                line_number: panic_location.as_ref().map(|(_, line)| *line),
                error_message: extract_first_assertion_message(test_output),
            }
        })
        .collect();

    if !all_passed && failing_tests.is_empty() && test_output.contains("FAILED") {
        failing_tests.push(FailingTest {
            test_name: "unknown".to_owned(),
            file_path: None,
            line_number: None,
            error_message: "test suite reported failure".to_owned(),
        });
    }

    TestOutcome {
        all_passed,
        failing_tests,
    }
}

fn extract_panic_location(test_output: &str) -> Option<(String, u32)> {
    for output_line in test_output.lines() {
        if let Some(captures) = PATTERN_PANIC_LOCATION.captures(output_line) {
            let file_path = captures[1].to_owned();
            let line_number = captures[2].parse::<u32>().ok()?;
            return Some((file_path, line_number));
        }
    }
    None
}

fn extract_first_assertion_message(test_output: &str) -> String {
    for output_line in test_output.lines() {
        let trimmed_line = output_line.trim();
        if trimmed_line.starts_with("thread '") && trimmed_line.contains("panicked at") {
            return trimmed_line.to_owned();
        }
        if trimmed_line.starts_with("assertion") && trimmed_line.contains("failed") {
            return trimmed_line.to_owned();
        }
    }
    "test failed".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_test_outcome_detects_passing_suite() {
        let output = "running 3 tests\ntest foo ... ok\ntest bar ... ok\ntest baz ... ok\n\ntest result: ok. 3 passed; 0 failed;";
        let test_outcome = parse_test_outcome(output);
        assert!(test_outcome.all_passed);
        assert!(test_outcome.failing_tests.is_empty());
    }

    #[test]
    fn parse_test_outcome_extracts_failing_test_names() {
        let output = "running 2 tests\ntest auth::login_works ... FAILED\ntest auth::logout_works ... ok\n\ntest result: FAILED. 1 passed; 1 failed;";
        let test_outcome = parse_test_outcome(output);
        assert!(!test_outcome.all_passed);
        assert_eq!(test_outcome.failing_tests.len(), 1);
        assert_eq!(test_outcome.failing_tests[0].test_name, "auth::login_works");
    }

    #[test]
    fn parse_test_outcome_extracts_panic_location() {
        let output = "test foo ... FAILED\n  thread 'foo' panicked at src/auth.rs:42:5\n\ntest result: FAILED. 0 passed; 1 failed;";
        let test_outcome = parse_test_outcome(output);
        assert!(!test_outcome.all_passed);
        let first_failure = &test_outcome.failing_tests[0];
        assert_eq!(first_failure.file_path.as_deref(), Some("src/auth.rs"));
        assert_eq!(first_failure.line_number, Some(42));
    }

    #[test]
    fn parse_test_outcome_empty_output_fails() {
        let test_outcome = parse_test_outcome("error: could not compile");
        assert!(!test_outcome.all_passed);
    }
}
