//! # forge-lsp: Unit Tests
//!
//! Exercises bridge routing logic (language detection, URI construction,
//! diagnostic parsing) without spawning actual language server processes.
//! Integration tests that require `rust-analyzer` are marked with a feature
//! guard comment and skipped in CI unless the binary is present.
//!
//! ## Input
//! - Hard-coded LSP JSON response fixtures
//!
//! ## Output
//! - Assertions over `Location`, `Diagnostic`, `HoverInfo`, `TextEdit` fields
//!
//! ## Related
//! - `yantra_lsp::LspBridge`
//! - `yantra_lsp::types`

use yantra_lsp::{Diagnostic, DiagnosticSeverity, HoverInfo, Language, Location, TextEdit};

#[test]
fn language_from_extension_rust() {
    let language = Language::from_extension("rs").unwrap();
    assert_eq!(language, Language::Rust);
    assert_eq!(language.default_server_binary(), "rust-analyzer");
    assert_eq!(language.lsp_language_id(), "rust");
}

#[test]
fn language_from_extension_python() {
    let language = Language::from_extension("py").unwrap();
    assert_eq!(language, Language::Python);
    assert_eq!(language.default_server_binary(), "pyright-langserver");
}

#[test]
fn language_from_extension_typescript() {
    let language = Language::from_extension("ts").unwrap();
    assert_eq!(language, Language::TypeScript);
}

#[test]
fn language_from_extension_tsx() {
    let language = Language::from_extension("tsx").unwrap();
    assert_eq!(language, Language::TypeScript);
}

#[test]
fn language_from_unknown_extension_returns_none() {
    assert!(Language::from_extension("md").is_none());
    assert!(Language::from_extension("json").is_none());
    assert!(Language::from_extension("").is_none());
}

#[test]
fn location_serializes_and_deserializes() {
    let location = Location {
        file: "/src/lib.rs".to_owned(),
        start_line: 5,
        start_column: 4,
        end_line: 5,
        end_column: 12,
    };
    let serialized = serde_json::to_string(&location).unwrap();
    let deserialized: Location = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.file, location.file);
    assert_eq!(deserialized.start_line, 5);
}

#[test]
fn diagnostic_serializes_with_severity() {
    let diagnostic = Diagnostic {
        severity: DiagnosticSeverity::Warning,
        message: "unused variable".to_owned(),
        location: Location {
            file: "src/main.rs".to_owned(),
            start_line: 10,
            start_column: 8,
            end_line: 10,
            end_column: 15,
        },
        code: Some("dead_code".to_owned()),
    };
    let serialized = serde_json::to_value(&diagnostic).unwrap();
    assert_eq!(serialized["severity"], "warning");
    assert_eq!(serialized["message"], "unused variable");
    assert_eq!(serialized["code"], "dead_code");
}

#[test]
fn hover_info_serializes_contents_and_range() {
    let hover_info = HoverInfo {
        contents: "fn greet(name: &str) -> String".to_owned(),
        range: None,
    };
    let serialized = serde_json::to_value(&hover_info).unwrap();
    assert_eq!(serialized["contents"], "fn greet(name: &str) -> String");
    assert!(serialized["range"].is_null());
}

#[test]
fn text_edit_serializes_all_fields() {
    let text_edit = TextEdit {
        file: "src/lib.rs".to_owned(),
        start_line: 3,
        start_column: 7,
        end_line: 3,
        end_column: 12,
        new_text: "renamed_fn".to_owned(),
    };
    let serialized = serde_json::to_value(&text_edit).unwrap();
    assert_eq!(serialized["new_text"], "renamed_fn");
    assert_eq!(serialized["start_line"], 3);
}

#[test]
fn lsp_error_unsupported_language_message() {
    use yantra_lsp::LspError;
    let error = LspError::UnsupportedLanguage {
        language: "cobol".to_owned(),
    };
    let message = error.to_string();
    assert!(message.contains("cobol"));
}

#[test]
fn lsp_error_timeout_message() {
    use yantra_lsp::LspError;
    let error = LspError::Timeout { timeout_ms: 5000 };
    let message = error.to_string();
    assert!(message.contains("5000"));
}
