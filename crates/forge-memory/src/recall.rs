//! # forge-memory: Recall (Episodic) Memory Tier
//!
//! Persists conversation turns and session summaries to SQLite. Provides
//! retrieval by session ID and background summarization of completed sessions.
//!
//! ## Input
//! - `ConversationTurn` values from agent loops
//! - Session-level summary text from the Memory Consolidation agent
//!
//! ## Output
//! - `Vec<ConversationTurn>` ordered by timestamp for context injection
//! - `SessionSummary` records for long-range recall across session boundaries
//!
//! ## Related
//! - `forge-memory::working` — consumes turns for prompt assembly
//! - `forge-memory::heartbeat` — triggers session summarization on eviction

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use yantra_core::db::{apply_migrations, connection_pool, DbPool};

use crate::error::{MemoryError, MemoryResult};

type SessionSummaryRow = (
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

const RECALL_MIGRATION: &str = "
CREATE TABLE IF NOT EXISTS conversation_turns (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    timestamp   TEXT NOT NULL,
    role        TEXT NOT NULL,
    content     TEXT NOT NULL,
    summary     TEXT,
    tokens      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_turns_session ON conversation_turns(session_id, timestamp);

CREATE TABLE IF NOT EXISTS session_summaries (
    session_id     TEXT PRIMARY KEY,
    started_at     TEXT,
    ended_at       TEXT,
    summary        TEXT NOT NULL,
    accomplished   TEXT,
    unresolved     TEXT,
    decisions_made TEXT
);
";

/// A single exchange between user and assistant, persisted for recall.
#[derive(Debug, Clone)]
pub struct ConversationTurn {
    /// Unique turn identifier (UUID).
    pub id: String,
    /// Session this turn belongs to.
    pub session_id: String,
    /// When the turn occurred.
    pub timestamp: DateTime<Utc>,
    /// Either `"user"` or `"assistant"`.
    pub role: String,
    /// Raw content of the message.
    pub content: String,
    /// Optional compressed summary (set when content is evicted).
    pub summary: Option<String>,
    /// Token count of `content`, if measured.
    pub tokens: Option<i64>,
}

/// Aggregated knowledge produced at the end of a conversation session.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Identifies the session being summarized.
    pub session_id: String,
    /// Timestamp of the first turn in the session.
    pub started_at: Option<DateTime<Utc>>,
    /// Timestamp of the last turn in the session.
    pub ended_at: Option<DateTime<Utc>>,
    /// High-level summary of what occurred.
    pub summary: String,
    /// Tasks or goals that were completed.
    pub accomplished: Option<String>,
    /// Open questions or unfinished items.
    pub unresolved: Option<String>,
    /// Key decisions made during the session.
    pub decisions_made: Option<String>,
}

/// Persistent episodic memory store backed by SQLite.
pub struct RecallStore {
    database_pool: DbPool,
}

impl RecallStore {
    /// Opens or creates the recall database at `db_path`.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database open or migration failure.
    pub fn new(db_path: &Path) -> MemoryResult<Self> {
        if let Some(parent_dir) = db_path.parent() {
            std::fs::create_dir_all(parent_dir).map_err(|source| MemoryError::DirectoryCreate {
                path: parent_dir.display().to_string(),
                source,
            })?;
        }
        let database_pool = connection_pool(db_path)?;
        let database_guard = database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        apply_migrations(&database_guard, &[RECALL_MIGRATION])?;
        drop(database_guard);
        Ok(Self { database_pool })
    }

    /// Records a conversation turn in the database.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database write failure.
    pub fn record_turn(&self, turn: &ConversationTurn) -> MemoryResult<()> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        database_guard.execute(
            "INSERT OR REPLACE INTO conversation_turns
             (id, session_id, timestamp, role, content, summary, tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                turn.id,
                turn.session_id,
                turn.timestamp.to_rfc3339(),
                turn.role,
                turn.content,
                turn.summary,
                turn.tokens,
            ],
        )?;
        Ok(())
    }

    /// Returns all turns for a session in chronological order.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database read failure.
    pub fn query_session(&self, session_id: &str) -> MemoryResult<Vec<ConversationTurn>> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        let mut statement = database_guard.prepare(
            "SELECT id, session_id, timestamp, role, content, summary, tokens
             FROM conversation_turns
             WHERE session_id = ?1
             ORDER BY timestamp ASC",
        )?;
        let turns = statement
            .query_map(rusqlite::params![session_id], |row| {
                let timestamp_string: String = row.get(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    timestamp_string,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            })?
            .filter_map(|row_result| {
                row_result.ok().and_then(
                    |(id, session_id, timestamp_string, role, content, summary, tokens)| {
                        let timestamp = DateTime::parse_from_rfc3339(&timestamp_string)
                            .ok()?
                            .with_timezone(&Utc);
                        Some(ConversationTurn {
                            id,
                            session_id,
                            timestamp,
                            role,
                            content,
                            summary,
                            tokens,
                        })
                    },
                )
            })
            .collect();
        Ok(turns)
    }

    /// Stores a completed-session summary.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database write failure.
    pub fn summarize_session(
        &self,
        session_id: &str,
        summary_text: &str,
        accomplished: Option<&str>,
        unresolved: Option<&str>,
        decisions_made: Option<&str>,
    ) -> MemoryResult<()> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;

        let (started_at, ended_at) = session_time_bounds(&database_guard, session_id)?;

        database_guard.execute(
            "INSERT OR REPLACE INTO session_summaries
             (session_id, started_at, ended_at, summary, accomplished, unresolved, decisions_made)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                session_id,
                started_at,
                ended_at,
                summary_text,
                accomplished,
                unresolved,
                decisions_made,
            ],
        )?;
        Ok(())
    }

    /// Returns the stored summary for a session, if one exists.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database read failure.
    pub fn get_session_summary(&self, session_id: &str) -> MemoryResult<Option<SessionSummary>> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        let row_opt: Option<SessionSummaryRow> = database_guard
            .query_row(
                "SELECT session_id, started_at, ended_at, summary,
                         accomplished, unresolved, decisions_made
                 FROM session_summaries WHERE session_id = ?1",
                rusqlite::params![session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()?;

        Ok(row_opt.map(
            |(
                session_id,
                started_at_str,
                ended_at_str,
                summary,
                accomplished,
                unresolved,
                decisions_made,
            )| {
                let started_at = started_at_str
                    .and_then(|timestamp_string| {
                        DateTime::parse_from_rfc3339(&timestamp_string).ok()
                    })
                    .map(|datetime| datetime.with_timezone(&Utc));
                let ended_at = ended_at_str
                    .and_then(|timestamp_string| {
                        DateTime::parse_from_rfc3339(&timestamp_string).ok()
                    })
                    .map(|datetime| datetime.with_timezone(&Utc));
                SessionSummary {
                    session_id,
                    started_at,
                    ended_at,
                    summary,
                    accomplished,
                    unresolved,
                    decisions_made,
                }
            },
        ))
    }

    /// Returns the IDs of the `limit` most recently active sessions.
    ///
    /// Sessions are ordered by their most recent conversation turn timestamp,
    /// descending. Used by the heartbeat task 4 KV-cache warm-up.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database read failure.
    pub fn get_recent_session_ids(&self, limit: usize) -> MemoryResult<Vec<String>> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        let mut statement = database_guard.prepare(
            "SELECT DISTINCT session_id
             FROM conversation_turns
             GROUP BY session_id
             ORDER BY MAX(timestamp) DESC
             LIMIT ?1",
        )?;
        let session_ids: Vec<String> = statement
            .query_map(rusqlite::params![limit as i64], |row| row.get(0))?
            .filter_map(std::result::Result::ok)
            .collect();
        Ok(session_ids)
    }
}

