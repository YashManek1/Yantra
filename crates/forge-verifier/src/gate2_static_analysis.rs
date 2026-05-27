//! # forge-verifier: Gate 2 — Static Analysis
//!
//! Detects the languages touched by a diff, then spawns the appropriate
//! static-analysis tools (cargo clippy + check for Rust; mypy + ruff for
//! Python; tsc + eslint for TypeScript). Parses each tool's output into
//! structured `Violation`s and returns a pass/fail verdict.
//!
//! ## Input
//! - `Diff` — raw unified diff text used only for language detection
//! - `VerificationContext` — project root path for running tools
//!
//! ## Output
//! - `ValidationResult::Pass` — all tools exited cleanly
//! - `ValidationResult::Fail(violations)` — one or more errors found
//!
//! ## Related
//! - `forge-verifier::subprocess` — used to spawn each tool
//! - `forge-verifier::pipeline` — calls this as gate 2

use std::sync::LazyLock;

use regex::Regex;

use crate::error::VerifierResult;
use crate::subprocess::spawn_command;
use crate::types::{Diff, ValidationResult, VerificationContext, Violation, ViolationSeverity};

static PATTERN_RUST_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)\+\+\+ b/.*\.rs$").expect("rust file pattern is valid"));

static PATTERN_PYTHON_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)\+\+\+ b/.*\.py$").expect("python file pattern is valid"));

static PATTERN_TS_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)\+\+\+ b/.*\.tsx?$").expect("typescript file pattern is valid")
});

static PATTERN_MYPY_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?):(\d+): (error|warning|note): (.+?)(?:\s+\[(.+?)\])?$")
        .expect("mypy line pattern is valid")
});

static PATTERN_TSC_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.+)$")
        .expect("tsc line pattern is valid")
});

/// Languages detected in the diff that require static analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisLanguage {
    Rust,
    Python,
    TypeScript,
}

/// Gate 2: static analysis across all languages touched by the diff.
pub struct StaticAnalysisGate;

impl StaticAnalysisGate {
    /// Runs static analysis for every language detected in `diff` and
    /// returns a consolidated pass/fail verdict.
    pub async fn check(diff: &Diff, ctx: &VerificationContext) -> VerifierResult<ValidationResult> {
        let detected_languages = detect_analysis_languages(&diff.text);
        let mut all_violations: Vec<Violation> = Vec::new();

        for language in &detected_languages {
            let language_violations = match language {
                AnalysisLanguage::Rust => check_rust(ctx).await?,
                AnalysisLanguage::Python => check_python(ctx).await?,
                AnalysisLanguage::TypeScript => check_typescript(ctx).await?,
            };
            all_violations.extend(language_violations);
        }

        let error_violations: Vec<Violation> = all_violations
            .into_iter()
            .filter(|violation| violation.severity == ViolationSeverity::Error)
            .collect();

        if error_violations.is_empty() {
            Ok(ValidationResult::Pass)
        } else {
            Ok(ValidationResult::Fail(error_violations))
        }
    }
}

/// Returns all languages found in `+++ b/…` headers of `diff_text`.
pub fn detect_analysis_languages(diff_text: &str) -> Vec<AnalysisLanguage> {
    let mut languages = Vec::new();
    if PATTERN_RUST_FILE.is_match(diff_text) {
        languages.push(AnalysisLanguage::Rust);
    }
    if PATTERN_PYTHON_FILE.is_match(diff_text) {
        languages.push(AnalysisLanguage::Python);
    }
    if PATTERN_TS_FILE.is_match(diff_text) {
        languages.push(AnalysisLanguage::TypeScript);
    }
    languages
}

async fn check_rust(ctx: &VerificationContext) -> VerifierResult<Vec<Violation>> {
    let clippy_output = spawn_command(
        "cargo",
        &["clippy", "--message-format=json", "--", "-D", "warnings"],
        &ctx.project_root,
    )
    .await?;

    let mut violations = parse_cargo_json_output(&clippy_output.stdout);

    if violations.is_empty() && !clippy_output.exit_success {
        let check_output = spawn_command(
            "cargo",
            &["check", "--message-format=json"],
            &ctx.project_root,
        )
        .await?;
        violations.extend(parse_cargo_json_output(&check_output.stdout));
    }

    Ok(violations)
}

