//! # forge-obs: Trace Storage
//!
//! Stores and retrieves observability spans in `SQLite`. The schema mirrors the
//! shared `forge-core::Span` structure while keeping truth-token persistence
//! limited to the token signature for query efficiency.
//!
//! ## Input
//! - `Span` records from runtime instrumentation
//! - Session and task identifiers for trace queries
//!
//! ## Output
//! - Durable span rows and reconstructed `Span` records
//!
//! ## Related
//! - `forge-core::span` — defines the span data model
//! - `forge-obs::tracing` — writes span endings through this module

use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use yantra_core::{ErrorRecord, ModelId, SessionId, Span, TaskId};

use crate::error::ObsResult;

/// `SQLite` schema for span trace storage.
pub const TRACES_SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS spans (
    span_id TEXT PRIMARY KEY,
    parent_id TEXT,
    session_id TEXT NOT NULL,
    task_id TEXT,
    truth_token_sig TEXT,
    agent TEXT,
    model TEXT,
    tokens_in INTEGER,
    tokens_out INTEGER,
    cost_usd REAL,
    duration_ms INTEGER,
    started_at TEXT NOT NULL,
    outcome TEXT NOT NULL,
    error_kind TEXT,
    error_message TEXT
);
CREATE INDEX IF NOT EXISTS idx_spans_session ON spans(session_id);
CREATE INDEX IF NOT EXISTS idx_spans_task ON spans(task_id);
CREATE INDEX IF NOT EXISTS idx_spans_started ON spans(started_at);";

/// Records a span in `SQLite`.
///
/// # Errors
///
/// Returns `ObsError` when schema setup, serialization, or insertion fails.
pub fn record_span(database: &Connection, span: &Span) -> ObsResult<()> {
    database.execute_batch(TRACES_SCHEMA_SQL)?;
    database.execute(
        "INSERT OR REPLACE INTO spans (
            span_id,
            parent_id,
            session_id,
            task_id,
            truth_token_sig,
            agent,
            model,
            tokens_in,
            tokens_out,
            cost_usd,
            duration_ms,
            started_at,
            outcome,
            error_kind,
            error_message
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            span.span_id.to_string(),
            span.parent_id.map(|span_id| span_id.to_string()),
            span.session_id.to_string(),
            span.task_id.map(|task_id| task_id.to_string()),
            span.truth_token
                .as_ref()
                .map(|truth_token| hex_encode(&truth_token.signature)),
            serialize_optional(span.agent.as_ref())?,
            span.model.as_ref().map(ToString::to_string),
            i64::try_from(span.tokens_in).unwrap_or(i64::MAX),
            i64::try_from(span.tokens_out).unwrap_or(i64::MAX),
            span.cost_usd,
            i64::try_from(span.duration_ms).unwrap_or(i64::MAX),
            span.started_at.to_rfc3339(),
            serde_json::to_string(&span.outcome)?,
            span.error
                .as_ref()
                .map(|error_record| error_record.kind.clone()),
            span.error
                .as_ref()
                .map(|error_record| error_record.message.clone()),
        ],
    )?;
    Ok(())
}

/// Queries spans belonging to a session.
///
/// # Errors
///
/// Returns `ObsError` when the query fails or stored identifiers cannot parse.
pub fn query_session_spans(database: &Connection, session_id: SessionId) -> ObsResult<Vec<Span>> {
    query_spans(
        database,
        "SELECT span_id, parent_id, session_id, task_id, agent, model, tokens_in,
            tokens_out, cost_usd, duration_ms, started_at, outcome, error_kind, error_message
         FROM spans WHERE session_id = ?1 ORDER BY started_at ASC",
        session_id.to_string(),
    )
}

/// Queries spans belonging to a task.
///
/// # Errors
///
/// Returns `ObsError` when the query fails or stored identifiers cannot parse.
pub fn query_task_spans(database: &Connection, task_id: TaskId) -> ObsResult<Vec<Span>> {
    query_spans(
        database,
        "SELECT span_id, parent_id, session_id, task_id, agent, model, tokens_in,
            tokens_out, cost_usd, duration_ms, started_at, outcome, error_kind, error_message
         FROM spans WHERE task_id = ?1 ORDER BY started_at ASC",
        task_id.to_string(),
    )
}

fn query_spans(database: &Connection, sql: &str, identifier: String) -> ObsResult<Vec<Span>> {
    database.execute_batch(TRACES_SCHEMA_SQL)?;
    let mut statement = database.prepare(sql)?;
    let mapped_rows = statement.query_map([identifier], row_to_span)?;
    let mut spans = Vec::new();
    for mapped_row in mapped_rows {
        spans.push(mapped_row?);
    }
    Ok(spans)
}

fn row_to_span(row: &rusqlite::Row<'_>) -> rusqlite::Result<Span> {
    let span_id_text: String = row.get(0)?;
    let parent_id_text: Option<String> = row.get(1)?;
    let session_id_text: String = row.get(2)?;
    let task_id_text: Option<String> = row.get(3)?;
    let agent_text: Option<String> = row.get(4)?;
    let model_text: Option<String> = row.get(5)?;
    let tokens_in: i64 = row.get(6)?;
    let tokens_out: i64 = row.get(7)?;
    let cost_usd: f64 = row.get(8)?;
    let duration_ms: i64 = row.get(9)?;
    let started_at_text: String = row.get(10)?;
    let outcome_text: String = row.get(11)?;
    let error_kind: Option<String> = row.get(12)?;
    let error_message: Option<String> = row.get(13)?;

    let span = Span {
        span_id: parse_rusqlite(&span_id_text)?,
        parent_id: parse_optional_rusqlite(parent_id_text.as_deref())?,
        session_id: parse_rusqlite(&session_id_text)?,
        task_id: parse_optional_rusqlite(task_id_text.as_deref())?,
        truth_token: None,
        agent: deserialize_optional_rusqlite(agent_text)?,
        model: model_text
            .map(|model_identifier| ModelId::new(model_identifier).map_err(to_rusqlite_error))
            .transpose()?,
        tokens_in: u64::try_from(tokens_in).unwrap_or_default(),
        tokens_out: u64::try_from(tokens_out).unwrap_or_default(),
        cost_usd,
        duration_ms: u64::try_from(duration_ms).unwrap_or_default(),
        started_at: parse_timestamp_rusqlite(&started_at_text)?,
        outcome: serde_json::from_str(&outcome_text).map_err(to_rusqlite_error)?,
        error: error_kind.map(|kind| ErrorRecord {
            kind,
            message: error_message.unwrap_or_default(),
            source_span: None,
        }),
    };
    Ok(span)
}

fn serialize_optional<T>(value: Option<&T>) -> ObsResult<Option<String>>
where
    T: serde::Serialize,
{
    value
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

fn deserialize_optional_rusqlite<T>(value: Option<String>) -> rusqlite::Result<Option<T>>
where
    T: serde::de::DeserializeOwned,
{
    value
        .map(|serialized_value| serde_json::from_str(&serialized_value).map_err(to_rusqlite_error))
        .transpose()
}

fn parse_optional_rusqlite<T>(value: Option<&str>) -> rusqlite::Result<Option<T>>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.map(parse_rusqlite).transpose()
}

fn parse_rusqlite<T>(value: &str) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.parse().map_err(to_rusqlite_error)
}

fn parse_timestamp_rusqlite(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(DateTime::<Utc>::from)
        .map_err(to_rusqlite_error)
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
