//! # forge-canvas::dom: DOM Tree Representation
//!
//! In-memory DOM tree built from a parsed `scraper::Html`. Each `DomNode`
//! carries a deterministic `YantraId` (SHA-256 of its DOM path) so the
//! browser editor and the TSX writer can refer to elements by a stable
//! identifier rather than fragile CSS selectors.
//!
//! ## Input
//! - `scraper::Html` — pre-parsed HTML document
//!
//! ## Output
//! - `DomTree` — root + flat `HashMap<YantraId, DomNode>` for O(1) lookup
//! - `YantraId` — 12-char hex prefix of SHA-256 of element path
//!
//! ## Related
//! - `forge-canvas::clone` — parses raw HTML into `scraper::Html`
//! - `forge-canvas::tsx_writer` — emits JSX elements keyed by `YantraId`
//! - `forge-canvas::editor` — looks up nodes by `YantraId` on edit requests

use std::collections::HashMap;

use ring::digest;
use scraper::{ElementRef, Html, Node};
use serde::{Deserialize, Serialize};

use crate::error::{CanvasError, CanvasResult};

/// Deterministic stable identifier for a DOM element.
///
/// Computed as the first 12 hex characters of the SHA-256 of the element's
/// path from the document root, joined by `>` (e.g. `html>body>div(0)>h1(1)`).
/// Stable across re-parses as long as DOM structure doesn't change.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct YantraId(String);

impl YantraId {
    /// Constructs a `YantraId` from an already-hashed string.
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Computes a `YantraId` from a path string of `tag(child_index)` segments
    /// joined by `>`.
    pub fn from_path(path: &str) -> Self {
        use std::fmt::Write;
        let hash = digest::digest(&digest::SHA256, path.as_bytes());
        let hex = hash.as_ref().iter().take(6).fold(
            String::with_capacity(12),
            |mut accumulated_hex, byte| {
                let _ = write!(accumulated_hex, "{byte:02x}");
                accumulated_hex
            },
        );
        Self(hex)
    }

    /// Returns the underlying hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for YantraId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A single DOM element with its tag, classes, inline styles, and children.
///
/// Text nodes are flattened into the `text_content` field of the parent
/// element — the editor doesn't manipulate raw text nodes today.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomNode {
    /// Deterministic stable identifier for this element.
    pub yantra_id: YantraId,
    /// HTML tag name (`div`, `h1`, `section`, etc.) in lower case.
    pub tag: String,
    /// CSS classes parsed from the `class` attribute.
    pub classes: Vec<String>,
    /// Other HTML attributes excluding `class` and `style`.
    pub attributes: HashMap<String, String>,
    /// Inline CSS declarations parsed from the `style` attribute.
    pub inline_style: HashMap<String, String>,
    /// Direct text content (if this element has any text children).
    pub text_content: String,
    /// Child element `YantraId`s in document order.
    pub children: Vec<YantraId>,
}

impl DomNode {
    fn empty(yantra_id: YantraId, tag: String) -> Self {
        Self {
            yantra_id,
            tag,
            classes: Vec::new(),
            attributes: HashMap::new(),
            inline_style: HashMap::new(),
            text_content: String::new(),
            children: Vec::new(),
        }
    }
}

/// Full DOM tree indexed by `YantraId` for O(1) lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomTree {
    /// Identifier of the root element (typically the `<body>` node).
    pub root_id: YantraId,
    /// Flat lookup table from `YantraId` to `DomNode`.
    pub nodes: HashMap<YantraId, DomNode>,
}

impl DomTree {
    /// Builds a `DomTree` from a parsed `scraper::Html` document.
    ///
    /// Walks the document starting at `<body>` (or `<html>` if no body) and
    /// assigns each element a `YantraId` derived from its path.
    ///
    /// # Errors
    ///
    /// Returns `CanvasError::DomParseFailed` when no root element exists.
    pub fn from_html(parsed_html: &Html) -> CanvasResult<Self> {
        let body_selector = scraper::Selector::parse("body")
            .map_err(|err| CanvasError::DomParseFailed(format!("body selector: {err}")))?;
        let html_selector = scraper::Selector::parse("html")
            .map_err(|err| CanvasError::DomParseFailed(format!("html selector: {err}")))?;

        let root_element = parsed_html
            .select(&body_selector)
            .next()
            .or_else(|| parsed_html.select(&html_selector).next())
            .ok_or_else(|| CanvasError::DomParseFailed("no <body> or <html> element".to_owned()))?;

        let mut nodes: HashMap<YantraId, DomNode> = HashMap::new();
        let root_id = walk_element(root_element, "", &mut nodes);

        Ok(Self { root_id, nodes })
    }