async fn check_python(ctx: &VerificationContext) -> VerifierResult<Vec<Violation>> {
    let mypy_output = spawn_command(
        "mypy",
        &[".", "--ignore-missing-imports"],
        &ctx.project_root,
    )
    .await?;

    let mut violations = parse_mypy_output(&mypy_output.stdout);

    let ruff_output = spawn_command(
        "ruff",
        &["check", "--output-format=json", "."],
        &ctx.project_root,
    )
    .await?;
    violations.extend(parse_ruff_json_output(&ruff_output.stdout));

    Ok(violations)
}

async fn check_typescript(ctx: &VerificationContext) -> VerifierResult<Vec<Violation>> {
    let tsc_output = spawn_command("tsc", &["--noEmit"], &ctx.project_root).await?;
    let mut violations = parse_tsc_output(&tsc_output.stdout);

    let eslint_output = spawn_command(
        "eslint",
        &["--format=json", "--ext", ".ts,.tsx", "."],
        &ctx.project_root,
    )
    .await?;
    violations.extend(parse_eslint_json_output(&eslint_output.stdout));

    Ok(violations)
}

/// Parses `--message-format=json` lines produced by `cargo clippy` or `cargo check`.
pub fn parse_cargo_json_output(json_lines: &str) -> Vec<Violation> {
    let mut violations = Vec::new();

    for raw_line in json_lines.lines() {
        let parsed_message: serde_json::Value = match serde_json::from_str(raw_line) {
            Ok(parsed_value) => parsed_value,
            Err(_) => continue,
        };

        if parsed_message
            .get("reason")
            .and_then(|reason| reason.as_str())
            != Some("compiler-message")
        {
            continue;
        }

        let message_object = match parsed_message.get("message") {
            Some(message_value) => message_value,
            None => continue,
        };

        let level = message_object
            .get("level")
            .and_then(|level| level.as_str())
            .unwrap_or("note");

        let severity = if level == "error" {
            ViolationSeverity::Error
        } else {
            ViolationSeverity::Warning
        };

        let rendered_message = message_object
            .get("rendered")
            .and_then(|rendered| rendered.as_str())
            .unwrap_or("(no message)")
            .to_owned();

        let rule_id = message_object
            .get("code")
            .and_then(|code| code.get("code"))
            .and_then(|code_string| code_string.as_str())
            .map(|code_string| format!("clippy::{code_string}"));

        let primary_span = message_object
            .get("spans")
            .and_then(|spans| spans.as_array())
            .and_then(|span_array| {
                span_array.iter().find(|span| {
                    span.get("is_primary").and_then(serde_json::Value::as_bool) == Some(true)
                })
            });

        let file_path = primary_span
            .and_then(|span| span.get("file_name"))
            .and_then(|file_name| file_name.as_str())
            .map(str::to_owned);

        let line = primary_span
            .and_then(|span| span.get("line_start"))
            .and_then(serde_json::Value::as_u64)
            .map(|line_number| line_number as u32);

        let column = primary_span
            .and_then(|span| span.get("column_start"))
            .and_then(serde_json::Value::as_u64)
            .map(|column_number| column_number as u32);

        violations.push(Violation {
            file_path,
            line,
            column,
            message: rendered_message,
            severity,
            rule_id,
        });
    }

    violations
}

/// Parses the text output produced by `mypy`.
///
/// Expected format: `path:line: severity: message  [rule-id]`
pub fn parse_mypy_output(output: &str) -> Vec<Violation> {
    let mut violations = Vec::new();

    for raw_line in output.lines() {
        let Some(captures) = PATTERN_MYPY_LINE.captures(raw_line) else {
            continue;
        };

        let severity_text = captures
            .get(3)
            .map_or("note", |match_group| match_group.as_str());
        if severity_text == "note" {
            continue;
        }

        let severity = if severity_text == "error" {
            ViolationSeverity::Error
        } else {
            ViolationSeverity::Warning
        };

        let file_path = captures
            .get(1)
            .map(|match_group| match_group.as_str().to_owned());
        let line = captures
            .get(2)
            .and_then(|match_group| match_group.as_str().parse::<u32>().ok());
        let message = captures
            .get(4)
            .map_or("(no message)", |match_group| match_group.as_str())
            .to_owned();
        let rule_id = captures
            .get(5)
            .map(|match_group| format!("mypy::{}", match_group.as_str()));

        violations.push(Violation {
            file_path,
            line,
            column: None,
            message,
            severity,
            rule_id,
        });
    }

    violations
}

