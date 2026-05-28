//! # forge-serve::server: Axum Router and HTTP Entry Point
//!
//! Builds the Axum application with all REST and SSE routes, and provides
//! `serve()` to bind a TCP listener and run it to completion. All handlers
//! share `AppState` via axum's `State` extractor.
//!
//! ## Input
//! - `AppState` — broadcast sender for events, session registry, redirect channels
//! - `SocketAddr` — TCP address to bind (caller decides localhost vs. 0.0.0.0)
//!
//! ## Output
//! - `axum::Router` — composable router for embedding in tests or larger apps
//! - Bound HTTP server accepting connections until the process exits
//!
//! ## Related
//! - `forge-serve::sse` — SSE handler mounted at `GET /api/stream`
//! - `forge-serve::session` — `SessionRegistry` and `RedirectChannels`
//! - `forge-serve::redirect` — `RedirectSignal` pushed by `POST /api/redirect/:id`

use std::net::SocketAddr;
use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use yantra_core::SessionId;
use yantra_orchestrator::AgentMessage;

use crate::error::{ServeError, ServeResult};
use crate::redirect::RedirectSignal;
use crate::session::{RedirectChannels, SessionInfo, SessionRegistry};
use crate::sse::sse_handler;

static DASHBOARD_HTML: &str = include_str!("../web/index.html");

/// Shared state injected into every Axum route handler.
#[derive(Clone)]
pub struct AppState {
    /// Broadcast sender for orchestrator events; handlers call `.subscribe()`.
    pub event_sender: broadcast::Sender<AgentMessage>,
    /// Live session metadata visible via `GET /api/sessions`.
    pub sessions: SessionRegistry,
    /// Per-agent redirect channels for `POST /api/redirect/:agent_id`.
    pub redirect_channels: RedirectChannels,
}

/// Builds the Axum router with all routes attached.
///
/// Separated from `serve` so tests can call `router.oneshot(...)` without
/// binding a real TCP socket.
pub fn build_router(application_state: AppState) -> Router {
    Router::new()
        .route("/", get(dashboard_handler))
        .route("/api/sessions", get(list_sessions_handler))
        .route("/api/session/{id}", get(get_session_handler))
        .route("/api/session/{id}/tasks", get(get_session_tasks_handler))
        .route("/api/stream", get(sse_handler))
        .route("/api/redirect/{agent_id}", post(redirect_handler))
        .with_state(application_state)
}

/// Binds to `bind_address` and serves the application until the process exits.
///
/// # Errors
///
/// Returns `ServeError::Bind` when the TCP listener cannot be created on the
/// requested address (e.g., port already in use or insufficient permissions).
pub async fn serve(application_state: AppState, bind_address: SocketAddr) -> ServeResult<()> {
    let tcp_listener =
        TcpListener::bind(bind_address)
            .await
            .map_err(|io_error| ServeError::Bind {
                addr: bind_address.to_string(),
                source: io_error,
            })?;

    tracing::info!(address = %bind_address, "Yantra Live Canvas listening");

    axum::serve(tcp_listener, build_router(application_state))
        .await
        .map_err(|io_error| ServeError::Bind {
            addr: bind_address.to_string(),
            source: io_error,
        })
}

async fn dashboard_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn list_sessions_handler(
    State(application_state): State<AppState>,
) -> Json<Vec<SessionInfo>> {
    let registry_guard = application_state.sessions.read().await;
    let session_list: Vec<SessionInfo> = registry_guard.values().cloned().collect();
    Json(session_list)
}

async fn get_session_handler(
    State(application_state): State<AppState>,
    Path(session_id_string): Path<String>,
) -> Response {
    let parsed_session_id = match SessionId::from_str(&session_id_string) {
        Ok(session_id) => session_id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid session id" })),
            )
                .into_response();
        }
    };

    let registry_guard = application_state.sessions.read().await;
    match registry_guard.get(&parsed_session_id) {
        Some(session_info) => Json(session_info.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response(),
    }
}

async fn get_session_tasks_handler(
    Path(_session_id_string): Path<String>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "tasks": [],
        "note": "live DAG not yet wired — start yantra run to populate"
    }))
}

/// Request body for `POST /api/redirect/:agent_id`.
#[derive(Deserialize)]
struct RedirectRequest {
    instruction: String,
}

async fn redirect_handler(
    State(application_state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(redirect_request): Json<RedirectRequest>,
) -> Response {
    let redirect_signal = RedirectSignal {
        agent_id: agent_id.clone(),
        instruction: redirect_request.instruction,
        timestamp: Utc::now(),
    };

    let channels_guard = application_state.redirect_channels.read().await;
    match channels_guard.get(&agent_id) {
        Some(redirect_sender) => {
            if redirect_sender.send(redirect_signal).await.is_err() {
                tracing::warn!(agent_id = %agent_id, "redirect channel closed before send");
                return (
                    StatusCode::GONE,
                    Json(serde_json::json!({ "error": "agent channel closed" })),
                )
                    .into_response();
            }
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "agent not found" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{empty_redirect_channels, empty_registry};

    fn test_app_state() -> AppState {
        let (event_sender, _initial_receiver) = broadcast::channel(16);
        AppState {
            event_sender,
            sessions: empty_registry(),
            redirect_channels: empty_redirect_channels(),
        }
    }

    #[tokio::test]
    async fn get_sessions_returns_empty_list() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let application_router = build_router(test_app_state());
        let http_response = application_router
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(http_response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_session_unknown_id_returns_404() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let application_router = build_router(test_app_state());
        let http_response = application_router
            .oneshot(
                Request::builder()
                    .uri("/api/session/00000000-0000-0000-0000-000000000000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(http_response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_session_tasks_returns_stub() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let application_router = build_router(test_app_state());
        let http_response = application_router
            .oneshot(
                Request::builder()
                    .uri("/api/session/00000000-0000-0000-0000-000000000000/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(http_response.status(), StatusCode::OK);
    }
}
