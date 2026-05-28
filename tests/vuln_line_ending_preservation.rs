//! # Line Ending Preservation Tests
//!
//! Verifies that unified diff application preserves the dominant line endings (CRLF)
//! of the modified file rather than converting them to LF.
//!
//! ## Input
//! - A temporary file written with CRLF line endings
//! - A unified diff applying a change
//!
//! ## Output
//! - A successfully written file using CRLF line endings
//!
//! ## Related
//! - `yantra_tools::apply_diff_to_file` — the unified diff engine

use yantra_tools::apply_diff_to_file;

#[test]
fn test_crlf_line_ending_preservation() {
    let temporary_dir = tempfile::tempdir().unwrap();
    let file_path = temporary_dir.path().join("crlf_file.rs");

    let original_content = "pub fn test() -> i32 {\r\n    42\r\n}\r\n";
    std::fs::write(&file_path, original_content).unwrap();

    let diff_text = "@@ -1,3 +1,4 @@\n pub fn test() -> i32 {\n-    42\n+    let result = 42;\n+    result\n }";

    let result = apply_diff_to_file(&file_path, diff_text);
    assert!(result.is_ok(), "Diff application failed: {:?}", result.err());

    let modified_content = std::fs::read_to_string(&file_path).unwrap();
    assert!(
        modified_content.contains("\r\n"),
        "Modified file must preserve CRLF line endings; got: {:?}",
        modified_content
    );
    assert!(
        !modified_content.contains("\n\n"),
        "Lines should not have double newlines; got: {:?}",
        modified_content
    );
}
