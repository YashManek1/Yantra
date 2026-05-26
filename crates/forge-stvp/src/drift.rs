//! # forge-stvp: Truth Drift Detector
//!
//! Parses a unified diff and checks whether the changes it describes violate
//! the constraints recorded in a `SourceTruth`. Violations indicate that the
//! agent's output has drifted from the validated task specification.
//!
//! ## Input
//! - `SourceTruth` — the task specification to enforce
//! - A raw unified-diff string produced by the Coder agent
//!
//! ## Output
//! - Zero or more `DriftViolation` values, each describing a constraint breach
//!
//! ## Related
//! - `forge-stvp::source_truth` — provides the `SourceTruth` type
//! - `forge-verifier` — calls `TruthDriftDetector::check` as one of its gates
//! - `forge-stvp::validation` — pre-execution validator (this module is post-execution)

use std::sync::LazyLock;

use regex::Regex;

use crate::source_truth::SourceTruth;

/// Category of drift detected between a diff and a `SourceTruth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftKind {
    /// A modified file is listed in `out_of_scope`.
    ScopeDrift,
    /// A dependency manifest was modified when `new_deps_allowed` is `"no"`.
    ConstraintDrift,
}

/// A single constraint violation found by `TruthDriftDetector`.
#[derive(Debug, Clone)]
pub struct DriftViolation {
    /// Category of the violation.
    pub kind: DriftKind,
    /// Human-readable description of the violation.
    pub message: String,
    /// Path of the file that caused the violation, if applicable.
    pub file_path: Option<String>,
}

/// Filenames that are treated as dependency manifests.
static DEPENDENCY_MANIFEST_NAMES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "requirements.txt",
    "Pipfile",
    "Pipfile.lock",
    "pyproject.toml",
    "go.mod",
    "go.sum",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
];

static PATTERN_DIFF_FILE_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\+\+\+ b/(.+)$").expect("diff file header pattern is valid"));

/// Checks a unified diff against a `SourceTruth` for constraint violations.
pub struct TruthDriftDetector;

impl TruthDriftDetector {
    /// Parses `diff_text` and returns every constraint violation found.
    pub fn check(truth: &SourceTruth, diff_text: &str) -> Vec<DriftViolation> {
        let modified_files = ParsedDiff::parse(diff_text);
        let mut violations = Vec::new();

        check_scope_drift(truth, &modified_files, &mut violations);
        check_constraint_drift(truth, &modified_files, &mut violations);

        violations
    }
}

/// Parsed representation of a unified diff: just the list of modified file paths.
pub struct ParsedDiff {
    /// All file paths that appear in `+++ b/<path>` headers of the diff.
    pub modified_files: Vec<String>,
}

impl ParsedDiff {
    /// Extracts modified file paths from a unified diff string.
    pub fn parse(diff_text: &str) -> Vec<String> {
        diff_text
            .lines()
            .filter_map(|line| {
                PATTERN_DIFF_FILE_HEADER
                    .captures(line)
                    .map(|cap| cap[1].to_owned())
            })
            .collect()
    }
}

fn check_scope_drift(
    truth: &SourceTruth,
    modified_files: &[String],
    violations: &mut Vec<DriftViolation>,
) {
    let out_of_scope_answer = match truth.answers.get("out_of_scope") {
        Some(answer) if !answer.trim().is_empty() => answer.trim().to_owned(),
        _ => return,
    };

    let out_of_scope_files: Vec<&str> = out_of_scope_answer
        .split([',', '\n'])
        .map(str::trim)
        .filter(|file_path| !file_path.is_empty())
        .collect();

    for modified_file in modified_files {
        if out_of_scope_files.contains(&modified_file.as_str()) {
            violations.push(DriftViolation {
                kind: DriftKind::ScopeDrift,
                message: format!("diff modifies '{modified_file}' which is listed in out_of_scope"),
                file_path: Some(modified_file.clone()),
            });
        }
    }
}

