//! # forge-router: Provider Health
//!
//! Polls provider status in the background and publishes state changes over a
//! Tokio channel for Live Canvas consumers. The health loop runs every
//! 30 seconds and delegates status checks to each provider.
//!
//! ## Input
//! - Registered model providers
//! - Provider status responses
//!
//! ## Output
//! - `HealthEvent` messages over an MPSC channel
//!
//! ## Related
//! - `forge-router::provider` — provider status source
//! - `forge-router::routing` — exposes provider handles for polling

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::provider::{ModelProvider, ProviderStatus};

/// Health polling interval.
pub const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Current state for one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    /// Provider identifier.
    pub provider_id: String,
    /// Current provider status.
    pub status: ProviderStatus,
}

/// Event emitted by the health polling loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthEvent {
    /// Provider state snapshot.
    pub health: ProviderHealth,
}

/// Starts background provider health polling.
pub fn spawn_health_polling(
    providers: Vec<Arc<dyn ModelProvider>>,
    health_sender: mpsc::Sender<HealthEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut poll_interval = tokio::time::interval(HEALTH_POLL_INTERVAL);
        loop {
            poll_interval.tick().await;
            for provider in &providers {
                let status = provider.status().await;
                let health_event = HealthEvent {
                    health: ProviderHealth {
                        provider_id: provider.id().to_string(),
                        status,
                    },
                };
                if health_sender.send(health_event).await.is_err() {
                    return;
                }
            }
        }
    })
}
