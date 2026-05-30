//! # forge-canvas: Axum Server Route Coverage Tests
//!
//! Verifies that every registered route in `build_router` responds with the
//! expected HTTP status code, and documents the known gap where `/preview`
//! is referenced in the canvas UI but not yet wired in the Phase-1 router.
//!
//! ## Input
//! - `AppState::empty()` — no projects registered, no SQLite graph database
//! - In-process `tower::ServiceExt::oneshot` requests (no real TCP listener)
//!
//! ## Output
//! - Test assertion results on HTTP status codes for each route
//!
//! ## Related
//! - `forge-canvas::server` — `build_router`, `AppState`
//! - `forge-canvas::graph_viz` — `/api/graph/*` handlers

use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;
use yantra_canvas::server::{build_router, AppState};

fn empty_get_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .expect("request construction must succeed for valid URI")
}

#[tokio::test]
async fn test_root_route_returns_200() {
    let application_state = AppState::empty();
    let router = build_router(application_state);

    let response = router
        .oneshot(empty_get_request("/"))
        .await
        .expect("router must handle GET /");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET / must return HTTP 200 OK"
    );
}

#[tokio::test]
async fn test_editor_route_returns_200() {
    let application_state = AppState::empty();
    let router = build_router(application_state);

    let response = router
        .oneshot(empty_get_request("/editor/test_project"))
        .await
        .expect("router must handle GET /editor/:project");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /editor/:project must return HTTP 200 OK (serves canvas.html regardless of project existence)"
    );
}

#[tokio::test]
async fn test_graph_route_returns_200() {
    let application_state = AppState::empty();
    let router = build_router(application_state);

    let response = router
        .oneshot(empty_get_request("/graph"))
        .await
        .expect("router must handle GET /graph");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /graph must return HTTP 200 OK (serves graph.html)"
    );
}

#[tokio::test]
async fn test_dom_tree_unknown_project_returns_404() {
    let application_state = AppState::empty();
    let router = build_router(application_state);

    let response = router
        .oneshot(empty_get_request("/api/dom/nonexistent/tree"))
        .await
        .expect("router must handle GET /api/dom/:project/tree");

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "GET /api/dom/:project/tree must return HTTP 404 when project is not registered"
    );
}

#[tokio::test]
async fn test_graph_json_endpoint_returns_200_or_unavailable() {
    let application_state = AppState::empty();
    let router = build_router(application_state);

    let response = router
        .oneshot(empty_get_request("/api/graph/json"))
        .await
        .expect("router must handle GET /api/graph/json");

    let response_status = response.status();
    assert!(
        response_status == StatusCode::OK || response_status == StatusCode::SERVICE_UNAVAILABLE,
        "GET /api/graph/json must return 200 (cache hit) or 503 (no SQLite database); got {response_status}"
    );
}

#[tokio::test]
async fn test_preview_route_returns_404_for_missing_project_directory() {
    // The /preview route is wired (Phase 2) and serves static files from
    // ./yantra-canvas/<project>/. When the project directory does not exist on
    // disk (as is the case in this in-process test), ServeDir returns 404.
    let application_state = AppState::empty();
    let router = build_router(application_state);

    let response = router
        .oneshot(empty_get_request("/preview/test_project/index.html"))
        .await
        .expect("router must handle GET /preview/:project/:path without panicking");

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "GET /preview/:project/index.html returns 404 when the project directory is absent"
    );
}
