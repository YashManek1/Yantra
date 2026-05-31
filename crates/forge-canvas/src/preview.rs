//! # forge-canvas::preview: DOM → Self-Contained Preview HTML Renderer
//!
//! Converts a `DomTree` into a complete, self-contained `index.html` document
//! suitable for serving by the Axum `ServeDir` at `/preview/<slug>/`. The
//! document inlines all captured CSS, sets a `<base href>` pointing back at
//! the cloned origin so unfetched assets resolve correctly, injects
//! `data-yantra-id` on every element so the canvas editor can locate elements
//! by click, and rewrites manifest-tracked asset references to their local
//! `public/<filename>` relative paths.
//!
//! ## Input
//! - `&DomTree` — parsed DOM with `YantraId` assigned to every element
//! - `&ClonedSite` — original HTML + captured inline/external CSS + resolved base URL
//! - `&AssetManifest` — map of original URL → local `public/<filename>` `PathBuf`
//! - `&Path` — project root (e.g. `yantra-canvas/github_com/`) used to derive
//!   relative paths from manifest entries
//!
//! ## Output
//! - `String` — complete HTML document with inlined CSS, `<base href>`,
//!   `data-yantra-id` on every element, and local-asset rewrites
//!
//! ## Related
//! - `forge-canvas::dom` — `DomTree` / `DomNode` / `YantraId` source
//! - `forge-canvas::clone` — produces `ClonedSite`
//! - `forge-canvas::assets` — produces `AssetManifest`

use std::path::Path;

use crate::assets::AssetManifest;
use crate::clone::ClonedSite;
use crate::dom::{DomTree, YantraId};

