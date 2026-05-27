//! # forge-stvp: Spec-as-Tests Compiler
//!
//! Translates each `success_criterion` in a validated `SourceTruth` into a
//! skeleton test file that the user reviews before the task begins. For Rust
//! projects the skeleton uses `proptest`; for Python projects it uses
//! `hypothesis`. The generated files are tagged with a
//! `// generated-from-stvp: <task_id>` header so they can be found, reviewed,
//! and eventually filled in with real assertions.
//!
//! ## Input
//! - `SourceTruth` with one or more newline-separated `success_criterion` answers
//! - `Language` — target test framework (Rust or Python)
//! - `output_dir` — directory where test files are written
//!
//! ## Output
//! - One `GeneratedTest` per non-blank criterion line, written to `output_dir`
//!
//! ## Related
//! - `forge-stvp::source_truth` — provides the `SourceTruth` artifact
//! - `forge-stvp::questionnaire` — the `success_criterion` question key is
//!   defined in the per-class questionnaire templates

use std::path::{Path, PathBuf};

use yantra_core::TaskId;

use crate::error::StvpError;
use crate::source_truth::SourceTruth;

/// Source language that determines which test framework the skeleton targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// Generates `proptest::proptest!` skeletons (`.rs` files).
    Rust,
    /// Generates `hypothesis`-based `pytest` skeletons (`.py` files).
    Python,
}

/// A single generated test file produced by `SpecCompiler::compile`.
#[derive(Debug, Clone)]
pub struct GeneratedTest {
    /// Zero-based index of the criterion that produced this test.
    pub criterion_idx: usize,
    /// Absolute path of the written test file.
    pub file_path: PathBuf,
    /// Full text content of the generated file.
    pub content: String,
}

/// Compiles `SourceTruth` success criteria into skeleton test files.
pub struct SpecCompiler;

impl SpecCompiler {
    /// Generates one test skeleton per non-blank criterion line in
    /// `truth.answers["success_criterion"]`.
    ///
    /// Files are written to `output_dir` and the directory is created if it
    /// does not exist. Returns an empty `Vec` when no criterion is present or
    /// when `success_criterion` is blank.
    ///
    /// # Errors
    ///
    /// Returns `StvpError::DirectoryCreate` or `StvpError::FileWrite` on
    /// filesystem failures.
    pub fn compile(
        truth: &SourceTruth,
        language: Language,
        output_dir: &Path,
    ) -> Result<Vec<GeneratedTest>, StvpError> {
        let criterion_raw = truth
            .answers
            .get("success_criterion")
            .map_or("", String::as_str);

        let criteria: Vec<&str> = criterion_raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();

        if criteria.is_empty() {
            return Ok(Vec::new());
        }

        std::fs::create_dir_all(output_dir).map_err(|source| StvpError::DirectoryCreate {
            path: output_dir.display().to_string(),
            source,
        })?;

        let mut generated_tests: Vec<GeneratedTest> = Vec::new();

        for (criterion_idx, criterion) in criteria.iter().enumerate() {
            let (file_name, content) = match language {
                Language::Rust => generate_rust_skeleton(truth.task_id, criterion_idx, criterion),
                Language::Python => {
                    generate_python_skeleton(truth.task_id, criterion_idx, criterion)
                }
            };

            let file_path = output_dir.join(&file_name);
            std::fs::write(&file_path, &content).map_err(|source| StvpError::FileWrite {
                path: file_path.display().to_string(),
                source,
            })?;

            generated_tests.push(GeneratedTest {
                criterion_idx,
                file_path,
                content,
            });
        }

        Ok(generated_tests)
    }
}

fn safe_id(task_id: TaskId, idx: usize) -> String {
    let sanitized: String = task_id
        .to_string()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!("{sanitized}_{idx}")
}

fn generate_rust_skeleton(task_id: TaskId, idx: usize, criterion: &str) -> (String, String) {
    let module_id = safe_id(task_id, idx);
    let file_name = format!("spec_{module_id}.rs");
    let content = format!(
        "// generated-from-stvp: {task_id}\n\
         // criterion[{idx}]: {criterion}\n\
         \n\
         #[cfg(test)]\n\
         mod spec_{module_id} {{\n\
         \n\
             proptest::proptest! {{\n\
                 /// Verifies: {criterion}\n\
                 #[test]\n\
                 fn spec_criterion_{idx}(_input in proptest::arbitrary::any::<u64>()) {{\n\
                     // TODO: implement test for criterion:\n\
                     // {criterion}\n\
                     proptest::prop_assert!(true);\n\
                 }}\n\
             }}\n\
         }}\n"
    );
    (file_name, content)
}

