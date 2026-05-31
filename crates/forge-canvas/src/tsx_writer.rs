//! # forge-canvas::tsx_writer: DOM → React + Tailwind Component Emitter
//!
//! Walks a `DomTree`, decomposes it into React components, and writes a
//! complete `./yantra-canvas/<project>/` project: `pages/index.tsx`,
//! `components/*.tsx`, `tailwind.config.js`. Also provides surgical
//! single-element rewrites (`rewrite_component`) for editor-driven edits.
//!
//! ## Input
//! - `DomTree` — parsed DOM with `YantraId` on every element
//! - `Path` — output project directory
//!
//! ## Output
//! - On-disk TSX project. Each JSX element carries `data-yantra-id` so the
//!   editor can locate it during edits.
//!
//! ## Decomposition heuristic (Phase 1)
//! - Top-level `<header>`, `<nav>`, `<main>`, `<section>`, `<footer>`,
//!   `<aside>` become components (PascalName + numeric suffix on collision).
//! - Everything else inlines into `pages/index.tsx`.
//!
//! ## Related
//! - `forge-canvas::dom` — `DomTree`/`DomNode` source
//! - `forge-canvas::css_to_tailwind` — class translation
//! - `forge-canvas::editor` — calls `rewrite_component` on edit requests

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::css_to_tailwind::{css_props_to_tailwind, CssDeclaration};
use crate::dom::{DomNode, DomTree, YantraId};
use crate::error::{CanvasError, CanvasResult};

/// A single React component decomposed from the DOM tree.
#[derive(Debug, Clone)]
pub struct Component {
    /// PascalCase component name (e.g. `Header`, `MainSection2`).
    pub name: String,
    /// Root DOM node that becomes the component's top-level JSX element.
    pub root_yantra_id: YantraId,
    /// Output file path (e.g. `components/Header.tsx`).
    pub file_path: PathBuf,
}

/// Where each piece of the emitted project lives on disk.
#[derive(Debug, Clone)]
pub struct ProjectLayout {
    /// Project root directory (`./yantra-canvas/<slug>/`).
    pub project_root: PathBuf,
    /// Path to the page entry (`pages/index.tsx`).
    pub page_path: PathBuf,
    /// All extracted component files.
    pub components: Vec<Component>,
    /// `YantraId` → `PathBuf` of the component file that owns it.
    pub component_file_by_yantra_id: HashMap<YantraId, PathBuf>,
}

/// Tailwind class set + leftover inline-style declarations for an element.
#[derive(Debug, Clone, Default)]
pub struct ElementProps {
    pub class_names: Vec<String>,
    pub inline_styles: Vec<CssDeclaration>,
}

