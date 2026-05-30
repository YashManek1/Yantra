//! # forge-canvas::editor: Project Registry + Edit Application
//!
//! Holds the in-memory `ProjectHandle` for every project loaded into the
//! canvas server, and applies inspector edits by mutating the `DomTree`
//! and writing the affected TSX component via `tsx_writer::rewrite_component`.
//!
//! ## Input
//! - `ProjectRegistry` — `Arc<RwLock<HashMap<String, ProjectHandle>>>`
//! - `PropertyUpdate` deserialized from `POST /api/dom/:project/update`
//!
//! ## Output
//! - On-disk TSX file rewritten
//! - `FileChange` returned for the WebSocket broadcaster
//!
//! ## Related
//! - `forge-canvas::dom` — `DomTree` mutated in place
//! - `forge-canvas::tsx_writer` — performs the surgical TSX edit
//! - `forge-canvas::server` — REST handler that calls `apply_update`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::css_to_tailwind::CssDeclaration;
use crate::dom::{DomTree, YantraId};
use crate::error::{CanvasError, CanvasResult};
use crate::server::AppState;
use crate::tsx_writer::{rewrite_component, ProjectLayout};

/// Per-project registry held in `AppState`.
pub type ProjectRegistry = Arc<RwLock<HashMap<String, ProjectHandle>>>;

/// A reversible snapshot of a single DOM node's mutable properties, captured
/// immediately before an `apply_update` mutates the node. Pushing this onto
/// `undo_stack` makes the edit undoable.
#[derive(Debug, Clone)]
pub struct DomPatch {
    /// Stable identifier of the DOM element this patch restores.
    pub yantra_id: String,
    /// Class list as it was before the edit.
    pub restore_classes: Vec<String>,
    /// Inline style map as it was before the edit.
    pub restore_style: HashMap<String, String>,
}

/// All state for one loaded project (DOM, TSX layout, root directory).
#[derive(Debug, Clone)]
pub struct ProjectHandle {
    /// Slug used as the URL path component.
    pub slug: String,
    /// Project root on disk (`./yantra-canvas/<slug>/`).
    pub root: PathBuf,
    /// In-memory DOM tree (the source of truth for inspector selects).
    pub dom: DomTree,
    /// Disk layout from `emit_project`.
    pub layout: ProjectLayout,
    /// Patches that can be applied to undo recent edits (most-recent last).
    pub undo_stack: Vec<DomPatch>,
    /// Patches that can be applied to redo undone edits (most-recent last).
    pub redo_stack: Vec<DomPatch>,
}

/// Information returned by `select(yantra_id)` for the inspector panel.
#[derive(Debug, Clone)]
pub struct SelectionInfo {
    /// Path of the component file containing this element.
    pub component_file: PathBuf,
    /// HTML tag name.
    pub tag: String,
    /// Current Tailwind classes on the element.
    pub classes: Vec<String>,
    /// Current inline styles (declarations the user has set explicitly).
    pub styles: HashMap<String, String>,
}

/// Request body for `POST /api/dom/:project/update`.
#[derive(Debug, Clone, Deserialize)]
pub struct PropertyUpdate {
    /// Stable identifier of the DOM element to mutate.
    pub yantra_id: String,
    /// Tailwind classes to add to the element.
    #[serde(default)]
    pub add_classes: Vec<String>,
    /// Tailwind classes to remove from the element.
    #[serde(default)]
    pub remove_classes: Vec<String>,
    /// CSS property → value pairs to set (any property; auto-converted to
    /// Tailwind by the writer where possible).
    #[serde(default)]
    pub set_style: HashMap<String, String>,
}

/// File-change event broadcast over the watch channel after every update.
#[derive(Debug, Clone, Serialize)]
pub struct FileChange {
    /// Path of the file that was just written.
    pub path: PathBuf,
    /// Project slug owning the file.
    pub project: String,
    /// Yantra ID of the element that changed.
    pub yantra_id: String,
}

impl FileChange {
    /// Initial empty value held in the `watch` channel before any edits land.
    pub fn idle() -> Self {
        Self {
            path: PathBuf::new(),
            project: String::new(),
            yantra_id: String::new(),
        }
    }
}

