//! # forge-obs: Watchdog Protocol
//!
//! Defines the message protocol and client used by the runtime to communicate
//! with a watchdog process. The client emits heartbeats every 30 seconds, and
//! the watchdog treats five minutes of silence as a kill condition.
//!
//! ## Input
//! - Runtime heartbeat ticks
//! - Snapshot requests and kill commands from the watchdog
//!
//! ## Output
//! - `WatchdogMessage` values sent over the watchdog channel
//!
//! ## Related
//! - `forge-sidecar` — hosts the future separate watchdog binary
//! - `forge-orchestrator` — uses the client during task execution

use std::time::Duration;

use tokio::sync::mpsc;

use crate::error::{ObsError, ObsResult};

/// Runtime heartbeat interval.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Silence threshold after which the watchdog may kill the runtime.
pub const SILENCE_KILL_THRESHOLD: Duration = Duration::from_secs(300);

/// Message exchanged with the watchdog process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogMessage {
    /// Runtime is alive.
    Heartbeat,
    /// Watchdog asks the runtime to snapshot current state.
    RequestSnapshot,
    /// Watchdog commands runtime termination.
    KillCommand,
}

/// Client used by the main runtime to send watchdog messages.
#[derive(Debug, Clone)]
pub struct WatchdogClient {
    sender: mpsc::Sender<WatchdogMessage>,
}

impl WatchdogClient {
    /// Creates a client over an existing message channel.
    pub fn new(sender: mpsc::Sender<WatchdogMessage>) -> Self {
        Self { sender }
    }

    /// Sends one heartbeat message.
    ///
    /// # Errors
    ///
    /// Returns `ObsError::WatchdogChannelClosed` when the receiver is closed.
    pub async fn heartbeat(&self) -> ObsResult<()> {
        self.send(WatchdogMessage::Heartbeat).await
    }

    /// Sends a watchdog message.
    ///
    /// # Errors
    ///
    /// Returns `ObsError::WatchdogChannelClosed` when the receiver is closed.
    pub async fn send(&self, message: WatchdogMessage) -> ObsResult<()> {
        self.sender
            .send(message)
            .await
            .map_err(|_| ObsError::WatchdogChannelClosed)
    }

    /// Starts a background heartbeat loop.
    pub fn spawn_heartbeat(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
            loop {
                heartbeat_interval.tick().await;
                if self.heartbeat().await.is_err() {
                    break;
                }
            }
        })
    }
}