/// Emits the complete React + Tailwind project for `tree` into `project_root`.
///
/// Creates `project_root` if it doesn't exist. Writes `pages/index.tsx`,
/// `components/*.tsx`, and `tailwind.config.js`. Existing files are
/// overwritten.
///
/// # Errors
///
/// Returns `CanvasError::TsxWriteFailed` on any disk write failure.
pub fn emit_project(tree: &DomTree, project_root: &Path) -> CanvasResult<ProjectLayout> {
    fs::create_dir_all(project_root).map_err(|err| CanvasError::TsxWriteFailed {
        path: project_root.to_path_buf(),
        reason: format!("create project root: {err}"),
    })?;
    fs::create_dir_all(project_root.join("pages")).map_err(|err| CanvasError::TsxWriteFailed {
        path: project_root.join("pages"),
        reason: format!("create pages dir: {err}"),
    })?;
    fs::create_dir_all(project_root.join("components")).map_err(|err| {
        CanvasError::TsxWriteFailed {
            path: project_root.join("components"),
            reason: format!("create components dir: {err}"),
        }
    })?;

    let components = decompose_heuristic(tree);
    let mut component_file_by_yantra_id: HashMap<YantraId, PathBuf> = HashMap::new();
    let mut component_entries: Vec<Component> = Vec::new();

    for component in &components {
        let component_file_path = project_root
            .join("components")
            .join(format!("{}.tsx", component.name));
        let component_source =
            emit_component_tsx(tree, &component.root_yantra_id, &component.name)?;
        fs::write(&component_file_path, component_source).map_err(|err| {
            CanvasError::TsxWriteFailed {
                path: component_file_path.clone(),
                reason: format!("write component: {err}"),
            }
        })?;
        walk_descendants(tree, &component.root_yantra_id, &mut |descendant_id| {
            component_file_by_yantra_id.insert(descendant_id.clone(), component_file_path.clone());
        });
        component_entries.push(Component {
            name: component.name.clone(),
            root_yantra_id: component.root_yantra_id.clone(),
            file_path: component_file_path,
        });
    }

    let inline_root_ids: Vec<YantraId> = tree
        .root()
        .children
        .iter()
        .filter(|child_id| {
            !components
                .iter()
                .any(|component| &component.root_yantra_id == *child_id)
        })
        .cloned()
        .collect();

    let page_path = project_root.join("pages").join("index.tsx");
    let page_source = emit_index_page(tree, &component_entries, &inline_root_ids)?;
    fs::write(&page_path, page_source).map_err(|err| CanvasError::TsxWriteFailed {
        path: page_path.clone(),
        reason: format!("write index page: {err}"),
    })?;
    for inline_id in &inline_root_ids {
        walk_descendants(tree, inline_id, &mut |descendant_id| {
            component_file_by_yantra_id.insert(descendant_id.clone(), page_path.clone());
        });
    }
    component_file_by_yantra_id.insert(tree.root_id.clone(), page_path.clone());

    let tailwind_config_path = project_root.join("tailwind.config.js");
    fs::write(&tailwind_config_path, TAILWIND_CONFIG_JS).map_err(|err| {
        CanvasError::TsxWriteFailed {
            path: tailwind_config_path,
            reason: format!("write tailwind.config.js: {err}"),
        }
    })?;

    Ok(ProjectLayout {
        project_root: project_root.to_path_buf(),
        page_path,
        components: component_entries,
        component_file_by_yantra_id,
    })
}

/// Surgical rewrite: locates the JSX element with `data-yantra-id="<id>"`
/// in `component_file_path` and replaces its `className` and inline `style`
/// attributes.
///
/// # Errors
///
/// Returns `CanvasError::TsxWriteFailed` on file I/O failure, or
/// `CanvasError::UnknownYantraId` if the marker is not present in the file.
pub fn rewrite_component(
    component_file_path: &Path,
    yantra_id: &YantraId,
    new_class_names: &[String],
    new_inline_styles: &[CssDeclaration],
) -> CanvasResult<()> {
    let source_text =
        fs::read_to_string(component_file_path).map_err(|err| CanvasError::TsxWriteFailed {
            path: component_file_path.to_path_buf(),
            reason: format!("read for rewrite: {err}"),
        })?;

    let marker = format!("data-yantra-id=\"{yantra_id}\"");
    let marker_index = source_text
        .find(&marker)
        .ok_or_else(|| CanvasError::UnknownYantraId {
            project: component_file_path.display().to_string(),
            yantra_id: yantra_id.to_string(),
        })?;

    let tag_open_start =
        source_text[..marker_index]
            .rfind('<')
            .ok_or_else(|| CanvasError::TsxWriteFailed {
                path: component_file_path.to_path_buf(),
                reason: "could not find opening '<' before yantra marker".to_owned(),
            })?;
    let tag_open_end = source_text[marker_index..]
        .find('>')
        .map(|relative_index| marker_index + relative_index)
        .ok_or_else(|| CanvasError::TsxWriteFailed {
            path: component_file_path.to_path_buf(),
            reason: "could not find closing '>' for tag with yantra marker".to_owned(),
        })?;

    let original_tag = &source_text[tag_open_start..=tag_open_end];
    let rewritten_tag =
        rewrite_tag_attributes(original_tag, yantra_id, new_class_names, new_inline_styles);

    let mut new_source = String::with_capacity(source_text.len() + rewritten_tag.len());
    new_source.push_str(&source_text[..tag_open_start]);
    new_source.push_str(&rewritten_tag);
    new_source.push_str(&source_text[tag_open_end + 1..]);

    fs::write(component_file_path, new_source).map_err(|err| CanvasError::TsxWriteFailed {
        path: component_file_path.to_path_buf(),
        reason: format!("write rewritten tsx: {err}"),
    })?;

    Ok(())
}

