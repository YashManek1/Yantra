//! # forge-canvas::server: Axum Router for Canvas Editor + Graph Viewer
//!
//! Defines `AppState` (shared registry + graph cache + optional model router),
//! builds the Axum router with all canvas/editor/graph/WebSocket routes, and
//! provides `serve()` to bind a TCP listener and run it to completion.
//!
//! ## Input
//! - `AppState` — project registry, graph cache, model router, file-change channel
//! - `SocketAddr` — bind address (CLI decides localhost vs. 0.0.0.0)
//!
//! ## Output
//! - `axum::Router` for tests (`build_router`) and `serve()` for production
//!
//! ## Related
//! - `forge-canvas::editor` — `ProjectRegistry` lives in `AppState.projects`
//! - `forge-canvas::graph_viz` — `GraphState` lives in `AppState.graph`
//! - `forge-canvas::ws` — WebSocket handler mounted at `/ws/:project`
//! - `forge-router::Router` — optional Tier-0 provider for graph explain

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tower_http::services::ServeDir;

use yantra_router::Router as ModelRouter;

use crate::editor::{
    apply_update, redo_update, undo_update, FileChange, ProjectRegistry, PropertyUpdate,
};
use crate::error::{CanvasError, CanvasResult};
use crate::graph_viz::{graph_explain_handler, graph_json_handler, GraphState};
use crate::ws::ws_handler;

static CANVAS_HTML: &str = include_str!("../web/canvas.html");
static GRAPH_HTML: &str = include_str!("../web/graph.html");
static INDEX_HTML: &str = include_str!("../web/index.html");

/// Shared state injected into every Axum route handler.
#[derive(Clone)]
pub struct AppState {
    /// Per-project handle map (DOM, layout, project root).
    pub projects: ProjectRegistry,
    /// CRG graph state shared with `/api/graph/*` handlers.
    pub graph: GraphState,
    /// Broadcast sender for file changes; WebSocket handlers subscribe.
    pub file_changes: Arc<watch::Sender<FileChange>>,
    /// Optional model router used by `POST /api/graph/explain` for LLM explanations.
    /// When `None` the explain endpoint returns the structural summary fallback.
    pub router: Option<Arc<ModelRouter>>,
}

impl AppState {
    /// Builds an `AppState` with no projects registered, an idle graph cache,
    /// and no LLM router. The graph cache lazy-loads from `.yantra/crg.sqlite`
    /// on the first `/api/graph/json` request.
    pub fn empty() -> Self {
        let (file_tx, _file_rx) = watch::channel(FileChange::idle());
        Self {
            projects: crate::editor::new_registry(),
            graph: GraphState::idle(),
            file_changes: Arc::new(file_tx),
            router: None,
        }
    }

    /// Builds an `AppState` wired with a live model router for LLM explanations.
    pub fn with_router(model_router: Arc<ModelRouter>) -> Self {
        let (file_tx, _file_rx) = watch::channel(FileChange::idle());
        Self {
            projects: crate::editor::new_registry(),
            graph: GraphState::idle(),
            file_changes: Arc::new(file_tx),
            router: Some(model_router),
        }
    }
}

/// Root directory under which per-project static files are emitted.
///
/// Relative to the server's working directory: `./yantra-canvas/`.
const CANVAS_BASE_DIR: &str = "yantra-canvas";

/// Builds the Axum router with all canvas, editor, graph, preview, and
/// undo/redo routes attached.
pub fn build_router(application_state: AppState) -> Router {
    let preview_service = build_preview_nest();

    Router::new()
        .route("/", get(index_handler))
        .route("/editor/{project}", get(editor_handler))
        .route("/graph", get(graph_view_handler))
        .route("/api/dom/{project}/tree", get(dom_tree_handler))
        .route("/api/dom/{project}/select", post(dom_select_handler))
        .route("/api/dom/{project}/update", post(dom_update_handler))
        .route("/api/dom/{project}/undo", post(dom_undo_handler))
        .route("/api/dom/{project}/redo", post(dom_redo_handler))
        .route("/api/graph/json", get(graph_json_handler))
        .route("/api/graph/explain", post(graph_explain_handler))
        .route("/ws/{project}", get(ws_handler))
        .nest_service("/preview", preview_service)
        .with_state(application_state)
}

/// Returns a `ServeDir` service rooted at `./yantra-canvas/` that serves the
/// emitted static project files. Path traversal outside the base directory is
/// prevented by `ServeDir`'s built-in path sanitization.
fn build_preview_nest() -> ServeDir {
    let base_path = PathBuf::from(CANVAS_BASE_DIR);
    ServeDir::new(base_path)
}