/// HTML5 void elements that must be serialized without a closing tag.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Escapes `&`, `<`, and `>` in text content for safe HTML embedding.
fn escape_text(raw_text: &str) -> String {
    raw_text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escapes `&`, `<`, `>`, and `"` in attribute values for safe insertion
/// inside double-quoted HTML attributes.
fn escape_attr(raw_value: &str) -> String {
    raw_value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Returns `true` when `tag` is an HTML5 void element (no closing tag).
fn is_void_element(tag: &str) -> bool {
    VOID_ELEMENTS.contains(&tag)
}

/// Derives the local relative path (e.g. `public/logo.png`) from a manifest
/// entry by stripping the project root prefix. Returns `None` when stripping
/// fails (the manifest entry is outside the project root, which should not
/// happen in normal operation).
fn manifest_relative_path(local_path: &Path, project_root: &Path) -> Option<String> {
    local_path
        .strip_prefix(project_root)
        .ok()
        .map(|relative_path| relative_path.to_string_lossy().replace('\\', "/"))
}

/// Rewrites an `src` or `href` attribute value for rendering in the preview.
///
/// Returns `Some(rewritten)` when the attribute should be rewritten:
/// - Manifest-tracked URLs → local `public/<filename>` relative path.
/// - Other relative or same-origin URLs → absolute URL resolved against
///   `base_url` so they still resolve from the sub-path `/preview/<slug>/`.
///
/// Returns `None` when the attribute should be left verbatim (e.g. it is not
/// an asset attribute for this tag, or it is a `data:` / `javascript:` URI).
fn rewrite_asset_attribute(
    raw_value: &str,
    tag: &str,
    attribute_name: &str,
    base_url: &url::Url,
    manifest: &AssetManifest,
    project_root: &Path,
) -> Option<String> {
    let is_asset_attribute = matches!(
        (tag, attribute_name),
        ("img" | "source", "src") | ("link", "href")
    );
    if !is_asset_attribute {
        return None;
    }
    if raw_value.starts_with("data:") || raw_value.starts_with("javascript:") {
        return None;
    }
    let resolved_url = base_url.join(raw_value).ok()?;
    let resolved_string = resolved_url.to_string();
    if let Some(local_path) = manifest.get(&resolved_string) {
        manifest_relative_path(local_path, project_root)
    } else {
        Some(resolved_string)
    }
}

/// Serializes the `DomNode` identified by `yantra_id` and all its descendants
/// into `output`, injecting `data-yantra-id` on each element and rewriting
/// asset references via the manifest.
fn serialize_node(
    yantra_id: &YantraId,
    tree: &DomTree,
    manifest: &AssetManifest,
    project_root: &Path,
    base_url: &url::Url,
    output: &mut String,
) {
    let Some(dom_node) = tree.find(yantra_id) else {
        return;
    };

    output.push('<');
    output.push_str(&dom_node.tag);

    output.push_str(" data-yantra-id=\"");
    output.push_str(yantra_id.as_str());
    output.push('"');

    if !dom_node.classes.is_empty() {
        output.push_str(" class=\"");
        output.push_str(&escape_attr(&dom_node.classes.join(" ")));
        output.push('"');
    }

    if !dom_node.inline_style.is_empty() {
        let style_declaration: String = dom_node
            .inline_style
            .iter()
            .map(|(property, value)| format!("{property}: {value}"))
            .collect::<Vec<_>>()
            .join("; ");
        output.push_str(" style=\"");
        output.push_str(&escape_attr(&style_declaration));
        output.push('"');
    }

    for (attribute_name, attribute_value) in &dom_node.attributes {
        let rewritten_value = rewrite_asset_attribute(
            attribute_value,
            &dom_node.tag,
            attribute_name,
            base_url,
            manifest,
            project_root,
        );
        let effective_value = rewritten_value.as_deref().unwrap_or(attribute_value);
        output.push(' ');
        output.push_str(attribute_name);
        output.push_str("=\"");
        output.push_str(&escape_attr(effective_value));
        output.push('"');
    }

    if is_void_element(&dom_node.tag) {
        output.push_str(" />");
        return;
    }

    output.push('>');

    if !dom_node.text_content.is_empty() {
        output.push_str(&escape_text(&dom_node.text_content));
    }

    for child_yantra_id in &dom_node.children {
        serialize_node(
            child_yantra_id,
            tree,
            manifest,
            project_root,
            base_url,
            output,
        );
    }

    output.push_str("</");
    output.push_str(&dom_node.tag);
    output.push('>');
}

/// Renders the `DomTree` as a self-contained preview `index.html` string.
///
/// The output HTML:
/// - Has `<!DOCTYPE html><html><head>` with `<meta charset>`, `<base href>`
///   pointing at the cloned origin, and all captured CSS inlined in a
///   `<style>` block.
/// - Has every element annotated with `data-yantra-id` so the canvas editor
///   can identify elements on click.
/// - Has `src`/`href` attributes rewritten: manifest-tracked assets use their
///   local `public/<filename>` path; everything else becomes absolute against
///   the origin so it still loads from the preview sub-path.
pub fn render_preview_html(
    tree: &DomTree,
    cloned: &ClonedSite,
    manifest: &AssetManifest,
    project_root: &Path,
) -> String {
    let mut html = String::with_capacity(65_536);

    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n");
    html.push_str("<meta charset=\"UTF-8\" />\n");
    html.push_str("<base href=\"");
    html.push_str(&escape_attr(cloned.base_url.as_str()));
    html.push_str("\" />\n");

    let has_css = !cloned.inline_styles.is_empty() || !cloned.external_styles.is_empty();
    if has_css {
        html.push_str("<style>\n");
        for style_block in &cloned.inline_styles {
            html.push_str(style_block);
            html.push('\n');
        }
        for style_block in &cloned.external_styles {
            html.push_str(style_block);
            html.push('\n');
        }
        html.push_str("</style>\n");
    }

    html.push_str("</head>\n");

    serialize_node(
        &tree.root_id,
        tree,
        manifest,
        project_root,
        &cloned.base_url,
        &mut html,
    );

    html.push_str("\n</html>\n");
    html
}

#[cfg(test)]
mod tests {
    
    use std::path::PathBuf;

    use scraper::Html;
    use url::Url;

    use super::*;
    use crate::clone::ClonedSite;
    use crate::dom::DomTree;

    fn minimal_cloned_site(base_url: &str) -> ClonedSite {
        ClonedSite {
            html: String::new(),
            base_url: Url::parse(base_url).unwrap(),
            inline_styles: Vec::new(),
            external_styles: Vec::new(),
        }
    }

    #[test]
    fn serializer_injects_data_yantra_id_on_every_element() {
        let html_text = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        let cloned = minimal_cloned_site("https://example.com/");
        let manifest = AssetManifest::new();
        let project_root = PathBuf::from("yantra-canvas/example_com");

        let output = render_preview_html(&tree, &cloned, &manifest, &project_root);

        assert!(
            output.contains("data-yantra-id="),
            "output must contain data-yantra-id attributes"
        );
    }

    #[test]
    fn void_element_has_no_closing_tag() {
        let html_text = r#"<html><body><img src="logo.png" alt="logo" /></body></html>"#;
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        let cloned = minimal_cloned_site("https://example.com/");
        let manifest = AssetManifest::new();
        let project_root = PathBuf::from("yantra-canvas/example_com");

        let output = render_preview_html(&tree, &cloned, &manifest, &project_root);

        assert!(
            !output.contains("</img>"),
            "void img element must not have a closing tag"
        );
        assert!(output.contains("/>"));
    }

    #[test]
    fn captured_css_is_inlined_in_head() {
        let html_text = "<html><body><p>text</p></body></html>";
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        let mut cloned = minimal_cloned_site("https://example.com/");
        cloned.inline_styles = vec!["body { color: red; }".to_owned()];
        cloned.external_styles = vec!["h1 { font-size: 2em; }".to_owned()];
        let manifest = AssetManifest::new();
        let project_root = PathBuf::from("yantra-canvas/example_com");

        let output = render_preview_html(&tree, &cloned, &manifest, &project_root);

        assert!(output.contains("<style>"));
        assert!(output.contains("body { color: red; }"));
        assert!(output.contains("h1 { font-size: 2em; }"));
    }

    #[test]
    fn manifest_entry_rewrites_img_src_to_local_path() {
        let html_text =
            r#"<html><body><img src="https://example.com/images/logo.png" /></body></html>"#;
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        let cloned = minimal_cloned_site("https://example.com/");
        let project_root = PathBuf::from("yantra-canvas/example_com");
        let mut manifest = AssetManifest::new();
        manifest.insert(
            "https://example.com/images/logo.png".to_owned(),
            project_root.join("public").join("logo.png"),
        );

        let output = render_preview_html(&tree, &cloned, &manifest, &project_root);

        assert!(
            output.contains("public/logo.png"),
            "manifest-tracked asset must be rewritten to local path; output: {output}"
        );
    }

    #[test]
    fn relative_href_is_made_absolute() {
        let html_text = r#"<html><body><link href="/styles/main.css" /></body></html>"#;
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        let cloned = minimal_cloned_site("https://example.com/");
        let manifest = AssetManifest::new();
        let project_root = PathBuf::from("yantra-canvas/example_com");

        let output = render_preview_html(&tree, &cloned, &manifest, &project_root);

        assert!(
            output.contains("https://example.com/styles/main.css"),
            "relative href must be rewritten to absolute; output: {output}"
        );
    }

    #[test]
    fn base_href_is_emitted_in_head() {
        let html_text = "<html><body></body></html>";
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        let cloned = minimal_cloned_site("https://example.com/");
        let manifest = AssetManifest::new();
        let project_root = PathBuf::from("yantra-canvas/example_com");

        let output = render_preview_html(&tree, &cloned, &manifest, &project_root);

        assert!(output.contains("<base href=\"https://example.com/\""));
    }

    #[test]
    fn data_uris_are_left_verbatim() {
        let html_text = r#"<html><body><img src="data:image/png;base64,abc" /></body></html>"#;
        let parsed = Html::parse_document(html_text);
        let tree = DomTree::from_html(&parsed).unwrap();
        let cloned = minimal_cloned_site("https://example.com/");
        let manifest = AssetManifest::new();
        let project_root = PathBuf::from("yantra-canvas/example_com");

        let output = render_preview_html(&tree, &cloned, &manifest, &project_root);

        assert!(output.contains("data:image/png;base64,abc"));
    }
}