fn rewrite_tag_attributes(
    original_tag: &str,
    yantra_id: &YantraId,
    new_class_names: &[String],
    new_inline_styles: &[CssDeclaration],
) -> String {
    let tag_name_end = original_tag[1..]
        .find(|character: char| character.is_whitespace() || character == '/' || character == '>')
        .map_or(original_tag.len() - 1, |relative_index| relative_index + 1);
    let tag_name = &original_tag[1..tag_name_end];

    let is_self_closing = original_tag.trim_end_matches('>').trim_end().ends_with('/');
    let class_attribute = if new_class_names.is_empty() {
        String::new()
    } else {
        format!(" className=\"{}\"", new_class_names.join(" "))
    };
    let style_attribute = format_inline_style_jsx(new_inline_styles);
    let suffix = if is_self_closing { " />" } else { ">" };

    format!("<{tag_name} data-yantra-id=\"{yantra_id}\"{class_attribute}{style_attribute}{suffix}",)
}

fn format_inline_style_jsx(inline_styles: &[CssDeclaration]) -> String {
    if inline_styles.is_empty() {
        return String::new();
    }
    let declarations: Vec<String> = inline_styles
        .iter()
        .map(|declaration| {
            let camel_case_property = css_property_to_jsx_key(&declaration.property);
            format!(
                "{camel_case_property}: \"{}\"",
                escape_jsx_string(&declaration.value)
            )
        })
        .collect();
    format!(" style={{{{ {} }}}}", declarations.join(", "))
}

fn css_property_to_jsx_key(css_property: &str) -> String {
    let mut camel_case = String::with_capacity(css_property.len());
    let mut next_upper = false;
    for character in css_property.chars() {
        if character == '-' {
            next_upper = true;
            continue;
        }
        if next_upper {
            camel_case.extend(character.to_uppercase());
            next_upper = false;
        } else {
            camel_case.push(character);
        }
    }
    camel_case
}

fn escape_jsx_string(value: &str) -> String {
    value.replace('"', "\\\"")
}

/// Returns components corresponding to top-level structural body children.
pub fn decompose_heuristic(tree: &DomTree) -> Vec<Component> {
    const COMPONENT_TAGS: &[&str] = &["header", "nav", "main", "section", "footer", "aside"];
    let mut components = Vec::new();
    let mut name_counters: HashMap<String, usize> = HashMap::new();

    for child_id in &tree.root().children {
        let child_node = match tree.find(child_id) {
            Some(node) => node,
            None => continue,
        };
        if !COMPONENT_TAGS.contains(&child_node.tag.as_str()) {
            continue;
        }
        let base_name = pascal_case_for_tag(&child_node.tag);
        let counter = name_counters.entry(base_name.clone()).or_insert(0);
        *counter += 1;
        let component_name = if *counter == 1 {
            base_name.clone()
        } else {
            format!("{base_name}{counter}")
        };
        components.push(Component {
            name: component_name,
            root_yantra_id: child_id.clone(),
            file_path: PathBuf::new(),
        });
    }

    components
}

