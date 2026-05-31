//! # forge-ast: Symbol Extraction Accuracy Tests
//!
//! Validates that `parse_file` + `extract_symbols` correctly identifies named
//! symbols from hand-labeled Rust fixture snippets. Each fixture is a 2-5 line
//! Rust function, struct, trait, or enum with a known canonical symbol name.
//! The golden set covers 10 fixtures; the accuracy test asserts ≥90% precision
//! (at least 9 of 10 expected names extracted).
//!
//! ## Input
//! - 10 hand-labeled Rust source snippets written to a temporary directory
//! - Fixture → expected symbol name mapping defined inline
//!
//! ## Output
//! - Test assertion: matched_count / total_expected ≥ 0.90
//!
//! ## Related
//! - `forge-ast::parser`    — `parse_file` under test
//! - `forge-ast::extractor` — `extract_symbols` under test

use std::fs;
use std::path::{Path, PathBuf};

use yantra_ast::{extract_symbols, parse_file};

struct AccuracyFixture {
    source_code: &'static str,
    expected_symbol_name: &'static str,
}

fn golden_fixtures() -> Vec<AccuracyFixture> {
    vec![
        AccuracyFixture {
            source_code: "/// Computes the checksum of a byte slice.\npub fn compute_checksum(payload: &[u8]) -> u64 {\n    payload.iter().fold(0u64, |accumulator, byte| accumulator ^ u64::from(*byte))\n}\n",
            expected_symbol_name: "compute_checksum",
        },
        AccuracyFixture {
            source_code: "/// Holds metadata for a repository entry.\npub struct RepositoryEntry {\n    pub entry_id: u64,\n    pub entry_name: String,\n}\n",
            expected_symbol_name: "RepositoryEntry",
        },
        AccuracyFixture {
            source_code: "/// Defines verification behavior for trust tokens.\npub trait TokenVerifier {\n    fn verify_token(&self, raw_token: &str) -> bool;\n}\n",
            expected_symbol_name: "TokenVerifier",
        },
        AccuracyFixture {
            source_code: "/// Error variants for routing failures.\npub enum RoutingError {\n    /// No provider available for the requested tier.\n    NoProviderAvailable,\n    /// The selected model is offline.\n    ModelOffline,\n}\n",
            expected_symbol_name: "RoutingError",
        },
        AccuracyFixture {
            source_code: "/// Maximum allowed token budget per task.\npub const MAX_TOKEN_BUDGET: usize = 8192;\n",
            expected_symbol_name: "MAX_TOKEN_BUDGET",
        },
        AccuracyFixture {
            source_code: "/// Formats a greeting for the given recipient name.\npub fn format_greeting(recipient_name: &str) -> String {\n    format!(\"Hello, {recipient_name}!\")\n}\n",
            expected_symbol_name: "format_greeting",
        },
        AccuracyFixture {
            source_code: "/// Wraps a raw JSON value as a typed agent response.\npub struct AgentResponse {\n    pub response_id: String,\n    pub payload: serde_json::Value,\n    pub is_final: bool,\n}\n",
            expected_symbol_name: "AgentResponse",
        },
        AccuracyFixture {
            source_code: "/// Validates that a path component is free of traversal sequences.\npub fn validate_path_component(component: &str) -> bool {\n    !component.contains(\"..\")\n        && !component.contains('/')\n        && !component.contains('\\\\')\n}\n",
            expected_symbol_name: "validate_path_component",
        },
        AccuracyFixture {
            source_code: "/// Shared application configuration loaded from `routing.toml`.\npub static DEFAULT_ROUTING_TIMEOUT_MS: u64 = 5_000;\n",
            expected_symbol_name: "DEFAULT_ROUTING_TIMEOUT_MS",
        },
        AccuracyFixture {
            source_code: "/// Counts the number of non-whitespace tokens in a string slice.\npub fn count_tokens_in_slice(text: &str) -> usize {\n    text.split_whitespace().count()\n}\n",
            expected_symbol_name: "count_tokens_in_slice",
        },
    ]
}

fn write_fixture_files<'fixture_lifetime>(
    base_directory: &Path,
    fixtures: &'fixture_lifetime [AccuracyFixture],
) -> Vec<(PathBuf, &'fixture_lifetime str)> {
    fs::create_dir_all(base_directory).expect("creating accuracy test temp directory must succeed");

    fixtures
        .iter()
        .enumerate()
        .map(|(fixture_index, fixture)| {
            let file_path = base_directory.join(format!("fixture_{fixture_index}.rs"));
            fs::write(&file_path, fixture.source_code)
                .expect("writing accuracy fixture file must succeed");
            (file_path, fixture.expected_symbol_name)
        })
        .collect()
}

