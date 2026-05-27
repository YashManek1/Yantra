//! # forge-verifier: Differential Testing
//!
//! For refactor-class diffs, verifies behavioural equivalence by copying the
//! project into `before/` and `after/` directories, applying the diff to the
//! `after/` copy, running `cargo test` in both forks, and comparing the sets
//! of failing tests. A divergence means the refactor changed observable
//! behaviour and the diff is rejected.
//!
//! ## Input
//! - `Diff` — the proposed refactor change
//! - `project_root` — root of the project to fork
//! - `input_count` — hint for how many inputs to generate (currently unused;
//!   the comparison is based on the test suite, not generated inputs)
//!
//! ## Output
//! - `DifferentialReport::Equivalent` — test suites agree in both forks
//! - `DifferentialReport::Reject { … }` — a test changed outcome
//! - `DifferentialReport::Error { reason }` — infrastructure failure
//!
//! ## Related
//! - `forge-verifier::pipeline` — calls this for `TaskClass::Refactor` diffs
//! - `forge-verifier::subprocess` — used to run cargo test in each fork
//! - `forge-verifier::gate3_boolean_exit` — `parse_test_outcome` is reused here

use std::path::{Path, PathBuf};

use crate::gate3_boolean_exit::parse_test_outcome;
use crate::subprocess::spawn_command;
use crate::types::Diff;

/// Whether the before-fork and after-fork test suites agreed.
#[derive(Debug, Clone)]
pub enum DifferentialReport {
    /// Both forks produced the same set of passing/failing tests.
    Equivalent,
    /// A specific test changed from passing to failing (or vice versa).
    Reject {
        /// Name of the test that diverged.
        diverging_test: String,
        /// Outcome in the before-fork.
        before_outcome: TestOutcomeLabel,
        /// Outcome in the after-fork.
        after_outcome: TestOutcomeLabel,
    },
    /// Infrastructure error (copy failed, diff could not be applied, etc.).
    Error {
        /// Human-readable reason for the failure.
        reason: String,
    },
}

/// Pass/fail label for one test in one fork.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOutcomeLabel {
    Passed,
    Failed,
}

/// Runs differential testing for a refactor-class diff.
///
/// `input_count` is accepted for API symmetry with the spec; the current
/// implementation compares full test-suite outcomes rather than per-input runs.
pub async fn check_differential(
    refactor_diff: &Diff,
    project_root: &Path,
    _input_count: usize,
) -> DifferentialReport {
    let timestamp_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let work_dir = std::env::temp_dir().join(format!(
        "yantra-diff-{}-{}",
        std::process::id(),
        timestamp_millis
    ));

    let before_dir = work_dir.join("before");
    let after_dir = work_dir.join("after");

    if let Err(copy_error) = copy_directory_recursive(project_root, &before_dir) {
        return DifferentialReport::Error {
            reason: format!("failed to copy project to before/: {copy_error}"),
        };
    }

    if let Err(copy_error) = copy_directory_recursive(project_root, &after_dir) {
        return DifferentialReport::Error {
            reason: format!("failed to copy project to after/: {copy_error}"),
        };
    }

    if let Err(apply_error) = apply_diff_to_directory(&refactor_diff.text, &after_dir).await {
        return DifferentialReport::Error {
            reason: format!("failed to apply diff to after/: {apply_error}"),
        };
    }

    let before_output = match spawn_command("cargo", &["test"], &before_dir).await {
        Ok(process_output) => format!("{}\n{}", process_output.stdout, process_output.stderr),
        Err(subprocess_error) => {
            return DifferentialReport::Error {
                reason: format!("cargo test in before/ failed: {subprocess_error}"),
            };
        }
    };

    let after_output = match spawn_command("cargo", &["test"], &after_dir).await {
        Ok(process_output) => format!("{}\n{}", process_output.stdout, process_output.stderr),
        Err(subprocess_error) => {
            return DifferentialReport::Error {
                reason: format!("cargo test in after/ failed: {subprocess_error}"),
            };
        }
    };

    let differential_report = compare_test_outcomes(&before_output, &after_output);

    let _ = std::fs::remove_dir_all(&work_dir);
    differential_report
}