fn pascal_case_for_tag(tag: &str) -> String {
    let mut chars = tag.chars();
    match chars.next() {
        Some(first_char) => first_char.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn emit_component_tsx(
    tree: &DomTree,
    root_yantra_id: &YantraId,
    component_name: &str,
) -> CanvasResult<String> {
    let mut output = String::new();
    output.push_str(&format!("export default function {component_name}() {{\n"));
    output.push_str("  return (\n");
    emit_node_jsx(tree, root_yantra_id, 2, &mut output)?;
    output.push('\n');
    output.push_str("  );\n");
    output.push_str("}\n");
    Ok(output)
}

fn emit_index_page(
    tree: &DomTree,
    components: &[Component],
    inline_root_ids: &[YantraId],
) -> CanvasResult<String> {
    let mut output = String::new();
    for component in components {
        output.push_str(&format!(
            "import {name} from '../components/{name}';\n",
            name = component.name,
        ));
    }
    output.push('\n');
    output.push_str("export default function IndexPage() {\n");
    output.push_str("  return (\n");
    output.push_str(&format!("    <div data-yantra-id=\"{}\">\n", tree.root_id));

    let mut child_id_to_component: HashMap<&YantraId, &Component> = HashMap::new();
    for component in components {
        child_id_to_component.insert(&component.root_yantra_id, component);
    }

    for child_id in &tree.root().children {
        if let Some(component) = child_id_to_component.get(child_id) {
            output.push_str(&format!("      <{} />\n", component.name));
        } else if inline_root_ids.contains(child_id) {
            emit_node_jsx(tree, child_id, 3, &mut output)?;
            output.push('\n');
        }
    }

    output.push_str("    </div>\n");
    output.push_str("  );\n");
    output.push_str("}\n");
    Ok(output)
}

fn emit_node_jsx(
    tree: &DomTree,
    yantra_id: &YantraId,
    indent_level: usize,
    output: &mut String,
) -> CanvasResult<()> {
    let dom_node = tree
        .find(yantra_id)
        .ok_or_else(|| CanvasError::UnknownYantraId {
            project: "<emit>".to_owned(),
            yantra_id: yantra_id.to_string(),
        })?;

    let indent = "  ".repeat(indent_level);
    let element_props = element_props_for(dom_node);
    let tag = if is_jsx_safe_tag(&dom_node.tag) {
        dom_node.tag.as_str()
    } else {
        "div"
    };

    let class_attribute = if element_props.class_names.is_empty() {
        String::new()
    } else {
        format!(" className=\"{}\"", element_props.class_names.join(" "))
    };
    let style_attribute = format_inline_style_jsx(&element_props.inline_styles);

    if is_void_html_tag(tag) {
        output.push_str(&format!(
            "{indent}<{tag} data-yantra-id=\"{yantra_id}\"{class_attribute}{style_attribute} />",
        ));
        return Ok(());
    }

    output.push_str(&format!(
        "{indent}<{tag} data-yantra-id=\"{yantra_id}\"{class_attribute}{style_attribute}>",
    ));

    let has_children = !dom_node.children.is_empty();
    let has_text = !dom_node.text_content.is_empty();

    if has_children || has_text {
        output.push('\n');
        if has_text {
            let escaped_text = escape_jsx_text(&dom_node.text_content);
            output.push_str(&format!(
                "{}{}\n",
                "  ".repeat(indent_level + 1),
                escaped_text
            ));
        }
        for child_id in &dom_node.children {
            emit_node_jsx(tree, child_id, indent_level + 1, output)?;
            output.push('\n');
        }
        output.push_str(&format!("{indent}</{tag}>"));
    } else {
        output.push_str(&format!("</{tag}>"));
    }

    Ok(())
}

fn element_props_for(dom_node: &DomNode) -> ElementProps {
    let inline_declarations: Vec<CssDeclaration> = dom_node
        .inline_style
        .iter()
        .map(|(property, value)| CssDeclaration::new(property, value))
        .collect();

    let translation = css_props_to_tailwind(&inline_declarations);

    let mut combined_classes: Vec<String> = dom_node.classes.clone();
    combined_classes.extend(translation.tailwind_classes);
    let deduped: HashSet<String> = combined_classes.iter().cloned().collect();
    let mut sorted_classes: Vec<String> = deduped.into_iter().collect();
    sorted_classes.sort();

    ElementProps {
        class_names: sorted_classes,
        inline_styles: translation.leftover_inline,
    }
}

fn is_jsx_safe_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn is_void_html_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn escape_jsx_text(text: &str) -> String {
    text.replace('{', "&#123;").replace('}', "&#125;")
}

fn walk_descendants(tree: &DomTree, start_id: &YantraId, visitor: &mut impl FnMut(&YantraId)) {
    visitor(start_id);
    if let Some(node) = tree.find(start_id) {
        for child_id in &node.children {
            walk_descendants(tree, child_id, visitor);
        }
    }
}

const TAILWIND_CONFIG_JS: &str = r#"/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./pages/**/*.{js,ts,jsx,tsx}",
    "./components/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {},
  },
  plugins: [],
};
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::Html;

    fn build_tree_from(html_text: &str) -> DomTree {
        let parsed = Html::parse_document(html_text);
        DomTree::from_html(&parsed).unwrap()
    }

    #[test]
    fn decompose_extracts_top_level_structural_tags() {
        let tree = build_tree_from(
            r"<html><body>
                <header><h1>Top</h1></header>
                <main><p>Body</p></main>
                <footer><p>Bottom</p></footer>
            </body></html>",
        );
        let components = decompose_heuristic(&tree);
        assert_eq!(components.len(), 3);
        let names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Header"));
        assert!(names.contains(&"Main"));
        assert!(names.contains(&"Footer"));
    }

    #[test]
    fn decompose_numbers_duplicate_tags() {
        let tree = build_tree_from(
            r"<html><body>
                <section><p>One</p></section>
                <section><p>Two</p></section>
            </body></html>",
        );
        let components = decompose_heuristic(&tree);
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].name, "Section");
        assert_eq!(components[1].name, "Section2");
    }

    #[test]
    fn emit_project_round_trip_writes_files() {
        let tree = build_tree_from(
            r#"<html><body>
                <header><h1 class="title">Hi</h1></header>
                <main><p style="color:red">Body</p></main>
            </body></html>"#,
        );
        let temp_dir = std::env::temp_dir().join(format!("yantra-canvas-test-{}", uuid_v4_str()));
        let layout = emit_project(&tree, &temp_dir).unwrap();
        assert!(layout.page_path.exists());
        assert_eq!(layout.components.len(), 2);
        for component in &layout.components {
            assert!(component.file_path.exists(), "{:?}", component.file_path);
        }
        let header_source = std::fs::read_to_string(&layout.components[0].file_path).unwrap();
        assert!(header_source.contains("data-yantra-id="));
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn rewrite_component_updates_classes() {
        let tree = build_tree_from(
            r"<html><body>
                <header><h1>Hi</h1></header>
            </body></html>",
        );
        let temp_dir = std::env::temp_dir().join(format!("yantra-canvas-test-{}", uuid_v4_str()));
        let layout = emit_project(&tree, &temp_dir).unwrap();
        let header_node_id = tree
            .find(&layout.components[0].root_yantra_id)
            .unwrap()
            .children[0]
            .clone();
        let header_file = layout
            .component_file_by_yantra_id
            .get(&header_node_id)
            .unwrap()
            .clone();

        rewrite_component(
            &header_file,
            &header_node_id,
            &["text-red-500".to_owned()],
            &[],
        )
        .unwrap();

        let new_source = std::fs::read_to_string(&header_file).unwrap();
        assert!(new_source.contains("text-red-500"));
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    fn uuid_v4_str() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{nanos}")
    }
}