impl ProjectHandle {
    /// Returns a serializable summary of the DOM tree for the editor.
    pub fn dom_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "slug": self.slug,
            "root_id": self.dom.root_id.as_str(),
            "node_count": self.dom.nodes.len(),
        })
    }

    /// Looks up an element by `YantraId` string and returns inspector info.
    pub fn select(&self, yantra_id_str: &str) -> Option<SelectionInfo> {
        let yantra_id = YantraId::from_string(yantra_id_str);
        let dom_node = self.dom.find(&yantra_id)?;
        let component_file = self
            .layout
            .component_file_by_yantra_id
            .get(&yantra_id)?
            .clone();
        Some(SelectionInfo {
            component_file,
            tag: dom_node.tag.clone(),
            classes: dom_node.classes.clone(),
            styles: dom_node.inline_style.clone(),
        })
    }
}

/// Creates an empty project registry.
pub fn new_registry() -> ProjectRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Applies an inspector `PropertyUpdate`: mutates the DOM tree, recomputes
/// effective classes + inline styles, rewrites the TSX file.
///
/// # Errors
///
/// - `UnknownProject` if `project_slug` is not registered.
/// - `UnknownYantraId` if `update.yantra_id` is not present in the DOM tree.
/// - `TsxWriteFailed` on any disk write failure.
pub async fn apply_update(
    state: &AppState,
    project_slug: &str,
    update: &PropertyUpdate,
) -> CanvasResult<FileChange> {
    let mut registry_guard = state.projects.write().await;
    let handle = registry_guard
        .get_mut(project_slug)
        .ok_or_else(|| CanvasError::UnknownProject(project_slug.to_owned()))?;

    let yantra_id = YantraId::from_string(&update.yantra_id);
    let component_file_path = handle
        .layout
        .component_file_by_yantra_id
        .get(&yantra_id)
        .ok_or_else(|| CanvasError::UnknownYantraId {
            project: project_slug.to_owned(),
            yantra_id: update.yantra_id.clone(),
        })?
        .clone();

    let (combined_classes, leftover_inline, pre_edit_patch) = {
        let dom_node =
            handle
                .dom
                .find_mut(&yantra_id)
                .ok_or_else(|| CanvasError::UnknownYantraId {
                    project: project_slug.to_owned(),
                    yantra_id: update.yantra_id.clone(),
                })?;

        let pre_edit_patch = DomPatch {
            yantra_id: update.yantra_id.clone(),
            restore_classes: dom_node.classes.clone(),
            restore_style: dom_node.inline_style.clone(),
        };

        for class_name in &update.remove_classes {
            dom_node.classes.retain(|existing| existing != class_name);
        }
        for class_name in &update.add_classes {
            if !dom_node.classes.contains(class_name) {
                dom_node.classes.push(class_name.clone());
            }
        }
        for (property, value) in &update.set_style {
            dom_node
                .inline_style
                .insert(property.to_lowercase(), value.clone());
        }

        let inline_declarations: Vec<CssDeclaration> = dom_node
            .inline_style
            .iter()
            .map(|(property, value)| CssDeclaration::new(property, value))
            .collect();
        let translation = crate::css_to_tailwind::css_props_to_tailwind(&inline_declarations);

        let mut node_classes = dom_node.classes.clone();
        node_classes.extend(translation.tailwind_classes);
        node_classes.sort();
        node_classes.dedup();

        (node_classes, translation.leftover_inline, pre_edit_patch)
    };

    handle.undo_stack.push(pre_edit_patch);
    handle.redo_stack.clear();

    rewrite_component(
        &component_file_path,
        &yantra_id,
        &combined_classes,
        &leftover_inline,
    )?;

    Ok(FileChange {
        path: component_file_path,
        project: project_slug.to_owned(),
        yantra_id: update.yantra_id.clone(),
    })
}

