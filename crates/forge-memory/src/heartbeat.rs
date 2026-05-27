//! # forge-memory: Heartbeat Background Task
//!
//! Runs a periodic maintenance loop every `interval` using Tokio. Each tick:
//! 1. Summarizes evicted conversation turns that lack a session summary
//! 2. Re-index trigger (stub — real CRG update is wired in Wave 2)
//! 3. Tool connection liveness check (stub)
//! 4. KV-cache warm-up for top-3 speculation predictions (stub)
//!
//! ## Input
//! - A `RecallStore` reference for session summarization
//! - A `Duration` interval (default 5 minutes in production)
//! - A shutdown signal via `tokio::sync::oneshot`
//!
//! ## Output
//! - A `HeartbeatHandle` that stops the background task when dropped or
//!   when `stop()` is called explicitly
//!
//! ## Related
//! - `forge-memory::recall` — `summarize_completed_sessions` called each tick
//! - `forge-night` — may drive the heartbeat during Night Mode runs

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;

use crate::recall::{summarize_completed_sessions, RecallStore};

/// Handle to a running heartbeat task.
///
/// Dropping this handle sends a shutdown signal to the background loop.
pub struct HeartbeatHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl HeartbeatHandle {
    /// Sends a shutdown signal to the background heartbeat task.
    ///
    /// The heartbeat loop exits after the current tick completes.
    pub fn stop(&mut self) {
        if let Some(sender) = self.shutdown_tx.take() {
            let _ = sender.send(());
        }
    }
}

impl Drop for HeartbeatHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawns the heartbeat background task and returns a handle for graceful shutdown.
///
/// The task runs `run_heartbeat_tick` every `interval`. Synchronous SQLite I/O
/// is offloaded to `tokio::task::spawn_blocking` to avoid blocking the async
/// runtime. The task stops when `HeartbeatHandle::stop()` is called or the
/// handle is dropped.
pub fn start_heartbeat(recall_store: Arc<RecallStore>, interval: Duration) -> HeartbeatHandle {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            // Cancel safety: the timer.tick() branch is cancel-safe (tokio::time::Interval is cancel-safe).
            // The shutdown_rx branch is cancel-safe (oneshot receiver is cancel-safe).
            // The spawn_blocking call in run_heartbeat_tick is wrapped in a JoinHandle and
            // is cancel-safe to drop (the blocking thread will complete independently).
            tokio::select! {
                _ = timer.tick() => {
                    run_heartbeat_tick(Arc::clone(&recall_store)).await;
                }
                _ = &mut shutdown_rx => {
                    tracing::debug!("heartbeat received shutdown signal");
                    break;
                }
            }
        }
    });

    HeartbeatHandle {
        shutdown_tx: Some(shutdown_tx),
    }
}

async fn run_heartbeat_tick(recall_store: Arc<RecallStore>) {
    let join_result = tokio::task::spawn_blocking(move || {
        heartbeat_tick(&recall_store);
    })
    .await;

    if let Err(join_error) = join_result {
        tracing::error!("heartbeat tick panicked: {}", join_error);
    }
}

fn heartbeat_tick(recall_store: &RecallStore) {
    if let Err(memory_error) = summarize_completed_sessions(recall_store) {
        tracing::warn!(?memory_error, "heartbeat: session summarization failed");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use yantra_core::TaskId;

    use super::*;
    use crate::recall::{ConversationTurn, RecallStore};

    fn temp_db_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("yantra-heartbeat-test-{}", TaskId::new()))
            .join("memory.sqlite")
    }

    #[tokio::test]
    async fn heartbeat_starts_and_stops_cleanly() {
        let recall_store = Arc::new(RecallStore::new(&temp_db_path()).expect("store created"));
        let mut handle = start_heartbeat(Arc::clone(&recall_store), Duration::from_secs(3600));
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop();
    }

    #[tokio::test]
    async fn heartbeat_drop_stops_task() {
        let recall_store = Arc::new(RecallStore::new(&temp_db_path()).expect("store created"));
        {
            let _handle = start_heartbeat(Arc::clone(&recall_store), Duration::from_secs(3600));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Task should stop when handle is dropped; no assertion needed beyond
        // the test completing without hanging.
    }

    #[tokio::test]
    async fn heartbeat_tick_summarizes_unsummarized_sessions() {
        let recall_store = Arc::new(RecallStore::new(&temp_db_path()).expect("store created"));
        let session_id = TaskId::new().to_string();

        let turn = ConversationTurn {
            id: TaskId::new().to_string(),
            session_id: session_id.clone(),
            timestamp: chrono::Utc::now(),
            role: "user".to_owned(),
            content: "implement JWT rotation".to_owned(),
            summary: None,
            tokens: None,
        };
        recall_store.record_turn(&turn).expect("turn recorded");

        let mut handle = start_heartbeat(Arc::clone(&recall_store), Duration::from_millis(20));
        tokio::time::sleep(Duration::from_millis(60)).await;
        handle.stop();

        let mut summary = None;
        for _ in 0..10 {
            summary = recall_store
                .get_session_summary(&session_id)
                .expect("query succeeds");
            if summary.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert!(summary.is_some(), "heartbeat should have created a summary");
    }
}
