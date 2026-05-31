//! # forge-lsp: Language Detection and Type-Mapping Accuracy Tests
//!
//! Verifies that `Language::from_extension` correctly classifies source file
//! extensions with 100% accuracy across a 15-entry golden corpus, and that all
//! LSP response types (`Diagnostic`, `Location`, `HoverInfo`, `TextEdit`)
//! survive a serde JSON round-trip without field loss.
//!
//! These properties are critical for correctness: a wrong language mapping sends
//! a Rust file to pyright, and a broken round-trip corrupts diagnostic data
//! before it reaches the verifier.
//!
//! ## Input
//! - Hardcoded `(extension, expected_language)` fixture pairs
//! - Hand-crafted `Diagnostic` and `Location` values serialised to JSON and back
//!
//! ## Output
//! - Assertions that language detection is 100% correct and round-trips are lossless
//!
//! ## Related
//! - `forge-lsp::types` — `Language`, `Diagnostic`, `Location`, `HoverInfo`, `TextEdit`
//! - `forge-lsp::bridge` — uses `Language::from_extension` to select the LSP server

use yantra_lsp::{Diagnostic, DiagnosticSeverity, HoverInfo, Language, Location, TextEdit};

/// One entry in the language-detection golden corpus.
struct LanguageFixture {
    /// File extension (without the leading dot).
    extension: &'static str,
    /// Expected language, or `None` if the extension should be unrecognised.
    expected: Option<Language>,
}

fn build_language_fixtures() -> Vec<LanguageFixture> {
    vec![
        LanguageFixture {
            extension: "rs",
            expected: Some(Language::Rust),
        },
        LanguageFixture {
            extension: "RS",
            expected: Some(Language::Rust),
        },
        LanguageFixture {
            extension: "py",
            expected: Some(Language::Python),
        },
        LanguageFixture {
            extension: "PY",
            expected: Some(Language::Python),
        },
        LanguageFixture {
            extension: "ts",
            expected: Some(Language::TypeScript),
        },
        LanguageFixture {
            extension: "tsx",
            expected: Some(Language::TypeScript),
        },
        LanguageFixture {
            extension: "TSX",
            expected: Some(Language::TypeScript),
        },
        LanguageFixture {
            extension: "TS",
            expected: Some(Language::TypeScript),
        },
        LanguageFixture {
            extension: "js",
            expected: None,
        },
        LanguageFixture {
            extension: "go",
            expected: None,
        },
        LanguageFixture {
            extension: "java",
            expected: None,
        },
        LanguageFixture {
            extension: "cpp",
            expected: None,
        },
        LanguageFixture {
            extension: "c",
            expected: None,
        },
        LanguageFixture {
            extension: "",
            expected: None,
        },
        LanguageFixture {
            extension: "toml",
            expected: None,
        },
    ]
}