/// Copies `source` to `dest`, skipping `target/` and `.git/` directories.
pub fn copy_directory_recursive(source: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;

    for directory_entry in std::fs::read_dir(source)? {
        let directory_entry = directory_entry?;
        let entry_path = directory_entry.path();
        let file_name = directory_entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        if file_name_str == "target" || file_name_str == ".git" {
            continue;
        }

        let dest_path: PathBuf = dest.join(&file_name);

        if entry_path.is_dir() {
            copy_directory_recursive(&entry_path, &dest_path)?;
        } else {
            std::fs::copy(&entry_path, &dest_path)?;
        }
    }

    Ok(())
}

/// Writes `diff_text` to a temporary file in `target_dir` and applies it with
/// `git apply --reject`.
pub async fn apply_diff_to_directory(diff_text: &str, target_dir: &Path) -> Result<(), String> {
    let patch_file_path = target_dir.join(".yantra_patch.diff");
    std::fs::write(&patch_file_path, diff_text)
        .map_err(|io_error| format!("could not write patch file: {io_error}"))?;

    let apply_output = spawn_command(
        "git",
        &["apply", "--reject", ".yantra_patch.diff"],
        target_dir,
    )
    .await
    .map_err(|verifier_error| verifier_error.to_string())?;

    let _ = std::fs::remove_file(&patch_file_path);

    if apply_output.exit_success {
        Ok(())
    } else {
        Err(format!("git apply failed:\n{}", apply_output.stderr))
    }
}

/// Compares `cargo test` output from before and after forks.
pub fn compare_test_outcomes(before_output: &str, after_output: &str) -> DifferentialReport {
    let before_outcome = parse_test_outcome(before_output);
    let after_outcome = parse_test_outcome(after_output);

    let before_failing: std::collections::HashSet<&str> = before_outcome
        .failing_tests
        .iter()
        .map(|failing_test| failing_test.test_name.as_str())
        .collect();

    let after_failing: std::collections::HashSet<&str> = after_outcome
        .failing_tests
        .iter()
        .map(|failing_test| failing_test.test_name.as_str())
        .collect();

    for &after_failed_test in &after_failing {
        if !before_failing.contains(after_failed_test) {
            return DifferentialReport::Reject {
                diverging_test: after_failed_test.to_owned(),
                before_outcome: TestOutcomeLabel::Passed,
                after_outcome: TestOutcomeLabel::Failed,
            };
        }
    }

    for &before_failed_test in &before_failing {
        if !after_failing.contains(before_failed_test) {
            return DifferentialReport::Reject {
                diverging_test: before_failed_test.to_owned(),
                before_outcome: TestOutcomeLabel::Failed,
                after_outcome: TestOutcomeLabel::Passed,
            };
        }
    }

    DifferentialReport::Equivalent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalent_suites_report_equivalent() {
        let passing_suite =
            "test foo ... ok\ntest bar ... ok\n\ntest result: ok. 2 passed; 0 failed;";
        let differential_report = compare_test_outcomes(passing_suite, passing_suite);
        assert!(matches!(
            differential_report,
            DifferentialReport::Equivalent
        ));
    }

    #[test]
    fn newly_failing_test_reports_reject() {
        let before_output = "test foo ... ok\n\ntest result: ok. 1 passed; 0 failed;";
        let after_output = "test foo ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed;";
        let differential_report = compare_test_outcomes(before_output, after_output);
        let DifferentialReport::Reject {
            diverging_test,
            before_outcome,
            after_outcome,
        } = differential_report
        else {
            panic!("expected Reject");
        };
        assert_eq!(diverging_test, "foo");
        assert_eq!(before_outcome, TestOutcomeLabel::Passed);
        assert_eq!(after_outcome, TestOutcomeLabel::Failed);
    }

    #[test]
    fn newly_passing_test_also_reports_reject() {
        let before_output = "test foo ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed;";
        let after_output = "test foo ... ok\n\ntest result: ok. 1 passed; 0 failed;";
        let differential_report = compare_test_outcomes(before_output, after_output);
        let DifferentialReport::Reject {
            before_outcome,
            after_outcome,
            ..
        } = differential_report
        else {
            panic!("expected Reject");
        };
        assert_eq!(before_outcome, TestOutcomeLabel::Failed);
        assert_eq!(after_outcome, TestOutcomeLabel::Passed);
    }
}
