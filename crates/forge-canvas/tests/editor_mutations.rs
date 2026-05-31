//! # forge-canvas: Editor DOM Mutation Integration Tests
//!
//! Verifies `apply_update` mutates the in-memory DOM, rewrites the on-disk
//! TSX file, and returns a `FileChange` on success; and returns typed errors
//! (`UnknownProject`, `UnknownYantraId`) when the slug or element ID is not
//! found in the registry.
//!
//! ## Input
//! - A `ProjectHandle` built from the sample_site.html fixture and emitted
//!   into a temp directory via `emit_project`
//! - `PropertyUpdate` structs with class additions and style mutations
//!
//! ## Output
//! - Test assertion results on `FileChange` fields and error variants
//!
//! ## Related
//! - `forge-canvas::editor` — `apply_update`, `ProjectHandle`, `PropertyUpdate`
//! - `forge-canvas::tsx_writer` — `emit_project`
//! - `forge-canvas::server` — `AppState`

use std::collections::HashMap;

use scraper::Html;
use yantra_canvas::dom::DomTree;
use yantra_canvas::editor::{new_registry, ProjectHandle, PropertyUpdate};
use yantra_canvas::error::CanvasError;
use yantra_canvas::server::AppState;
use yantra_canvas::tsx_writer::emit_project;

fn load_fixture_html_text() -> String {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample_site.html");
    std::fs::read_to_string(&fixture_path).expect("sample_site.html fixture must be readable")
}

fn unique_temp_directory(label: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("yantra-canvas-editor-{label}-{nanos_since_epoch}"))
}

async fn build_populated_app_state(
    project_slug: &str,
    temp_directory: &std::path::Path,
) -> (AppState, String) {
    let html_text = load_fixture_html_text();
    let parsed_document = Html::parse_document(&html_text);
    let dom_tree =
        DomTree::from_html(&parsed_document).expect("DomTree::from_html must succeed on fixture");

    let project_layout = emit_project(&dom_tree, temp_directory)
        .expect("emit_project must succeed with a valid DomTree");

    let first_yantra_id = dom_tree
        .root()
        .children
        .first()
        .expect("fixture body must have at least one child element")
        .as_str()
        .to_owned();

    let project_handle = ProjectHandle {
        slug: project_slug.to_owned(),
        root: temp_directory.to_path_buf(),
        dom: dom_tree,
        layout: project_layout,
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
    };

    let project_registry = new_registry();
    {
        let mut registry_guard = project_registry.write().await;
        registry_guard.insert(project_slug.to_owned(), project_handle);
    }

    let (file_tx, _file_rx) =
        tokio::sync::watch::channel(yantra_canvas::editor::FileChange::idle());
    let application_state = AppState {
        projects: project_registry,
        graph: yantra_canvas::GraphState::idle(),
        file_changes: std::sync::Arc::new(file_tx),
        router: None,
    };

    (application_state, first_yantra_id)
}

#[tokio::test]
async fn test_apply_update_adds_class_to_element() {
    let temp_directory = unique_temp_directory("add-class");
    let project_slug = "test_project_add_class";

    let (application_state, first_child_yantra_id) =
        build_populated_app_state(project_slug, &temp_directory).await;

    let property_update = PropertyUpdate {
        yantra_id: first_child_yantra_id.clone(),
        add_classes: vec!["bg-red-500".to_owned()],
        remove_classes: Vec::new(),
        set_style: HashMap::new(),
    };

    let file_change_result =
        yantra_canvas::editor::apply_update(&application_state, project_slug, &property_update)
            .await;

    assert!(
        file_change_result.is_ok(),
        "apply_update must succeed when project and yantra_id are valid, got: {file_change_result:?}"
    );

    let file_change = file_change_result.unwrap();
    assert_eq!(
        file_change.project, project_slug,
        "FileChange.project must match the slug passed to apply_update"
    );
    assert_eq!(
        file_change.yantra_id, first_child_yantra_id,
        "FileChange.yantra_id must match the yantra_id in the PropertyUpdate"
    );
    assert!(
        file_change.path.exists(),
        "FileChange.path must point to an existing file on disk"
    );

    std::fs::remove_dir_all(&temp_directory).ok();
}

#[tokio::test]
async fn test_apply_update_unknown_project_returns_error() {
    let application_state = AppState::empty();

    let property_update = PropertyUpdate {
        yantra_id: "aabbccddeeff".to_owned(),
        add_classes: vec!["text-blue-500".to_owned()],
        remove_classes: Vec::new(),
        set_style: HashMap::new(),
    };

    let apply_result = yantra_canvas::editor::apply_update(
        &application_state,
        "nonexistent_project_xyz",
        &property_update,
    )
    .await;

    assert!(
        apply_result.is_err(),
        "apply_update must return Err for an unregistered project slug"
    );

    let canvas_error = apply_result.unwrap_err();
    assert!(
        matches!(canvas_error, CanvasError::UnknownProject(_)),
        "error must be CanvasError::UnknownProject, got: {canvas_error:?}"
    );
}

#[tokio::test]
async fn test_apply_update_unknown_yantra_id_returns_error() {
    let temp_directory = unique_temp_directory("bad-yantra-id");
    let project_slug = "test_project_bad_yantra_id";

    let (application_state, _) = build_populated_app_state(project_slug, &temp_directory).await;

    let property_update = PropertyUpdate {
        yantra_id: "000000000000".to_owned(),
        add_classes: vec!["text-green-500".to_owned()],
        remove_classes: Vec::new(),
        set_style: HashMap::new(),
    };

    let apply_result =
        yantra_canvas::editor::apply_update(&application_state, project_slug, &property_update)
            .await;

    assert!(
        apply_result.is_err(),
        "apply_update must return Err when yantra_id does not exist in the project DOM"
    );

    let canvas_error = apply_result.unwrap_err();
    assert!(
        matches!(canvas_error, CanvasError::UnknownYantraId { .. }),
        "error must be CanvasError::UnknownYantraId, got: {canvas_error:?}"
    );

    std::fs::remove_dir_all(&temp_directory).ok();
}
