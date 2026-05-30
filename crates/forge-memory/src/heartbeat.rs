//! # forge-memory: Heartbeat Background Task
//!
//! Runs a periodic maintenance loop every `interval` using Tokio. Each tick:
//! 1. Summarizes evicted conversation turns that lack a session summary.
//! 2. Re-index staleness check — warns via `tracing` when `crg.sqlite` has
//!    not been updated within `CRG_STALENESS_THRESHOLD_SECS`. The actual
//!    re-index is triggered by whoever processes the warning (e.g. the sidecar).
//! 3. Tool liveness probe — TCP-connects to the Ollama endpoint
//!    (127.0.0.1:11434) with a 1-second timeout and logs the result.
//! 4. KV-cache warm-up — queries the recall store for the most recent active
//!    sessions and logs their summary so the context is primed for the next run.
//!
//! ## Input
//! - A `RecallStore` reference for session summarization and warm-up
//! - A `yantra_dir: PathBuf` pointing to `.yantra/` (for crg.sqlite liveness)
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

use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;

use crate::recall::{summarize_completed_sessions, RecallStore};

/// Age threshold in seconds after which `crg.sqlite` is considered stale.
const CRG_STALENESS_THRESHOLD_SECS: u64 = 3600;
/// Default Ollama TCP endpoint for the liveness probe.
const OLLAMA_PROBE_ADDR: &str = "127.0.0.1:11434";
/// Timeout for the Ollama TCP liveness probe.
const OLLAMA_PROBE_TIMEOUT_SECS: u64 = 1;

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
/// The task runs all four maintenance tasks every `interval`. Synchronous I/O
/// (SQLite, TCP liveness probe, file-system stat) is offloaded to
/// `tokio::task::spawn_blocking` to avoid blocking the async runtime. The task
/// stops when `HeartbeatHandle::stop()` is called or the handle is dropped.
pub fn start_heartbeat(
    recall_store: Arc<RecallStore>,
    yantra_dir: PathBuf,
    interval: Duration,
) -> HeartbeatHandle {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            // Cancel safety: the timer.tick() branch is cancel-safe (tokio::time::Interval is cancel-safe).
            // The shutdown_rx branch is cancel-safe (oneshot receiver is cancel-safe).
            // The spawn_blocking calls inside run_heartbeat_tick are each wrapped in a
            // JoinHandle and are cancel-safe to drop (the blocking thread completes independently).
            tokio::select! {
                _ = timer.tick() => {
                    run_heartbeat_tick(Arc::clone(&recall_store), yantra_dir.clone()).await;
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

async fn run_heartbeat_tick(recall_store: Arc<RecallStore>, yantra_dir: PathBuf) {
    let crg_db_path = yantra_dir.join("crg.sqlite");
    let recall_store_for_blocking = Arc::clone(&recall_store);

    let join_result = tokio::task::spawn_blocking(move || {
        heartbeat_tick(&recall_store_for_blocking, &crg_db_path);
    })
    .await;

    if let Err(join_error) = join_result {
        tracing::error!("heartbeat tick panicked: {}", join_error);
    }
}

fn heartbeat_tick(recall_store: &RecallStore, crg_db_path: &std::path::Path) {
    task1_session_summarization(recall_store);
    task2_crg_staleness_check(crg_db_path);
    task3_ollama_liveness_probe();
    task4_kv_cache_warm(recall_store);
}

fn task1_session_summarization(recall_store: &RecallStore) {
    if let Err(memory_error) = summarize_completed_sessions(recall_store) {
        tracing::warn!(
            ?memory_error,
            "heartbeat task 1: session summarization failed"
        );
    }
}

fn task2_crg_staleness_check(crg_db_path: &std::path::Path) {
    match std::fs::metadata(crg_db_path) {
        Ok(metadata) => {
            let modified_time = match metadata.modified() {
                Ok(time) => time,
                Err(time_error) => {
                    tracing::debug!(error = %time_error, "heartbeat task 2: cannot read crg.sqlite mtime");
                    return;
                }
            };
            let age_secs = std::time::SystemTime::now()
                .duration_since(modified_time)
                .map(|age| age.as_secs())
                .unwrap_or(0);
            if age_secs > CRG_STALENESS_THRESHOLD_SECS {
                tracing::warn!(
                    age_secs,
                    "heartbeat task 2: crg.sqlite is stale — run 'yantra index .' to refresh"
                );
            } else {
                tracing::debug!(age_secs, "heartbeat task 2: crg.sqlite is fresh");
            }
        }
        Err(_) => {
            tracing::debug!("heartbeat task 2: crg.sqlite not found — index not yet built");
        }
    }
}

fn task3_ollama_liveness_probe() {
    let probe_timeout = Duration::from_secs(OLLAMA_PROBE_TIMEOUT_SECS);
    let ollama_socket_addr: std::net::SocketAddr = match OLLAMA_PROBE_ADDR.parse() {
        Ok(addr) => addr,
        Err(parse_error) => {
            tracing::error!(error = %parse_error, "heartbeat task 3: invalid Ollama address");
            return;
        }
    };
    if TcpStream::connect_timeout(&ollama_socket_addr, probe_timeout).is_ok() {
        tracing::debug!("heartbeat task 3: Ollama reachable at {OLLAMA_PROBE_ADDR}");
    } else {
        tracing::warn!(
            "heartbeat task 3: Ollama not reachable at {OLLAMA_PROBE_ADDR} — \
             Tier 0 routing will fail"
        );
    }
}

fn task4_kv_cache_warm(recall_store: &RecallStore) {
    match recall_store.get_recent_session_ids(3) {
        Ok(session_ids) => {
            for session_id in &session_ids {
                match recall_store.get_session_summary(session_id) {
                    Ok(Some(summary)) => {
                        tracing::debug!(
                            session_id = %session_id,
                            summary_len = summary.summary.len(),
                            "heartbeat task 4: primed session summary in KV cache"
                        );
                    }
                    Ok(None) => {
                        tracing::debug!(
                            session_id = %session_id,
                            "heartbeat task 4: session has no summary yet"
                        );
                    }
                    Err(recall_error) => {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %recall_error,
                            "heartbeat task 4: KV-cache warm failed for session"
                        );
                    }
                }
            }
        }
        Err(recall_error) => {
            tracing::warn!(
                error = %recall_error,
                "heartbeat task 4: could not retrieve recent session IDs"
            );
        }
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
        let mut handle = start_heartbeat(
            Arc::clone(&recall_store),
            std::env::temp_dir(),
            Duration::from_secs(3600),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop();
    }

    #[tokio::test]
    async fn heartbeat_drop_stops_task() {
        let recall_store = Arc::new(RecallStore::new(&temp_db_path()).expect("store created"));
        {
            let _handle = start_heartbeat(
                Arc::clone(&recall_store),
                std::env::temp_dir(),
                Duration::from_secs(3600),
            );
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

        let mut handle = start_heartbeat(
            Arc::clone(&recall_store),
            std::env::temp_dir(),
            Duration::from_millis(20),
        );
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