/// Parses the JSON array produced by `ruff check --output-format=json`.
pub fn parse_ruff_json_output(json_text: &str) -> Vec<Violation> {
    let parsed_array: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(parsed_value) => parsed_value,
        Err(_) => return Vec::new(),
    };

    let Some(ruff_entries) = parsed_array.as_array() else {
        return Vec::new();
    };

    ruff_entries
        .iter()
        .filter_map(|ruff_entry| {
            let filename = ruff_entry
                .get("filename")
                .and_then(|file_name| file_name.as_str())
                .map(str::to_owned);

            let message = ruff_entry
                .get("message")
                .and_then(|message_value| message_value.as_str())?
                .to_owned();

            let rule_id = ruff_entry
                .get("code")
                .and_then(|code| code.as_str())
                .map(|code_string| format!("ruff::{code_string}"));

            let line = ruff_entry
                .get("location")
                .and_then(|location| location.get("row"))
                .and_then(serde_json::Value::as_u64)
                .map(|row_number| row_number as u32);

            let column = ruff_entry
                .get("location")
                .and_then(|location| location.get("column"))
                .and_then(serde_json::Value::as_u64)
                .map(|column_number| column_number as u32);

            Some(Violation {
                file_path: filename,
                line,
                column,
                message,
                severity: ViolationSeverity::Error,
                rule_id,
            })
        })
        .collect()
}

/// Parses the stdout produced by `tsc --noEmit`.
///
/// Expected format: `path(line,col): error|warning TSxxxx: message`
pub fn parse_tsc_output(output: &str) -> Vec<Violation> {
    let mut violations = Vec::new();

    for raw_line in output.lines() {
        let Some(captures) = PATTERN_TSC_LINE.captures(raw_line) else {
            continue;
        };

        let severity_text = captures.get(4).map_or("warning", |m| m.as_str());
        let severity = if severity_text == "error" {
            ViolationSeverity::Error
        } else {
            ViolationSeverity::Warning
        };

        let file_path = captures.get(1).map(|m| m.as_str().to_owned());
        let line = captures.get(2).and_then(|m| m.as_str().parse::<u32>().ok());
        let column = captures.get(3).and_then(|m| m.as_str().parse::<u32>().ok());
        let rule_id = captures.get(5).map(|m| format!("tsc::{}", m.as_str()));
        let message = captures
            .get(6)
            .map_or("(no message)", |m| m.as_str())
            .to_owned();

        violations.push(Violation {
            file_path,
            line,
            column,
            message,
            severity,
            rule_id,
        });
    }

    violations
}

