//! # forge-serve::session: Session Registry and Redirect Channel Map
//!
//! Maintains an in-memory registry of active Yantra sessions and a map of
//! per-agent redirect channels. Both collections are wrapped in
//! `Arc<RwLock<...>>` so they can be shared across Axum route handlers.
//!
//! ## Input
//! - `SessionInfo` values inserted by the orchestrator integration layer
//! - `RedirectSender` values registered when agents start
//!
//! ## Output
//! - `SessionRegistry` — async-safe map from `SessionId` to `SessionInfo`
//! - `RedirectChannels` — async-safe map from agent ID string to `RedirectSender`
//!
//! ## Related
//! - `forge-serve::server` — reads `SessionRegistry` for REST responses
//! - `forge-serve::redirect` — defines `RedirectSender` and `RedirectSignal`

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use yantra_core::SessionId;

use crate::redirect::RedirectSender;

/// Snapshot of an active Yantra session exposed via the REST API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Stable identifier for this session.
    pub session_id: SessionId,
    /// Wall-clock time when the session was opened.
    pub started_at: DateTime<Utc>,
    /// Number of tasks registered in the DAG for this session.
    pub task_count: usize,
}

/// Async-safe registry mapping `SessionId` to live `SessionInfo`.
pub type SessionRegistry = Arc<RwLock<HashMap<SessionId, SessionInfo>>>;

/// Async-safe map from agent ID string to its redirect sender.
pub type RedirectChannels = Arc<RwLock<HashMap<String, RedirectSender>>>;

/// Creates an empty session registry.
pub fn empty_registry() -> SessionRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Creates an empty redirect-channel map.
pub fn empty_redirect_channels() -> RedirectChannels {
    Arc::new(RwLock::new(HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_starts_with_no_sessions() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let session_registry = empty_registry();
            let registry_guard = session_registry.read().await;
            assert!(registry_guard.is_empty());
        });
    }

    #[test]
    fn empty_redirect_channels_starts_with_no_entries() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let redirect_channels = empty_redirect_channels();
            let channels_guard = redirect_channels.read().await;
            assert!(channels_guard.is_empty());
        });
    }
}