/// Pops the most-recent patch from the undo stack, restores the DOM node to
/// its previous state, rewrites the TSX file, and pushes an inverse patch
/// onto the redo stack so the action can be re-applied.
///
/// # Errors
///
/// - `UnknownProject` if `project_slug` is not registered.
/// - `EmptyUndoStack` if there are no edits to undo.
/// - `UnknownYantraId` if the yantra_id from the patch is no longer in the DOM.
/// - `TsxWriteFailed` on any disk write failure.
pub async fn undo_update(state: &AppState, project_slug: &str) -> CanvasResult<FileChange> {
    let mut registry_guard = state.projects.write().await;
    let handle = registry_guard
        .get_mut(project_slug)
        .ok_or_else(|| CanvasError::UnknownProject(project_slug.to_owned()))?;

    let undo_patch = handle
        .undo_stack
        .pop()
        .ok_or_else(|| CanvasError::EmptyUndoStack(project_slug.to_owned()))?;

    let (file_change, inverse_patch) = restore_dom_node(handle, project_slug, &undo_patch)?;

    handle.redo_stack.push(inverse_patch);
    Ok(file_change)
}

/// Pops the most-recent entry from the redo stack, re-applies it to the DOM
/// node, rewrites the TSX file, and pushes an inverse patch back onto the
/// undo stack.
///
/// # Errors
///
/// - `UnknownProject` if `project_slug` is not registered.
/// - `EmptyRedoStack` if there are no undone edits to redo.
/// - `UnknownYantraId` if the yantra_id from the patch is no longer in the DOM.
/// - `TsxWriteFailed` on any disk write failure.
pub async fn redo_update(state: &AppState, project_slug: &str) -> CanvasResult<FileChange> {
    let mut registry_guard = state.projects.write().await;
    let handle = registry_guard
        .get_mut(project_slug)
        .ok_or_else(|| CanvasError::UnknownProject(project_slug.to_owned()))?;

    let redo_patch = handle
        .redo_stack
        .pop()
        .ok_or_else(|| CanvasError::EmptyRedoStack(project_slug.to_owned()))?;

    let (file_change, inverse_patch) = restore_dom_node(handle, project_slug, &redo_patch)?;

    handle.undo_stack.push(inverse_patch);
    Ok(file_change)
}

/// Restores a DOM node to the state described by `patch`, capturing the
/// pre-restore state as an inverse `DomPatch`. Rewrites the TSX component
/// file to match the restored state.
///
/// Returns `(FileChange, inverse_patch)` on success. The caller decides
/// which stack receives the inverse patch.
fn restore_dom_node(
    handle: &mut ProjectHandle,
    project_slug: &str,
    patch: &DomPatch,
) -> CanvasResult<(FileChange, DomPatch)> {
    let yantra_id = YantraId::from_string(&patch.yantra_id);

    let component_file_path = handle
        .layout
        .component_file_by_yantra_id
        .get(&yantra_id)
        .ok_or_else(|| CanvasError::UnknownYantraId {
            project: project_slug.to_owned(),
            yantra_id: patch.yantra_id.clone(),
        })?
        .clone();

    let (combined_classes, leftover_inline, inverse_patch) = {
        let dom_node =
            handle
                .dom
                .find_mut(&yantra_id)
                .ok_or_else(|| CanvasError::UnknownYantraId {
                    project: project_slug.to_owned(),
                    yantra_id: patch.yantra_id.clone(),
                })?;

        let inverse_patch = DomPatch {
            yantra_id: patch.yantra_id.clone(),
            restore_classes: dom_node.classes.clone(),
            restore_style: dom_node.inline_style.clone(),
        };

        dom_node.classes.clone_from(&patch.restore_classes);
        dom_node.inline_style.clone_from(&patch.restore_style);

        let inline_declarations: Vec<CssDeclaration> = dom_node
            .inline_style
            .iter()
            .map(|(property, value)| CssDeclaration::new(property, value))
            .collect();
        let translation = crate::css_to_tailwind::css_props_to_tailwind(&inline_declarations);

        let mut node_classes = dom_node.classes.clone();
        node_classes.extend(translation.tailwind_classes);
        node_classes.sort();
        node_classes.dedup();

        (node_classes, translation.leftover_inline, inverse_patch)
    };

    rewrite_component(
        &component_file_path,
        &yantra_id,
        &combined_classes,
        &leftover_inline,
    )?;

    let file_change = FileChange {
        path: component_file_path,
        project: project_slug.to_owned(),
        yantra_id: patch.yantra_id.clone(),
    };
    Ok((file_change, inverse_patch))
}
