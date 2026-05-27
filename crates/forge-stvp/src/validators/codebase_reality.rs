//! # forge-stvp: Codebase Reality Validator
//!
//! Checks that every file path referenced in the `SourceTruth` answers
//! (`primary_files`, `pattern_files`, `suspected_files`) exists on disk and,
//! for supported languages, can be parsed by the Tree-sitter AST layer.
//!
//! ## Input
//! - `SourceTruth` — answer keys checked: `primary_files`, `pattern_files`,
//!   `suspected_files`
//! - `ProjectContext` — provides the project root for path resolution
//!
//! ## Output
//! - Zero or more `ValidationViolation` values
//!
//! ## Related
//! - `forge-stvp::validation` — Validator trait and ValidationReport
//! - `forge-ast::parser` — `parse_file` used to AST-check referenced sources

use std::path::Path;

use yantra_ast::{parse_file, AstError};

use crate::source_truth::SourceTruth;
use crate::validation::{ProjectContext, ValidationViolation, Validator, ViolationSeverity};

/// Validates that files referenced in the source truth exist and are parseable.
pub struct CodebaseRealityValidator;

const FILE_LIST_KEYS: &[&str] = &["primary_files", "pattern_files", "suspected_files"];

impl Validator for CodebaseRealityValidator {
    fn validate(&self, truth: &SourceTruth, context: &ProjectContext) -> Vec<ValidationViolation> {
        let mut violations = Vec::new();
        let project_root = context.project_root.as_path();

        for &answer_key in FILE_LIST_KEYS {
            if let Some(raw_list) = truth.answers.get(answer_key) {
                let file_paths: Vec<&str> = raw_list
                    .split([',', '\n'])
                    .map(str::trim)
                    .filter(|file_path| !file_path.is_empty())
                    .collect();

                for file_path_str in file_paths {
                    let absolute_path = project_root.join(file_path_str);
                    check_file_reality(file_path_str, &absolute_path, &mut violations);
                }
            }
        }

        violations
    }
}

fn check_file_reality(
    relative_path: &str,
    absolute_path: &Path,
    violations: &mut Vec<ValidationViolation>,
) {
    if !absolute_path.exists() {
        violations.push(ValidationViolation {
            validator: "CodebaseReality",
            message: format!("referenced file does not exist: '{relative_path}'"),
            severity: ViolationSeverity::Error,
        });
        return;
    }

    match parse_file(absolute_path) {
        Ok(_) => {}
        Err(AstError::UnsupportedLanguage { .. }) => {
            // Non-Rust/Python/TS files are fine; no parse check possible
        }
        Err(parse_error) => {
            violations.push(ValidationViolation {
                validator: "CodebaseReality",
                message: format!("'{relative_path}' could not be parsed: {parse_error}"),
                severity: ViolationSeverity::Warning,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{ProjectRoot, Strictness, TaskClass, TaskId};

    use super::*;
    use crate::validation::ProjectContext;

    fn make_truth_with_files(answer_key: &str, file_list: &str) -> SourceTruth {
        let mut answers = BTreeMap::new();
        answers.insert(answer_key.to_owned(), file_list.to_owned());
        SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::NewFeature,
            strictness: Strictness::Strict,
            description: "test task".to_owned(),
            created_at: Utc::now(),
            answers,
            augmented_question_ids: Vec::new(),
        }
    }

    fn temp_context() -> (ProjectContext, std::path::PathBuf) {
        let temp_path = std::env::temp_dir().join(format!("yantra-reality-test-{}", TaskId::new()));
        std::fs::create_dir_all(&temp_path).unwrap();
        let context = ProjectContext {
            project_root: ProjectRoot::new(temp_path.clone()).unwrap(),
        };
        (context, temp_path)
    }

    #[test]
    fn missing_file_produces_error_violation() {
        let source_truth = make_truth_with_files("primary_files", "src/nonexistent.rs");
        let (context, _temp_path) = temp_context();
        let violations = CodebaseRealityValidator.validate(&source_truth, &context);
        assert!(
            violations
                .iter()
                .any(|violation| violation.validator == "CodebaseReality"
                    && violation.severity == ViolationSeverity::Error
                    && violation.message.contains("nonexistent")),
            "expected Error for missing file"
        );
    }

    #[test]
    fn existing_unsupported_file_produces_no_violation() {
        let (context, temp_path) = temp_context();
        let existing_file = temp_path.join("data.csv");
        std::fs::write(&existing_file, "a,b,c\n1,2,3\n").unwrap();

        let source_truth = make_truth_with_files("primary_files", "data.csv");
        let violations = CodebaseRealityValidator.validate(&source_truth, &context);
        assert!(
            !violations
                .iter()
                .any(|violation| violation.message.contains("data.csv")),
            "unsupported-language file that exists should not produce violations"
        );
    }

    #[test]
    fn empty_answer_produces_no_violations() {
        let source_truth = make_truth_with_files("primary_files", "  ");
        let (context, _temp_path) = temp_context();
        let violations = CodebaseRealityValidator.validate(&source_truth, &context);
        assert!(violations.is_empty());
    }

    #[test]
    fn multiple_files_checked_independently() {
        let (context, temp_path) = temp_context();
        let existing_file = temp_path.join("exists.txt");
        std::fs::write(&existing_file, "hello").unwrap();

        let source_truth = make_truth_with_files("primary_files", "exists.txt, missing_file.rs");
        let violations = CodebaseRealityValidator.validate(&source_truth, &context);
        assert!(
            violations
                .iter()
                .any(|violation| violation.message.contains("missing_file.rs")),
            "missing_file.rs should be flagged"
        );
        assert!(
            !violations
                .iter()
                .any(|violation| violation.message.contains("exists.txt")),
            "exists.txt should not be flagged"
        );
    }
}