/// Parses the JSON array produced by `eslint --format=json`.
pub fn parse_eslint_json_output(json_text: &str) -> Vec<Violation> {
    let parsed_array: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(parsed_value) => parsed_value,
        Err(_) => return Vec::new(),
    };

    let Some(eslint_files) = parsed_array.as_array() else {
        return Vec::new();
    };

    let mut violations = Vec::new();

    for eslint_file in eslint_files {
        let file_path = eslint_file
            .get("filePath")
            .and_then(|fp| fp.as_str())
            .map(str::to_owned);

        let Some(file_messages) = eslint_file.get("messages").and_then(|m| m.as_array()) else {
            continue;
        };

        for eslint_message in file_messages {
            let message = eslint_message
                .get("message")
                .and_then(|msg| msg.as_str())
                .unwrap_or("(no message)")
                .to_owned();

            let rule_id = eslint_message
                .get("ruleId")
                .and_then(|rule| rule.as_str())
                .map(|rule_string| format!("eslint::{rule_string}"));

            let eslint_severity = eslint_message
                .get("severity")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1);

            let severity = if eslint_severity >= 2 {
                ViolationSeverity::Error
            } else {
                ViolationSeverity::Warning
            };

            let line = eslint_message
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .map(|line_number| line_number as u32);

            let column = eslint_message
                .get("column")
                .and_then(serde_json::Value::as_u64)
                .map(|col_number| col_number as u32);

            violations.push(Violation {
                file_path: file_path.clone(),
                line,
                column,
                message,
                severity,
                rule_id,
            });
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_languages_rust_only() {
        let diff_text = "+++ b/src/lib.rs\n+fn foo() {}";
        let detected_languages = detect_analysis_languages(diff_text);
        assert_eq!(detected_languages, vec![AnalysisLanguage::Rust]);
    }

    #[test]
    fn detect_languages_python_and_typescript() {
        let diff_text = "+++ b/app.py\n+++ b/index.tsx\n+hello";
        let detected_languages = detect_analysis_languages(diff_text);
        assert!(detected_languages.contains(&AnalysisLanguage::Python));
        assert!(detected_languages.contains(&AnalysisLanguage::TypeScript));
    }

    #[test]
    fn detect_languages_returns_empty_for_unknown() {
        let diff_text = "+++ b/README.md\n+hello";
        let detected_languages = detect_analysis_languages(diff_text);
        assert!(detected_languages.is_empty());
    }

    #[test]
    fn cargo_json_parser_extracts_error_violation() {
        let json_line = r#"{"reason":"compiler-message","message":{"level":"error","rendered":"error[E0308]: mismatched types","code":{"code":"E0308","explanation":null},"spans":[{"file_name":"src/lib.rs","line_start":10,"column_start":5,"is_primary":true}]}}"#;
        let violations = parse_cargo_json_output(json_line);
        assert_eq!(violations.len(), 1);
        let first_violation = &violations[0];
        assert_eq!(first_violation.severity, ViolationSeverity::Error);
        assert_eq!(first_violation.file_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(first_violation.line, Some(10));
    }

    #[test]
    fn cargo_json_parser_skips_non_compiler_messages() {
        let json_line = r#"{"reason":"build-script-executed","package_id":"foo 0.1.0"}"#;
        let violations = parse_cargo_json_output(json_line);
        assert!(violations.is_empty());
    }

    #[test]
    fn mypy_parser_extracts_error_violation() {
        let mypy_output = "src/main.py:42: error: Argument has incompatible type  [arg-type]";
        let violations = parse_mypy_output(mypy_output);
        assert_eq!(violations.len(), 1);
        let first_violation = &violations[0];
        assert_eq!(first_violation.severity, ViolationSeverity::Error);
        assert_eq!(first_violation.line, Some(42));
        assert_eq!(first_violation.rule_id.as_deref(), Some("mypy::arg-type"));
    }

    #[test]
    fn mypy_parser_skips_notes() {
        let mypy_output = "src/main.py:1: note: Revealed type is 'int'";
        let violations = parse_mypy_output(mypy_output);
        assert!(violations.is_empty());
    }

    #[test]
    fn ruff_json_parser_extracts_violation() {
        let ruff_json = r#"[{"filename":"src/foo.py","message":"undefined name 'bar'","code":"F821","location":{"row":5,"column":3}}]"#;
        let violations = parse_ruff_json_output(ruff_json);
        assert_eq!(violations.len(), 1);
        let first_violation = &violations[0];
        assert_eq!(first_violation.line, Some(5));
        assert_eq!(first_violation.rule_id.as_deref(), Some("ruff::F821"));
    }

    #[test]
    fn tsc_parser_extracts_error() {
        let tsc_output = "src/index.ts(10,5): error TS2345: Argument of type 'string' is not assignable to parameter of type 'number'.";
        let violations = parse_tsc_output(tsc_output);
        assert_eq!(violations.len(), 1);
        let first_violation = &violations[0];
        assert_eq!(first_violation.severity, ViolationSeverity::Error);
        assert_eq!(first_violation.line, Some(10));
        assert_eq!(first_violation.rule_id.as_deref(), Some("tsc::TS2345"));
    }

    #[test]
    fn eslint_json_parser_extracts_violation() {
        let eslint_json = r#"[{"filePath":"/app/src/index.ts","messages":[{"message":"'foo' is defined but never used.","ruleId":"no-unused-vars","severity":2,"line":3,"column":1}]}]"#;
        let violations = parse_eslint_json_output(eslint_json);
        assert_eq!(violations.len(), 1);
        let first_violation = &violations[0];
        assert_eq!(first_violation.severity, ViolationSeverity::Error);
        assert_eq!(
            first_violation.rule_id.as_deref(),
            Some("eslint::no-unused-vars")
        );
    }
}