fn generate_python_skeleton(task_id: TaskId, idx: usize, criterion: &str) -> (String, String) {
    let module_id = safe_id(task_id, idx);
    let file_name = format!("test_spec_{module_id}.py");
    let content = format!(
        "# generated-from-stvp: {task_id}\n\
         # criterion[{idx}]: {criterion}\n\
         \n\
         import pytest\n\
         from hypothesis import given, strategies as st\n\
         \n\
         \n\
         def test_spec_criterion_{idx}():\n\
             \"\"\"\n\
             Verifies: {criterion}\n\
             \"\"\"\n\
             # TODO: implement test for criterion:\n\
             # {criterion}\n\
             pass\n"
    );
    (file_name, content)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use yantra_core::{Strictness, TaskClass, TaskId};

    use super::*;

    fn temp_output_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("yantra-spec-{label}-{}", TaskId::new()))
    }

    fn truth_with_criterion(criterion: &str) -> SourceTruth {
        let mut answers = BTreeMap::new();
        answers.insert("success_criterion".to_owned(), criterion.to_owned());
        SourceTruth {
            task_id: TaskId::new(),
            task_class: TaskClass::NewFeature,
            strictness: Strictness::Strict,
            description: "add JWT rotation".to_owned(),
            created_at: Utc::now(),
            answers,
            augmented_question_ids: Vec::new(),
        }
    }

    #[test]
    fn compile_rust_generates_one_file_per_criterion_line() {
        let truth = truth_with_criterion(
            "returns HTTP 200 when token is valid\nrejects expired tokens with 401",
        );
        let output_dir = temp_output_dir("rust");

        let generated = SpecCompiler::compile(&truth, Language::Rust, &output_dir)
            .expect("compilation succeeds");

        assert_eq!(generated.len(), 2, "one file per non-blank criterion line");
        assert!(generated[0]
            .file_path
            .extension()
            .is_some_and(|ext| ext == "rs"));
        assert!(generated[0].content.contains("generated-from-stvp"));
        assert!(generated[0].content.contains("proptest"));
        assert!(generated[0].content.contains("returns HTTP 200"));
        assert!(generated[1].content.contains("rejects expired tokens"));
    }

    #[test]
    fn compile_python_generates_hypothesis_skeleton() {
        let truth = truth_with_criterion("rotation endpoint returns a new access token");
        let output_dir = temp_output_dir("py");

        let generated = SpecCompiler::compile(&truth, Language::Python, &output_dir)
            .expect("compilation succeeds");

        assert_eq!(generated.len(), 1);
        assert!(generated[0]
            .file_path
            .extension()
            .is_some_and(|ext| ext == "py"));
        assert!(generated[0].content.contains("generated-from-stvp"));
        assert!(generated[0].content.contains("hypothesis"));
        assert!(generated[0].content.contains("rotation endpoint"));
    }

    #[test]
    fn compile_empty_criterion_produces_no_files() {
        let truth = truth_with_criterion("");
        let output_dir = temp_output_dir("empty");

        let generated = SpecCompiler::compile(&truth, Language::Rust, &output_dir)
            .expect("compilation succeeds on empty criterion");
        assert!(generated.is_empty());
    }

    #[test]
    fn compile_blank_lines_are_filtered_out() {
        let truth = truth_with_criterion("\n  \nreturns 200\n\n");
        let output_dir = temp_output_dir("blank");

        let generated = SpecCompiler::compile(&truth, Language::Rust, &output_dir)
            .expect("compilation succeeds");
        assert_eq!(
            generated.len(),
            1,
            "only the non-blank criterion line should produce a file"
        );
    }

    #[test]
    fn generated_files_exist_on_disk_after_compile() {
        let truth = truth_with_criterion("token is rotated and old token is invalidated");
        let output_dir = temp_output_dir("disk");

        let generated = SpecCompiler::compile(&truth, Language::Rust, &output_dir)
            .expect("compilation succeeds");

        for test_file in &generated {
            assert!(
                test_file.file_path.exists(),
                "generated file must exist on disk: {}",
                test_file.file_path.display()
            );
        }
    }

    #[test]
    fn generated_rust_file_contains_task_id_tag() {
        let truth = truth_with_criterion("returns the rotated token in the response body");
        let output_dir = temp_output_dir("tag");
        let task_id = truth.task_id;

        let generated = SpecCompiler::compile(&truth, Language::Rust, &output_dir)
            .expect("compilation succeeds");
        assert!(
            generated[0].content.contains(&task_id.to_string()),
            "file must be tagged with the task ID"
        );
    }
}
