//! # forge-canvas: Clone + Emit Roundtrip Integration Tests
//!
//! Verifies that the full DOM-parsing → TSX-emission pipeline produces a
//! structurally correct project from a realistic HTML fixture. These tests
//! exercise `DomTree::from_html`, `emit_project`, `YantraId` uniqueness, and
//! deterministic ID generation end-to-end.
//!
//! ## Input
//! - `tests/fixtures/sample_site.html` — multi-element HTML document with
//!   inline styles, a stylesheet link, and an inline `<style>` block
//!
//! ## Output
//! - Test assertion results verifying node counts, file existence, and ID
//!   stability
//!
//! ## Related
//! - `forge-canvas::dom` — `DomTree::from_html` and `YantraId`
//! - `forge-canvas::tsx_writer` — `emit_project`

use std::collections::HashSet;

use scraper::Html;
use yantra_canvas::dom::DomTree;
use yantra_canvas::tsx_writer::emit_project;

fn load_fixture_html() -> Html {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample_site.html");
    let html_content =
        std::fs::read_to_string(&fixture_path).expect("sample_site.html fixture must be readable");
    Html::parse_document(&html_content)
}

fn unique_temp_directory() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("yantra-canvas-roundtrip-{nanos_since_epoch}"))
}

#[test]
fn test_dom_tree_from_fixture_html_has_expected_structure() {
    let parsed_document = load_fixture_html();
    let dom_tree = DomTree::from_html(&parsed_document)
        .expect("DomTree::from_html must succeed on the fixture document");

    assert!(
        !dom_tree.nodes.is_empty(),
        "parsed fixture must produce at least one DOM node"
    );

    let root_node = dom_tree.root();
    assert_eq!(
        root_node.tag, "body",
        "root node must be the <body> element"
    );

    let all_tags: Vec<String> = dom_tree
        .nodes
        .values()
        .map(|node| node.tag.clone())
        .collect();

    assert!(
        all_tags.iter().any(|tag| tag == "header"),
        "DOM tree must contain at least one <header> element"
    );
    assert!(
        all_tags.iter().any(|tag| tag == "main"),
        "DOM tree must contain at least one <main> element"
    );
    assert!(
        all_tags.iter().any(|tag| tag == "footer"),
        "DOM tree must contain at least one <footer> element"
    );
}

#[test]
fn test_emit_project_creates_expected_files() {
    let parsed_document = load_fixture_html();
    let dom_tree = DomTree::from_html(&parsed_document)
        .expect("DomTree::from_html must succeed on the fixture document");

    let temp_directory = unique_temp_directory();
    let project_layout = emit_project(&dom_tree, &temp_directory)
        .expect("emit_project must succeed with a valid DomTree and writable directory");

    let index_page_path = project_layout.page_path.clone();
    assert!(
        index_page_path.exists(),
        "pages/index.tsx must exist after emit_project: {index_page_path:?}"
    );

    let tailwind_config_path = temp_directory.join("tailwind.config.js");
    assert!(
        tailwind_config_path.exists(),
        "tailwind.config.js must exist after emit_project"
    );

    let index_tsx_content = std::fs::read_to_string(&index_page_path)
        .expect("pages/index.tsx must be readable after emit_project");
    assert!(
        index_tsx_content.contains("data-yantra-id=\""),
        "pages/index.tsx must contain at least one data-yantra-id attribute"
    );

    std::fs::remove_dir_all(&temp_directory).ok();
}

#[test]
fn test_yantra_ids_are_unique_per_element() {
    let parsed_document = load_fixture_html();
    let dom_tree = DomTree::from_html(&parsed_document)
        .expect("DomTree::from_html must succeed on the fixture document");

    let all_yantra_ids: Vec<String> = dom_tree
        .nodes
        .values()
        .map(|node| node.yantra_id.as_str().to_owned())
        .collect();

    let unique_yantra_ids: HashSet<&String> = all_yantra_ids.iter().collect();
    assert_eq!(
        all_yantra_ids.len(),
        unique_yantra_ids.len(),
        "every DOM node must have a unique YantraId (found {} total, {} unique)",
        all_yantra_ids.len(),
        unique_yantra_ids.len()
    );
}

#[test]
fn test_yantra_id_is_deterministic() {
    let html_content = {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("sample_site.html");
        std::fs::read_to_string(&fixture_path).expect("sample_site.html fixture must be readable")
    };

    let first_parsed_document = Html::parse_document(&html_content);
    let first_dom_tree =
        DomTree::from_html(&first_parsed_document).expect("first DomTree::from_html must succeed");

    let second_parsed_document = Html::parse_document(&html_content);
    let second_dom_tree = DomTree::from_html(&second_parsed_document)
        .expect("second DomTree::from_html must succeed");

    assert_eq!(
        first_dom_tree.root_id, second_dom_tree.root_id,
        "root YantraId must be identical across two parses of the same HTML"
    );
}