/// Returns three Rust snippets whose primary named items are inside `impl` blocks.
///
/// The extractor currently skips `function_item` nodes inside `impl_item` (see
/// `extractor.rs:203`). These fixtures document that design decision: the test
/// below asserts 0% recall for impl methods, not because the extractor is broken
/// but because it deliberately excludes them to avoid noise.  If this ever
/// changes, `impl_method_extraction_documents_known_gap` will fail as a reminder
/// to update both the test and this doc comment.
fn impl_method_fixtures() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "pub struct TokenCache { inner: Vec<String> }\nimpl TokenCache {\n    pub fn insert_token(&mut self, raw_token: String) { self.inner.push(raw_token); }\n}\n",
            "insert_token",
        ),
        (
            "pub trait Processor { fn process(&self, input: &str) -> String; }\npub struct EchoProcessor;\nimpl Processor for EchoProcessor {\n    fn process(&self, input: &str) -> String { input.to_owned() }\n}\n",
            "process",
        ),
        (
            "pub struct Counter { value: u64 }\nimpl Counter {\n    pub fn increment_by(&mut self, delta: u64) { self.value += delta; }\n}\n",
            "increment_by",
        ),
    ]
}

/// Documents the known extractor gap: `function_item` nodes inside `impl_item`
/// are explicitly skipped by the extractor (`extractor.rs:203`).
///
/// The impl STRUCT itself is captured as `SymbolKind::Impl`, but the individual
/// methods inside are not. This test asserts the expected 0% method-level recall
/// and will fail loudly if the extractor is updated to capture impl methods —
/// at which point the threshold in `ast_symbol_extraction_achieves_target_accuracy`
/// should also be raised to reflect the improved recall.
#[test]
fn impl_method_extraction_documents_known_gap() {
    let fixtures = impl_method_fixtures();
    let temp_directory = std::env::temp_dir().join(format!(
        "yantra-ast-impl-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&temp_directory)
        .expect("creating impl test temp directory must succeed");

    let mut found_as_function_count: usize = 0;

    for (index, (source_code, method_name)) in fixtures.iter().enumerate() {
        let file_path = temp_directory.join(format!("impl_fixture_{index}.rs"));
        std::fs::write(&file_path, source_code).expect("writing impl fixture file must succeed");

        let parsed_file =
            yantra_ast::parse_file(&file_path).expect("parse_file must succeed on impl fixture");
        let extracted_symbols = yantra_ast::extract_symbols(&parsed_file)
            .expect("extract_symbols must succeed on impl fixture");

        let extracted_names: Vec<&str> = extracted_symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect();

        if extracted_names.contains(method_name) {
            found_as_function_count += 1;
        }
    }

    let _ = std::fs::remove_dir_all(&temp_directory);

    assert_eq!(
        found_as_function_count,
        0,
        "Expected impl methods to NOT be extracted as Function symbols \
         (extractor.rs:203 skips function_item inside impl_item). \
         If this changes, update both this test and the accuracy threshold above. \
         Found {} / {} methods unexpectedly extracted.",
        found_as_function_count,
        fixtures.len(),
    );
}

/// Runs the golden fixture set and asserts that symbol extraction achieves ≥90% recall.
///
/// Each of the 10 hand-labeled Rust snippets is parsed and extracted. The test
/// counts how many of the expected symbol names appear in the extracted output
/// and asserts the ratio meets the 0.90 floor.
///
/// **Coverage**: top-level `pub fn`, `pub struct`, `pub trait`, `pub enum`,
/// `pub const`, `pub static`. Does NOT cover impl methods (see
/// `impl_method_extraction_documents_known_gap` for that gap).
#[test]
fn ast_symbol_extraction_achieves_target_accuracy() {
    let temp_directory = std::env::temp_dir().join(format!(
        "yantra-ast-accuracy-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or(0)
    ));

    let fixtures = golden_fixtures();
    let fixture_paths = write_fixture_files(&temp_directory, &fixtures);

    let total_expected_count = fixture_paths.len();
    let mut matched_count: usize = 0;

    for (fixture_path, expected_symbol_name) in &fixture_paths {
        let parsed_file = parse_file(fixture_path).unwrap_or_else(|parse_error| {
            panic!("parse_file failed for {fixture_path:?}: {parse_error}")
        });
        let extracted_symbols = extract_symbols(&parsed_file).unwrap_or_else(|extract_error| {
            panic!("extract_symbols failed for {fixture_path:?}: {extract_error}")
        });

        let symbol_names: Vec<&str> = extracted_symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect();

        if symbol_names.contains(expected_symbol_name) {
            matched_count += 1;
        } else {
            eprintln!(
                "accuracy miss: expected symbol '{expected_symbol_name}' not found in {fixture_path:?}; extracted: {symbol_names:?}"
            );
        }
    }

    let _ = fs::remove_dir_all(&temp_directory);

    assert!(
        matched_count * 10 >= total_expected_count * 9,
        "AST symbol extraction accuracy target (≥90%) not met: {matched_count}/{total_expected_count} symbols matched",
    );
}