    /// Returns the root node.
    pub fn root(&self) -> &DomNode {
        self.nodes.get(&self.root_id).expect("root id must exist")
    }

    /// Looks up a node by its `YantraId`.
    pub fn find(&self, yantra_id: &YantraId) -> Option<&DomNode> {
        self.nodes.get(yantra_id)
    }

    /// Looks up a mutable node by its `YantraId`.
    pub fn find_mut(&mut self, yantra_id: &YantraId) -> Option<&mut DomNode> {
        self.nodes.get_mut(yantra_id)
    }
}

fn walk_element(
    element: ElementRef<'_>,
    path_prefix: &str,
    nodes: &mut HashMap<YantraId, DomNode>,
) -> YantraId {
    let tag_name = element.value().name().to_owned();
    let current_path = if path_prefix.is_empty() {
        tag_name.clone()
    } else {
        format!("{path_prefix}>{tag_name}")
    };
    let yantra_id = YantraId::from_path(&current_path);
    let mut dom_node = DomNode::empty(yantra_id.clone(), tag_name);

    for (attribute_name, attribute_value) in element.value().attrs() {
        match attribute_name {
            "class" => {
                dom_node.classes = attribute_value
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
            }
            "style" => {
                dom_node.inline_style = parse_inline_style(attribute_value);
            }
            _ => {
                dom_node
                    .attributes
                    .insert(attribute_name.to_owned(), attribute_value.to_owned());
            }
        }
    }

    let mut text_buffer = String::new();
    let mut child_index_by_tag: HashMap<String, usize> = HashMap::new();
    for child in element.children() {
        match child.value() {
            Node::Element(child_element_value) => {
                if let Some(child_element_ref) = ElementRef::wrap(child) {
                    let child_tag = child_element_value.name().to_owned();
                    let child_counter = child_index_by_tag.entry(child_tag.clone()).or_insert(0);
                    let child_path = format!("{current_path}>{child_tag}({child_counter})");
                    *child_counter += 1;
                    let child_id = walk_element(child_element_ref, &child_path, nodes);
                    dom_node.children.push(child_id);
                }
            }
            Node::Text(text_value) => {
                text_buffer.push_str(text_value);
            }
            _ => {}
        }
    }
    text_buffer.trim().clone_into(&mut dom_node.text_content);

    nodes.insert(yantra_id.clone(), dom_node);
    yantra_id
}

fn parse_inline_style(style_attribute: &str) -> HashMap<String, String> {
    let mut declarations = HashMap::new();
    for declaration_segment in style_attribute.split(';') {
        let trimmed_segment = declaration_segment.trim();
        if trimmed_segment.is_empty() {
            continue;
        }
        if let Some((property, value)) = trimmed_segment.split_once(':') {
            declarations.insert(property.trim().to_lowercase(), value.trim().to_owned());
        }
    }
    declarations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yantra_id_is_deterministic() {
        let id_a = YantraId::from_path("html>body>div(0)");
        let id_b = YantraId::from_path("html>body>div(0)");
        assert_eq!(id_a, id_b);
        assert_eq!(id_a.as_str().len(), 12);
    }

    #[test]
    fn yantra_id_differs_for_different_paths() {
        let id_a = YantraId::from_path("html>body>div(0)");
        let id_b = YantraId::from_path("html>body>div(1)");
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn dom_tree_from_minimal_html() {
        let html_text = r#"<html><body><h1 class="title">Hi</h1><p>World</p></body></html>"#;
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        assert_eq!(tree.root().tag, "body");
        assert_eq!(tree.root().children.len(), 2);
        let header_id = &tree.root().children[0];
        let header_node = tree.find(header_id).unwrap();
        assert_eq!(header_node.tag, "h1");
        assert_eq!(header_node.classes, vec!["title".to_owned()]);
        assert_eq!(header_node.text_content, "Hi");
    }

    #[test]
    fn inline_style_parsing() {
        let html_text =
            r#"<html><body><div style="color: red; padding: 8px;">x</div></body></html>"#;
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        let div_id = &tree.root().children[0];
        let div_node = tree.find(div_id).unwrap();
        assert_eq!(div_node.inline_style.get("color"), Some(&"red".to_owned()));
        assert_eq!(
            div_node.inline_style.get("padding"),
            Some(&"8px".to_owned())
        );
    }
}
