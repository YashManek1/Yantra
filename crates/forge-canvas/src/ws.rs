//! # forge-canvas::ws: WebSocket Hot-Reload Handler
//!
//! Provides the `/ws/:project` handler. Each connection subscribes to the
//! shared `watch::Receiver<FileChange>` and pushes `{type: "reload"}` JSON
//! frames whenever a write lands for the matching project. Clients reload
//! the preview iframe in response.
//!
//! ## Input
//! - `AppState.file_changes` — `watch::Sender<FileChange>` broadcast bus
//!
//! ## Output
//! - JSON frames on connected WebSockets
//!
//! ## Related
//! - `forge-canvas::editor::apply_update` — fires `FileChange` events
//! - `web/canvas.html` — JS client that reconnects on close

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Serialize;

use crate::editor::FileChange;
use crate::server::AppState;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Reload { path: String, yantra_id: String },
}

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
                let server_message = ServerMessage::Reload {
                    path: file_change.path.display().to_string(),
                    yantra_id: file_change.yantra_id,
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