fn check_constraint_drift(
    truth: &SourceTruth,
    modified_files: &[String],
    violations: &mut Vec<DriftViolation>,
) {
    let deps_disallowed = truth
        .answers
        .get("new_deps_allowed")
        .is_some_and(|answer| answer.trim().eq_ignore_ascii_case("no"));

    if !deps_disallowed {
        return;
    }

    for modified_file in modified_files {
        let file_name = std::path::Path::new(modified_file)
            .file_name()
            .and_then(|os_str| os_str.to_str())
            .unwrap_or(modified_file.as_str());

        if DEPENDENCY_MANIFEST_NAMES.contains(&file_name) {
            violations.push(DriftViolation {
                kind: DriftKind::ConstraintDrift,
                message: format!(
                    "diff modifies dependency manifest '{modified_file}' \
                     but new_deps_allowed is 'no'"
                ),
                file_path: Some(modified_file.clone()),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{Strictness, TaskClass, TaskId};

    use super::*;

    fn make_truth(answers: &[(&str, &str)]) -> SourceTruth {
        SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::NewFeature,
            strictness: Strictness::Strict,
            description: "implement rate limiting".to_owned(),
            created_at: Utc::now(),
            answers: answers
                .iter()
                .map(|&(key, value)| (key.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn diff_touching(file_paths: &[&str]) -> String {
        file_paths
            .iter()
            .flat_map(|path| {
                [
                    format!("--- a/{path}"),
                    format!("+++ b/{path}"),
                    "@@ -1,3 +1,4 @@".to_owned(),
                    "+new line".to_owned(),
                ]
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parsed_diff_extracts_modified_file_paths() {
        let diff = diff_touching(&["src/auth.rs", "src/lib.rs"]);
        let modified_files = ParsedDiff::parse(&diff);
        assert_eq!(modified_files, vec!["src/auth.rs", "src/lib.rs"]);
    }

    #[test]
    fn scope_drift_flagged_when_diff_touches_out_of_scope_file() {
        let source_truth = make_truth(&[("out_of_scope", "src/payments.rs")]);
        let diff = diff_touching(&["src/auth.rs", "src/payments.rs"]);
        let violations = TruthDriftDetector::check(&source_truth, &diff);
        assert!(
            violations
                .iter()
                .any(|violation| violation.kind == DriftKind::ScopeDrift
                    && violation
                        .file_path
                        .as_deref()
                        .is_some_and(|path| path == "src/payments.rs")),
            "expected ScopeDrift for src/payments.rs"
        );
    }

    #[test]
    fn no_scope_drift_when_modified_files_are_in_scope() {
        let source_truth = make_truth(&[("out_of_scope", "src/payments.rs")]);
        let diff = diff_touching(&["src/auth.rs", "src/lib.rs"]);
        let violations = TruthDriftDetector::check(&source_truth, &diff);
        assert!(violations
            .iter()
            .all(|violation| violation.kind != DriftKind::ScopeDrift),);
    }

    #[test]
    fn constraint_drift_flagged_when_cargo_toml_modified_and_deps_disallowed() {
        let source_truth = make_truth(&[("new_deps_allowed", "no")]);
        let diff = diff_touching(&["src/lib.rs", "Cargo.toml"]);
        let violations = TruthDriftDetector::check(&source_truth, &diff);
        assert!(
            violations
                .iter()
                .any(|violation| violation.kind == DriftKind::ConstraintDrift
                    && violation
                        .file_path
                        .as_deref()
                        .is_some_and(|path| path == "Cargo.toml")),
            "expected ConstraintDrift for Cargo.toml"
        );
    }

    #[test]
    fn no_constraint_drift_when_deps_allowed() {
        let source_truth = make_truth(&[("new_deps_allowed", "yes")]);
        let diff = diff_touching(&["Cargo.toml"]);
        let violations = TruthDriftDetector::check(&source_truth, &diff);
        assert!(violations
            .iter()
            .all(|violation| violation.kind != DriftKind::ConstraintDrift),);
    }

    #[test]
    fn nested_cargo_toml_path_detected_as_manifest() {
        let source_truth = make_truth(&[("new_deps_allowed", "no")]);
        let diff = diff_touching(&["crates/forge-stvp/Cargo.toml"]);
        let violations = TruthDriftDetector::check(&source_truth, &diff);
        assert!(
            violations
                .iter()
                .any(|violation| violation.kind == DriftKind::ConstraintDrift),
            "nested Cargo.toml must still be flagged"
        );
    }
}