fn session_time_bounds(
    database: &Connection,
    session_id: &str,
) -> MemoryResult<(Option<String>, Option<String>)> {
    let started_at: Option<String> = database
        .query_row(
            "SELECT MIN(timestamp) FROM conversation_turns WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    let ended_at: Option<String> = database
        .query_row(
            "SELECT MAX(timestamp) FROM conversation_turns WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    Ok((started_at, ended_at))
}

/// Summarizes all sessions that have turns but no summary record yet.
///
/// Called by the heartbeat to keep the recall tier compact.
///
/// # Errors
///
/// Returns `MemoryError` on database failure.
pub fn summarize_completed_sessions(recall_store: &RecallStore) -> MemoryResult<()> {
    let database_guard = recall_store
        .database_pool
        .lock()
        .map_err(|_| MemoryError::LockPoisoned)?;

    let unsummarized_sessions: Vec<String> = {
        let mut statement = database_guard.prepare(
            "SELECT DISTINCT t.session_id
             FROM conversation_turns t
             LEFT JOIN session_summaries s ON t.session_id = s.session_id
             WHERE s.session_id IS NULL",
        )?;
        let sessions: Vec<String> = statement
            .query_map([], |row| row.get(0))?
            .filter_map(std::result::Result::ok)
            .collect();
        sessions
    };
    drop(database_guard);

    for session_id in unsummarized_sessions {
        let turns = recall_store.query_session(&session_id)?;
        if turns.is_empty() {
            continue;
        }
        let summary_text = build_turn_summary(&turns);
        recall_store.summarize_session(&session_id, &summary_text, None, None, None)?;
    }

    Ok(())
}

fn build_turn_summary(turns: &[ConversationTurn]) -> String {
    let turn_count = turns.len();
    let first_content_preview: &str = turns.first().map_or("", |turn| turn.content.as_str());
    let preview_length = first_content_preview.len().min(120);
    format!(
        "Session with {turn_count} turns. First message: \"{}…\"",
        &first_content_preview[..preview_length]
    )
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use yantra_core::TaskId;

    use super::*;

    fn temp_db_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("yantra-recall-test-{}", TaskId::new()))
            .join("memory.sqlite")
    }

    fn sample_turn(session_id: &str, role: &str, content: &str) -> ConversationTurn {
        ConversationTurn {
            id: TaskId::new().to_string(),
            session_id: session_id.to_owned(),
            timestamp: Utc::now(),
            role: role.to_owned(),
            content: content.to_owned(),
            summary: None,
            tokens: None,
        }
    }

    #[test]
    fn record_and_query_turns_round_trips() {
        let recall_store = RecallStore::new(&temp_db_path()).expect("store created");
        let session_id = TaskId::new().to_string();
        let turn_a = sample_turn(&session_id, "user", "what is the bug?");
        let turn_b = sample_turn(&session_id, "assistant", "null dereference in auth.rs");
        recall_store.record_turn(&turn_a).expect("turn A recorded");
        recall_store.record_turn(&turn_b).expect("turn B recorded");

        let turns = recall_store
            .query_session(&session_id)
            .expect("query succeeds");
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[1].role, "assistant");
    }

    #[test]
    fn summarize_session_stores_and_retrieves_summary() {
        let recall_store = RecallStore::new(&temp_db_path()).expect("store created");
        let session_id = TaskId::new().to_string();
        let turn = sample_turn(&session_id, "user", "implement rate limiting");
        recall_store.record_turn(&turn).expect("turn recorded");

        recall_store
            .summarize_session(
                &session_id,
                "Implemented token-bucket rate limiter",
                Some("rate limiting implemented"),
                Some("load testing not done"),
                Some("use token bucket over leaky bucket"),
            )
            .expect("summary stored");

        let summary = recall_store
            .get_session_summary(&session_id)
            .expect("query succeeds")
            .expect("summary present");
        assert!(summary.summary.contains("token-bucket"));
        assert_eq!(
            summary.accomplished.as_deref(),
            Some("rate limiting implemented")
        );
    }

    #[test]
    fn query_session_returns_empty_for_unknown_session() {
        let recall_store = RecallStore::new(&temp_db_path()).expect("store created");
        let turns = recall_store
            .query_session("nonexistent-session")
            .expect("query succeeds");
        assert!(turns.is_empty());
    }

    #[test]
    fn summarize_completed_sessions_creates_summaries_for_unsummarized() {
        let recall_store = RecallStore::new(&temp_db_path()).expect("store created");
        let session_id = TaskId::new().to_string();
        for role in ["user", "assistant", "user"] {
            let turn = sample_turn(&session_id, role, "message content");
            recall_store.record_turn(&turn).expect("turn recorded");
        }
        summarize_completed_sessions(&recall_store).expect("summarization succeeds");
        let summary = recall_store
            .get_session_summary(&session_id)
            .expect("query succeeds");
        assert!(summary.is_some(), "summary should be auto-generated");
    }
}
