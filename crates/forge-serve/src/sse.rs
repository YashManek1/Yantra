//! # forge-serve::sse: Server-Sent Events Stream Handler
//!
//! Subscribes to the orchestrator `broadcast::Sender<AgentMessage>` carried
//! in `AppState` and converts each event to an SSE `data:` frame. Slow
//! clients that fall behind the channel buffer have their lag logged and
//! missed messages skipped — they stay connected.
//!
//! ## Input
//! - `AppState::event_sender` — cloneable broadcast sender; handler calls
//!   `.subscribe()` to obtain its own receiver
//!
//! ## Output
//! - SSE stream of `data: {json}\n\n` frames, one per `AgentMessage`
//! - Keep-alive pings at the axum default interval (~15 s) so proxies do not
//!   time out idle connections
//!
//! ## Related
//! - `forge-serve::server` — mounts this handler at `GET /api/stream`
//! - `forge-orchestrator::event_bus` — `AgentMessage` source and `EventBus`

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream;
use serde::Serialize;
use tokio::sync::broadcast;
use yantra_orchestrator::AgentMessage;

use crate::server::AppState;

/// JSON payload emitted as the `data:` field of each SSE frame.
///
/// `AgentMessage` does not derive `Serialize`, so each variant is mapped to
/// this DTO before serialization.
#[derive(Serialize)]
struct SseEventPayload {
    event_type: String,
    #[serde(flatten)]
    data: serde_json::Value,
}

/// Axum handler for `GET /api/stream`.
///
/// Returns an infinite SSE stream that replays every `AgentMessage` published
/// on the orchestrator event bus. The stream ends only when the broadcast
/// channel is dropped (server shutdown).
pub async fn sse_handler(
    State(application_state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let broadcast_receiver = application_state.event_sender.subscribe();

    let event_stream = stream::unfold(broadcast_receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(agent_message) => {
                    let sse_payload = agent_message_to_payload(&agent_message);
                    let json_string = serde_json::to_string(&sse_payload)
                        .unwrap_or_else(|_| r#"{"event_type":"serialize_error"}"#.to_owned());
                    let sse_event = Event::default().data(json_string);
                    return Some((Ok(sse_event), receiver));
                }
                Err(broadcast::error::RecvError::Lagged(skipped_message_count)) => {
                    tracing::warn!(
                        skipped = skipped_message_count,
                        "SSE subscriber lagged; skipping messages"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(event_stream).keep_alive(KeepAlive::default())
}

fn agent_message_to_payload(agent_message: &AgentMessage) -> SseEventPayload {
    match agent_message {
        AgentMessage::TaskQueued { task_id } => SseEventPayload {
            event_type: "task_queued".to_owned(),
            data: serde_json::json!({ "task_id": task_id.to_string() }),
        },
        AgentMessage::TaskStarted {
            task_id,
            agent_kind,
        } => SseEventPayload {
            event_type: "task_started".to_owned(),
            data: serde_json::json!({
                "task_id": task_id.to_string(),
                "agent_kind": format!("{agent_kind:?}"),
            }),
        },
        AgentMessage::TaskCompleted {
            task_id,
            decision_id,
            agent_kind,
            summary,
        } => SseEventPayload {
            event_type: "task_completed".to_owned(),
            data: serde_json::json!({
                "task_id": task_id.to_string(),
                "decision_id": decision_id.to_string(),
                "agent_kind": format!("{agent_kind:?}"),
                "summary": summary,
            }),
        },
        AgentMessage::TaskFailed {
            task_id,
            reason,
            retry_count,
        } => SseEventPayload {
            event_type: "task_failed".to_owned(),
            data: serde_json::json!({
                "task_id": task_id.to_string(),
                "reason": reason,
                "retry_count": retry_count,
            }),
        },
        AgentMessage::TaskMaxRetries { task_id } => SseEventPayload {
            event_type: "task_max_retries".to_owned(),
            data: serde_json::json!({ "task_id": task_id.to_string() }),
        },
        AgentMessage::CircuitOpen { calls_per_minute } => SseEventPayload {
            event_type: "circuit_open".to_owned(),
            data: serde_json::json!({ "calls_per_minute": calls_per_minute }),
        },
        AgentMessage::CircuitHalfOpen => SseEventPayload {
            event_type: "circuit_half_open".to_owned(),
            data: serde_json::json!({}),
        },
        AgentMessage::CircuitClosed => SseEventPayload {
            event_type: "circuit_closed".to_owned(),
            data: serde_json::json!({}),
        },
        AgentMessage::SpeculationReady {
            predicted_descriptions,
        } => SseEventPayload {
            event_type: "speculation_ready".to_owned(),
            data: serde_json::json!({ "predictions": predicted_descriptions }),
        },
        AgentMessage::DagUpdated { ready_count } => SseEventPayload {
            event_type: "dag_updated".to_owned(),
            data: serde_json::json!({ "ready_count": ready_count }),
        },
    }
}
