//! # Canvas Clone Vulnerability Tests
//!
//! Exercises the `DomTree::from_html` and `css_props_to_tailwind` functions
//! against adversarial and degenerate HTML/CSS inputs to verify that no
//! amount of malformed or deeply nested input can cause a panic or a
//! `YantraId` collision.
//!
//! ## Input
//! - Adversarially crafted HTML strings (deep nesting, truncated, empty)
//! - Invalid or unknown CSS property declarations
//!
//! ## Output
//! - Assertions that parsing always completes without panic and returns valid
//!   results, and that IDs remain collision-free across structurally distinct
//!   DOM paths.
//!
//! ## Related
//! - `forge-canvas::dom` — `DomTree::from_html` under test
//! - `forge-canvas::css_to_tailwind` — `css_props_to_tailwind` under test

use scraper::Html;
use yantra_canvas::{css_props_to_tailwind, CssDeclaration, DomTree, YantraId};

/// Builds an HTML string with `depth` levels of nested `<div>` elements.
fn build_deeply_nested_html(depth: usize) -> String {
    let mut html_content = String::with_capacity(depth * 11 + 20);
    html_content.push_str("<html><body>");
    for _ in 0..depth {
        html_content.push_str("<div>");
    }
    html_content.push_str("leaf");
    for _ in 0..depth {
        html_content.push_str("</div>");
    }
    html_content.push_str("</body></html>");
    html_content
}

#[test]
fn test_deeply_nested_html_does_not_overflow_stack() {
    let deeply_nested_html = build_deeply_nested_html(500);
    let parsed_document = Html::parse_document(&deeply_nested_html);
    let dom_tree_result = DomTree::from_html(&parsed_document);
    assert!(
        dom_tree_result.is_ok(),
        "DomTree::from_html must succeed on 500-level nesting without stack overflow"
    );
    let dom_tree = dom_tree_result.unwrap();
    assert!(
        !dom_tree.nodes.is_empty(),
        "parsed tree must contain at least one node"
    );
}

#[test]
fn test_malformed_html_does_not_panic() {
    let truncated_html = "<div><p>unclosed";
    let parsed_document = Html::parse_document(truncated_html);
    let dom_tree_result = DomTree::from_html(&parsed_document);
    assert!(
        dom_tree_result.is_ok(),
        "DomTree::from_html must not panic on truncated/malformed HTML"
    );
}

#[test]
fn test_yantra_id_injection_in_html_does_not_collide() {
    let html_with_siblings = r#"
        <html><body>
            <div class="first">sibling one</div>
            <div class="second">sibling two</div>
        </body></html>
    "#;
    let parsed_document = Html::parse_document(html_with_siblings);
    let dom_tree = DomTree::from_html(&parsed_document).unwrap();

    let body_node = dom_tree.root();
    assert!(
        body_node.children.len() >= 2,
        "body must have at least two child nodes"
    );

    let first_child_id = &body_node.children[0];
    let second_child_id = &body_node.children[1];
    assert_ne!(
        first_child_id, second_child_id,
        "sibling elements at distinct child indices must produce distinct YantraIds"
    );
}

#[test]
fn test_empty_html_body_returns_valid_dom_tree() {
    let empty_body_html = "<html><body></body></html>";
    let parsed_document = Html::parse_document(empty_body_html);
    let dom_tree_result = DomTree::from_html(&parsed_document);
    assert!(
        dom_tree_result.is_ok(),
        "DomTree::from_html must succeed on an empty <body>"
    );
    let dom_tree = dom_tree_result.unwrap();
    assert_eq!(
        dom_tree.root().tag,
        "body",
        "root element of an empty-body document must be <body>"
    );
}

#[test]
fn test_css_to_tailwind_unknown_properties_become_leftover() {
    let unknown_css_declarations = vec![
        CssDeclaration::new("box-shadow", "2px 2px 4px rgba(0,0,0,0.5)"),
        CssDeclaration::new("cursor", "pointer"),
        CssDeclaration::new("overflow", "hidden"),
        CssDeclaration::new("transition", "all 0.3s ease"),
        CssDeclaration::new("z-index", "100"),
        CssDeclaration::new("transform", "rotate(45deg)"),
        CssDeclaration::new("animation", "spin 1s linear infinite"),
        CssDeclaration::new("grid-template-columns", "repeat(3, 1fr)"),
        CssDeclaration::new("list-style", "none"),
        CssDeclaration::new("outline", "2px solid red"),
    ];

    let translation = css_props_to_tailwind(&unknown_css_declarations);

    assert!(
        translation.tailwind_classes.is_empty(),
        "unknown/arbitrary CSS properties must produce no Tailwind classes; got: {:?}",
        translation.tailwind_classes
    );
    assert_eq!(
        translation.leftover_inline.len(),
        10,
        "all 10 unknown declarations must appear in leftover_inline"
    );
}

#[test]
fn test_css_to_tailwind_empty_input_returns_empty() {
    let empty_declarations: &[CssDeclaration] = &[];
    let translation = css_props_to_tailwind(empty_declarations);
    assert!(
        translation.tailwind_classes.is_empty(),
        "empty input must produce no Tailwind classes"
    );
    assert!(
        translation.leftover_inline.is_empty(),
        "empty input must produce no leftover declarations"
    );
}

#[test]
fn test_yantra_id_from_path_is_exactly_twelve_chars() {
    let yantra_id = YantraId::from_path("html>body>section(0)>article(1)");
    assert_eq!(
        yantra_id.as_str().len(),
        12,
        "YantraId must always be exactly 12 hex characters"
    );
}

#[test]
fn test_yantra_id_from_path_differs_across_paths() {
    let paths_to_check = [
        "html>body>div(0)",
        "html>body>div(1)",
        "html>body>span(0)",
        "html>head>title(0)",
    ];
    let generated_ids: Vec<YantraId> = paths_to_check
        .iter()
        .map(|path| YantraId::from_path(path))
        .collect();

    for outer_index in 0..generated_ids.len() {
        for inner_index in (outer_index + 1)..generated_ids.len() {
            assert_ne!(
                generated_ids[outer_index], generated_ids[inner_index],
                "distinct DOM paths must produce distinct YantraIds: {:?} vs {:?}",
                paths_to_check[outer_index], paths_to_check[inner_index]
            );
        }
    }
}