/// Binds to `bind_address` and serves the application until the process exits.
///
/// # Errors
///
/// Returns `CanvasError::Bind` when the TCP listener cannot be created on
/// the requested address (port already in use, insufficient permissions).
pub async fn serve(application_state: AppState, bind_address: SocketAddr) -> CanvasResult<()> {
    let tcp_listener =
        TcpListener::bind(bind_address)
            .await
            .map_err(|io_error| CanvasError::Bind {
                addr: bind_address.to_string(),
                source: io_error,
            })?;

    tracing::info!(address = %bind_address, "Yantra Canvas listening");

    axum::serve(tcp_listener, build_router(application_state))
        .await
        .map_err(|io_error| CanvasError::Bind {
            addr: bind_address.to_string(),
            source: io_error,
        })
}

async fn index_handler() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn editor_handler(Path(_project): Path<String>) -> Html<&'static str> {
    Html(CANVAS_HTML)
}

async fn graph_view_handler() -> Html<&'static str> {
    Html(GRAPH_HTML)
}

async fn dom_tree_handler(State(state): State<AppState>, Path(project): Path<String>) -> Response {
    let registry_guard = state.projects.read().await;
    match registry_guard.get(&project) {
        Some(handle) => Json(handle.dom_summary()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "unknown project", "project": project })),
        )
            .into_response(),
    }
}

/// Request body for `POST /api/dom/:project/select`.
#[derive(Debug, Deserialize)]
struct SelectRequest {
    yantra_id: String,
}

/// Response body for `POST /api/dom/:project/select`.
#[derive(Debug, Serialize)]
struct SelectResponse {
    component_file: String,
    tag: String,
    classes: Vec<String>,
    styles: serde_json::Value,
}

async fn dom_select_handler(
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<SelectRequest>,
) -> Response {
    let registry_guard = state.projects.read().await;
    let handle = match registry_guard.get(&project) {
        Some(handle) => handle,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "unknown project", "project": project })),
            )
                .into_response();
        }
    };
    match handle.select(&request.yantra_id) {
        Some(info) => Json(SelectResponse {
            component_file: info.component_file.display().to_string(),
            tag: info.tag,
            classes: info.classes,
            styles: serde_json::to_value(info.styles).unwrap_or_default(),
        })
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "unknown yantra_id",
                "yantra_id": request.yantra_id,
            })),
        )
            .into_response(),
    }
}

async fn dom_update_handler(
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(update): Json<PropertyUpdate>,
) -> Response {
    match apply_update(&state, &project, &update).await {
        Ok(file_change) => {
            let _ = state.file_changes.send(file_change.clone());
            Json(serde_json::json!({
                "ok": true,
                "file_changed": file_change.path.display().to_string(),
            }))
            .into_response()
        }
        Err(canvas_error) => {
            tracing::warn!(error = %canvas_error, "dom update failed");
            let status = match canvas_error {
                CanvasError::UnknownProject(_) | CanvasError::UnknownYantraId { .. } => {
                    StatusCode::NOT_FOUND
                }
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (
                status,
                Json(serde_json::json!({ "error": canvas_error.to_string() })),
            )
                .into_response()
        }
    }
}

async fn dom_undo_handler(State(state): State<AppState>, Path(project): Path<String>) -> Response {
    match undo_update(&state, &project).await {
        Ok(file_change) => {
            let _ = state.file_changes.send(file_change.clone());
            Json(serde_json::json!({
                "ok": true,
                "file_changed": file_change.path.display().to_string(),
            }))
            .into_response()
        }
        Err(canvas_error) => {
            tracing::warn!(error = %canvas_error, "dom undo failed");
            let status = match &canvas_error {
                CanvasError::UnknownProject(_) | CanvasError::UnknownYantraId { .. } => {
                    StatusCode::NOT_FOUND
                }
                CanvasError::EmptyUndoStack(_) => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (
                status,
                Json(serde_json::json!({ "error": canvas_error.to_string() })),
            )
                .into_response()
        }
    }
}

async fn dom_redo_handler(State(state): State<AppState>, Path(project): Path<String>) -> Response {
    match redo_update(&state, &project).await {
        Ok(file_change) => {
            let _ = state.file_changes.send(file_change.clone());
            Json(serde_json::json!({
                "ok": true,
                "file_changed": file_change.path.display().to_string(),
            }))
            .into_response()
        }
        Err(canvas_error) => {
            tracing::warn!(error = %canvas_error, "dom redo failed");
            let status = match &canvas_error {
                CanvasError::UnknownProject(_) | CanvasError::UnknownYantraId { .. } => {
                    StatusCode::NOT_FOUND
                }
                CanvasError::EmptyRedoStack(_) => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (
                status,
                Json(serde_json::json!({ "error": canvas_error.to_string() })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn index_route_returns_html() {
        let router = build_router(AppState::empty());
        let response = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn editor_route_returns_html_for_any_project() {
        let router = build_router(AppState::empty());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/editor/example_com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn dom_tree_for_unknown_project_returns_404() {
        let router = build_router(AppState::empty());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/dom/no_such_project/tree")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
