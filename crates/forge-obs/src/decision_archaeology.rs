//! # forge-obs: Decision Archaeology
//!
//! Persists the causal decision log used for time-travel debugging, surgical
//! rollback, and audit trails. Each decision stores its parent decision so the
//! full causal chain can be reconstructed later.
//!
//! ## Input
//! - `DecisionRecord` values produced by agents and the orchestrator
//! - Decision identifiers for ancestry and descendant queries
//!
//! ## Output
//! - Durable decision rows and reconstructed causal chains
//!
//! ## Related
//! - `forge-core::id` — provides decision and session identifiers
//! - `forge-core::truth` — provides optional truth-token grounding

use std::collections::VecDeque;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use yantra_core::{AgentKind, DecisionId, SessionId, TruthToken};

use crate::error::ObsResult;

/// `SQLite` schema for decision archaeology.
pub const DECISIONS_SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL,
    session_id TEXT NOT NULL,
    parent_decision_id TEXT,
    action_type TEXT NOT NULL,
    reasoning TEXT NOT NULL,
    alternatives_json TEXT,
    truth_token_sig TEXT,
    git_hash TEXT,
    agent TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_decisions_session ON decisions(session_id);
CREATE INDEX IF NOT EXISTS idx_decisions_parent ON decisions(parent_decision_id);";

/// Durable record of an agent or orchestrator decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// Unique decision identifier.
    pub id: DecisionId,
    /// Timestamp at which the decision was made.
    pub timestamp: DateTime<Utc>,
    /// Session containing this decision.
    pub session_id: SessionId,
    /// Parent decision in the causal chain.
    pub parent_decision_id: Option<DecisionId>,
    /// Stable action type label.
    pub action_type: String,
    /// Human-readable reasoning summary.
    pub reasoning: String,
    /// Alternatives considered before this decision.
    pub alternatives_considered: Vec<String>,
    /// Truth token grounding this decision.
    pub truth_token: Option<TruthToken>,
    /// Git hash produced by the decision, when any.
    pub git_hash: Option<String>,
    /// Agent that made the decision.
    pub agent: AgentKind,
}

/// Records one decision in `SQLite`.
///
/// # Errors
///
/// Returns `ObsError` when schema setup, serialization, or insertion fails.
pub fn record_decision(database: &Connection, decision: &DecisionRecord) -> ObsResult<()> {
    database.execute_batch(DECISIONS_SCHEMA_SQL)?;
    database.execute(
        "INSERT OR REPLACE INTO decisions (
            id,
            timestamp,
            session_id,
            parent_decision_id,
            action_type,
            reasoning,
            alternatives_json,
            truth_token_sig,
            git_hash,
            agent
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            decision.id.to_string(),
            decision.timestamp.to_rfc3339(),
            decision.session_id.to_string(),
            decision
                .parent_decision_id
                .map(|decision_id| decision_id.to_string()),
            decision.action_type,
            decision.reasoning,
            serde_json::to_string(&decision.alternatives_considered)?,
            decision
                .truth_token
                .as_ref()
                .map(|truth_token| hex_encode(&truth_token.signature)),
            decision.git_hash,
            serde_json::to_string(&decision.agent)?,
        ],
    )?;
    Ok(())
}

/// Returns a decision's causal chain, beginning with the requested decision.
///
/// # Errors
///
/// Returns `ObsError` when database reads or row reconstruction fail.
pub fn causal_chain(
    database: &Connection,
    decision_id: DecisionId,
) -> ObsResult<Vec<DecisionRecord>> {
    database.execute_batch(DECISIONS_SCHEMA_SQL)?;
    let mut chain = Vec::new();
    let mut current_decision_id = Some(decision_id);

    while let Some(parent_candidate_id) = current_decision_id {
        let decision = query_decision(database, parent_candidate_id)?;
        current_decision_id = decision.parent_decision_id;
        chain.push(decision);
    }

    Ok(chain)
}

/// Returns all descendants of a decision in breadth-first order.
///
/// # Errors
///
/// Returns `ObsError` when database reads or row reconstruction fail.
pub fn descendants(
    database: &Connection,
    decision_id: DecisionId,
) -> ObsResult<Vec<DecisionRecord>> {
    database.execute_batch(DECISIONS_SCHEMA_SQL)?;
    let mut descendants = Vec::new();
    let mut pending_decision_ids = VecDeque::from([decision_id]);

    while let Some(parent_decision_id) = pending_decision_ids.pop_front() {
        for child_decision in query_children(database, parent_decision_id)? {
            pending_decision_ids.push_back(child_decision.id);
            descendants.push(child_decision);
        }
    }

    Ok(descendants)
}

fn query_decision(database: &Connection, decision_id: DecisionId) -> ObsResult<DecisionRecord> {
    let decision = database.query_row(
        "SELECT id, timestamp, session_id, parent_decision_id, action_type,
            reasoning, alternatives_json, git_hash, agent
         FROM decisions WHERE id = ?1",
        [decision_id.to_string()],
        row_to_decision,
    )?;
    Ok(decision)
}

fn query_children(
    database: &Connection,
    parent_decision_id: DecisionId,
) -> ObsResult<Vec<DecisionRecord>> {
    let mut statement = database.prepare(
        "SELECT id, timestamp, session_id, parent_decision_id, action_type,
            reasoning, alternatives_json, git_hash, agent
         FROM decisions WHERE parent_decision_id = ?1 ORDER BY timestamp ASC",
    )?;
    let mapped_rows = statement.query_map([parent_decision_id.to_string()], row_to_decision)?;
    let mut decisions = Vec::new();
    for mapped_row in mapped_rows {
        decisions.push(mapped_row?);
    }
    Ok(decisions)
}

fn row_to_decision(row: &rusqlite::Row<'_>) -> rusqlite::Result<DecisionRecord> {
    let id_text: String = row.get(0)?;
    let timestamp_text: String = row.get(1)?;
    let session_id_text: String = row.get(2)?;
    let parent_decision_id_text: Option<String> = row.get(3)?;
    let alternatives_json: Option<String> = row.get(6)?;
    let agent_json: String = row.get(8)?;

    Ok(DecisionRecord {
        id: parse_rusqlite(&id_text)?,
        timestamp: DateTime::parse_from_rfc3339(&timestamp_text)
            .map(DateTime::<Utc>::from)
            .map_err(to_rusqlite_error)?,
        session_id: parse_rusqlite(&session_id_text)?,
        parent_decision_id: parent_decision_id_text
            .as_deref()
            .map(parse_rusqlite)
            .transpose()?,
        action_type: row.get(4)?,
        reasoning: row.get(5)?,
        alternatives_considered: alternatives_json
            .map(|serialized_value| {
                serde_json::from_str(&serialized_value).map_err(to_rusqlite_error)
            })
            .transpose()?
            .unwrap_or_default(),
        truth_token: None,
        git_hash: row.get(7)?,
        agent: serde_json::from_str(&agent_json).map_err(to_rusqlite_error)?,
    })
}

fn parse_rusqlite<T>(value: &str) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.parse().map_err(to_rusqlite_error)
}

fn to_rusqlite_error<E>(error: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}