#[test]
fn language_detection_is_100pct_accurate() {
    let fixtures = build_language_fixtures();
    let total_fixture_count = fixtures.len();
    let mut correct_count: usize = 0;
    let mut incorrect_labels: Vec<String> = Vec::new();

    for fixture in &fixtures {
        let detected = Language::from_extension(fixture.extension);
        if detected == fixture.expected {
            correct_count += 1;
        } else {
            incorrect_labels.push(format!(
                "extension={:?}: expected {:?} got {:?}",
                fixture.extension, fixture.expected, detected,
            ));
        }
    }

    let accuracy_percentage = (correct_count * 100) / total_fixture_count;

    assert!(
        accuracy_percentage == 100,
        "Language detection accuracy {correct_count}/{total_fixture_count} ({accuracy_percentage}%) \
         is below the 100% target. Misclassified:\n{}",
        incorrect_labels
            .iter()
            .map(|label| format!("  {label}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn language_server_binary_mapping_is_100pct_accurate() {
    let binary_fixtures: &[(&str, Language)] = &[
        ("rust-analyzer", Language::Rust),
        ("pyright-langserver", Language::Python),
        ("typescript-language-server", Language::TypeScript),
    ];

    for (expected_binary, language) in binary_fixtures {
        let actual_binary = language.default_server_binary();
        assert_eq!(
            actual_binary, *expected_binary,
            "language {language:?}: expected binary {expected_binary:?} but got {actual_binary:?}",
        );
    }
}

#[test]
fn language_id_mapping_is_100pct_accurate() {
    let id_fixtures: &[(&str, Language)] = &[
        ("rust", Language::Rust),
        ("python", Language::Python),
        ("typescript", Language::TypeScript),
    ];

    for (expected_id, language) in id_fixtures {
        let actual_id = language.lsp_language_id();
        assert_eq!(
            actual_id, *expected_id,
            "language {language:?}: expected LSP id {expected_id:?} but got {actual_id:?}",
        );
    }
}

#[test]
fn diagnostic_serde_round_trip_is_lossless() {
    let original_location = Location {
        file: "/workspace/src/lib.rs".to_owned(),
        start_line: 10,
        start_column: 4,
        end_line: 10,
        end_column: 20,
    };

    let diagnostics = vec![
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            message: "E0308: mismatched types".to_owned(),
            location: original_location.clone(),
            code: Some("E0308".to_owned()),
        },
        Diagnostic {
            severity: DiagnosticSeverity::Warning,
            message: "unused variable `x`".to_owned(),
            location: Location {
                file: "/workspace/src/main.rs".to_owned(),
                start_line: 3,
                start_column: 8,
                end_line: 3,
                end_column: 9,
            },
            code: Some("unused_variables".to_owned()),
        },
        Diagnostic {
            severity: DiagnosticSeverity::Information,
            message: "consider adding a semicolon".to_owned(),
            location: original_location.clone(),
            code: None,
        },
        Diagnostic {
            severity: DiagnosticSeverity::Hint,
            message: "field can be made private".to_owned(),
            location: original_location.clone(),
            code: None,
        },
    ];

    for diagnostic in &diagnostics {
        let serialised =
            serde_json::to_string(diagnostic).expect("Diagnostic must serialise to JSON");
        let deserialised: Diagnostic =
            serde_json::from_str(&serialised).expect("Diagnostic must deserialise from JSON");

        assert_eq!(
            deserialised.severity, diagnostic.severity,
            "severity field lost across round-trip for: {:?}",
            diagnostic.message,
        );
        assert_eq!(
            deserialised.message, diagnostic.message,
            "message field corrupted across round-trip",
        );
        assert_eq!(
            deserialised.code, diagnostic.code,
            "code field lost across round-trip for: {:?}",
            diagnostic.message,
        );
        assert_eq!(
            deserialised.location.file, diagnostic.location.file,
            "location.file corrupted across round-trip",
        );
        assert_eq!(
            deserialised.location.start_line, diagnostic.location.start_line,
            "location.start_line corrupted across round-trip",
        );
    }
}

#[test]
fn hover_info_serde_round_trip_is_lossless() {
    let hover_fixtures = vec![
        HoverInfo {
            contents: "fn extract_subgraph(cache: &GraphCache) -> RenderedSubgraph".to_owned(),
            range: Some(Location {
                file: "/workspace/crates/forge-crg/src/lib.rs".to_owned(),
                start_line: 42,
                start_column: 0,
                end_line: 42,
                end_column: 60,
            }),
        },
        HoverInfo {
            contents: "A signed truth token proving STVP approval.".to_owned(),
            range: None,
        },
    ];

    for hover in &hover_fixtures {
        let serialised = serde_json::to_string(hover).expect("HoverInfo must serialise to JSON");
        let deserialised: HoverInfo =
            serde_json::from_str(&serialised).expect("HoverInfo must deserialise from JSON");

        assert_eq!(
            deserialised.contents, hover.contents,
            "HoverInfo.contents corrupted across round-trip",
        );
        assert_eq!(
            deserialised.range.is_some(),
            hover.range.is_some(),
            "HoverInfo.range presence changed across round-trip",
        );
    }
}

#[test]
fn text_edit_serde_round_trip_is_lossless() {
    let text_edit = TextEdit {
        file: "/workspace/src/auth.rs".to_owned(),
        start_line: 5,
        start_column: 0,
        end_line: 5,
        end_column: 12,
        new_text: "session_manager".to_owned(),
    };

    let serialised = serde_json::to_string(&text_edit).expect("TextEdit must serialise to JSON");
    let deserialised: TextEdit =
        serde_json::from_str(&serialised).expect("TextEdit must deserialise from JSON");

    assert_eq!(deserialised.file, text_edit.file);
    assert_eq!(deserialised.start_line, text_edit.start_line);
    assert_eq!(deserialised.start_column, text_edit.start_column);
    assert_eq!(deserialised.end_line, text_edit.end_line);
    assert_eq!(deserialised.end_column, text_edit.end_column);
    assert_eq!(deserialised.new_text, text_edit.new_text);
}
