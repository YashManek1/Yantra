//! # forge-canvas::ws: WebSocket Hot-Reload Handler
//!
//! Provides the `/ws/:project` handler. Each connection subscribes to the
//! shared `watch::Receiver<FileChange>` and pushes a `component_swap` JSON
//! frame whenever a write lands for the matching project. The frame carries
//! the changed component's source text so the browser can patch just the
//! affected element without a full iframe reload.
//!
//! ## Input
//! - `AppState.file_changes` — `watch::Sender<FileChange>` broadcast bus
//!
//! ## Output
//! - JSON frames on connected WebSockets:
//!   `{"type":"component_swap","yantra_id":"…","component_html":"…"}`
//!
//! ## Related
//! - `forge-canvas::editor::apply_update` — fires `FileChange` events
//! - `web/canvas.html` — JS client handles `component_swap` to update elements

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Serialize;

use crate::editor::FileChange;
use crate::server::AppState;

/// WebSocket message sent to the browser after a component file is rewritten.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    /// Carries the changed component's source so the browser can do a surgical
    /// swap of the matching element without reloading the whole preview iframe.
    ComponentSwap {
        /// Stable identifier of the element that changed.
        yantra_id: String,
        /// Full text content of the rewritten TSX component file.
        component_html: String,
    },
}

/// HTTP upgrade handler for `GET /ws/:project`.
pub async fn ws_handler(
    upgrade: WebSocketUpgrade,
    Path(project_slug): Path<String>,
    State(state): State<AppState>,
) -> Response {
    upgrade.on_upgrade(move |socket| handle_socket(socket, project_slug, state))
}

async fn handle_socket(mut socket: WebSocket, project_slug: String, state: AppState) {
    let mut file_change_rx = state.file_changes.subscribe();
    loop {
        tokio::select! {
            biased;
            change_result = file_change_rx.changed() => {
                if change_result.is_err() {
                    break;
                }
                let file_change: FileChange = file_change_rx.borrow().clone();
                if file_change.project != project_slug {
                    continue;
                }

                let component_source = match std::fs::read_to_string(&file_change.path) {
                    Ok(source_text) => source_text,
                    Err(read_error) => {
                        tracing::warn!(
                            path = %file_change.path.display(),
                            error = %read_error,
                            "could not read changed component file for HMR swap"
                        );
                        continue;
                    }
                };

                let server_message = ServerMessage::ComponentSwap {
                    yantra_id: file_change.yantra_id,
                    component_html: component_source,
                };
                let payload_text = serde_json::to_string(&server_message).unwrap_or_default();
                if socket.send(Message::Text(payload_text.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}
