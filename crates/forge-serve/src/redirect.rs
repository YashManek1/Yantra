//! # forge-serve::redirect: Mid-Stream Redirect Signal
//!
//! Defines the `RedirectSignal` type pushed onto an agent's redirect channel
//! when an operator sends a course-correction instruction mid-run. Agents
//! poll their `RedirectReceiver` between atomic operations and replan on
//! receipt.
//!
//! ## Input
//! - Operator instruction string and target `agent_id` from `POST /api/redirect/:agent_id`
//!
//! ## Output
//! - `RedirectSignal` delivered over a bounded `mpsc` channel to the target agent
//! - `RedirectSender` / `RedirectReceiver` type aliases for ergonomic use
//!
//! ## Related
//! - `forge-serve::server` — creates redirect channels and routes POST requests
//! - `forge-serve::session` — `RedirectChannels` registry maps agent_id → sender

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A mid-stream instruction pushed to a running agent.
///
/// The operator fills `instruction` via the Live Canvas redirect input, and
/// the server delivers it to the agent over `RedirectSender`. The agent
/// checks its `RedirectReceiver` between atomic operations and replans.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectSignal {
    /// Identifier of the agent that should receive this instruction.
    pub agent_id: String,
    /// Free-text instruction telling the agent to change course.
    pub instruction: String,
    /// Wall-clock time at which the operator submitted the redirect.
    pub timestamp: DateTime<Utc>,
}

/// Sender half of a redirect channel.
pub type RedirectSender = tokio::sync::mpsc::Sender<RedirectSignal>;

/// Receiver half of a redirect channel.
pub type RedirectReceiver = tokio::sync::mpsc::Receiver<RedirectSignal>;

/// Creates a redirect channel with a capacity of 32 buffered signals.
///
/// Returns `(sender, receiver)`. The server holds the sender; the target
/// agent holds the receiver and polls it between atomic steps.
pub fn redirect_channel() -> (RedirectSender, RedirectReceiver) {
    tokio::sync::mpsc::channel(32)
}
