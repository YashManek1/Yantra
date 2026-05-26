//! # forge-obs: Tracing Initialization
//!
//! Initializes `tracing-subscriber` with environment filtering, optional
//! pretty stderr output, and a custom `SQLite` span layer. The custom layer
//! records every closed tracing span as a durable `forge-core::Span`.
//!
//! ## Input
//! - Tracing configuration at process startup
//! - Runtime tracing spans emitted by other crates
//!
//! ## Output
//! - Global tracing subscriber and durable rows in `.yantra/traces.sqlite`
//!
//! ## Related
//! - `forge-obs::traces` — persists closed spans
//! - `forge-core::span` — provides the durable span model

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::Connection;
use tracing_subscriber::field::Visit;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{fmt, EnvFilter, Layer};
use yantra_core::{ErrorRecord, ModelId, Outcome, ProjectRoot, SessionId, Span, SpanId, TaskId};

use crate::error::{ObsError, ObsResult};
use crate::traces::record_span;

/// Startup configuration for observability tracing.
#[derive(Debug, Clone)]
pub struct TracingConfig {
    /// Project root used to locate `.yantra/traces.sqlite`.
    pub project_root: ProjectRoot,
    /// Session identifier attached to spans without an explicit field.
    pub session_id: SessionId,
    /// Environment filter directive, such as `info` or `yantra=debug`.
    pub env_filter: String,
    /// Enables human-readable stderr output when set.
    pub pretty_stderr: bool,
}

/// Guard returned after tracing initialization.
#[derive(Debug)]
pub struct TracingGuard {
    traces_path: PathBuf,
}

impl TracingGuard {
    /// Returns the `SQLite` trace database path.
    pub fn traces_path(&self) -> &PathBuf {
        &self.traces_path
    }
}

/// Custom tracing layer that writes closed spans to `SQLite`.
#[derive(Debug, Clone)]
pub struct SqliteSpanLayer {
    database: std::sync::Arc<Mutex<Connection>>,
    session_id: SessionId,
}

impl SqliteSpanLayer {
    /// Creates a span layer backed by a `SQLite` database path.
    ///
    /// # Errors
    ///
    /// Returns `ObsError::Database` when the database cannot be opened or
    /// trace schema setup fails.
    pub fn new(traces_path: PathBuf, session_id: SessionId) -> ObsResult<Self> {
        if let Some(parent_directory) = traces_path.parent() {
            std::fs::create_dir_all(parent_directory).map_err(|source| {
                ObsError::Core(yantra_core::CoreError::PathIo {
                    path: parent_directory.to_path_buf(),
                    source,
                })
            })?;
        }
        let connection = Connection::open(traces_path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        Ok(Self {
            database: std::sync::Arc::new(Mutex::new(connection)),
            session_id,
        })
    }
}

/// Initializes global tracing once at startup.
///
/// # Errors
///
/// Returns `ObsError` if the trace database cannot be opened or a global
/// subscriber has already been installed.
pub fn init_tracing(config: TracingConfig) -> ObsResult<TracingGuard> {
    let traces_path = config
        .project_root
        .as_path()
        .join(".yantra")
        .join("traces.sqlite");
    let sqlite_layer = SqliteSpanLayer::new(traces_path.clone(), config.session_id)?;
    let env_filter =
        EnvFilter::try_new(config.env_filter).unwrap_or_else(|_| EnvFilter::new("info"));

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(sqlite_layer)
        .with(config.pretty_stderr.then(|| fmt::layer().pretty()));

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| ObsError::TracingAlreadyInitialized)?;

    Ok(TracingGuard { traces_path })
}

impl<S> Layer<S> for SqliteSpanLayer
where
    S: tracing::Subscriber,
    S: for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        span_id: &tracing::Id,
        context: Context<'_, S>,
    ) {
        if let Some(span_reference) = context.span(span_id) {
            let mut visitor = SpanFieldVisitor::default();
            attributes.record(&mut visitor);
            let parent_id = attributes
                .parent()
                .and_then(|parent_span_id| context.span(parent_span_id))
                .and_then(|parent_span| {
                    parent_span
                        .extensions()
                        .get::<ObservedSpan>()
                        .map(|observed_span| observed_span.span_id)
                });

            span_reference.extensions_mut().insert(ObservedSpan {
                span_id: visitor
                    .fields
                    .get("span_id")
                    .and_then(|value| SpanId::from_str(value).ok())
                    .unwrap_or_default(),
                parent_id,
                started_at: Utc::now(),
                started_instant: Instant::now(),
                fields: visitor.fields,
            });
        }
    }

    fn on_close(&self, span_id: tracing::Id, context: Context<'_, S>) {
        let Some(span_reference) = context.span(&span_id) else {
            return;
        };
        let extensions = span_reference.extensions();
        let Some(observed_span) = extensions.get::<ObservedSpan>() else {
            return;
        };

        let span = observed_span.to_span(self.session_id);
        let database = self.database.lock();
        let _record_result = record_span(&database, &span);
    }
}

#[derive(Debug)]
struct ObservedSpan {
    span_id: SpanId,
    parent_id: Option<SpanId>,
    started_at: DateTime<Utc>,
    started_instant: Instant,
    fields: BTreeMap<String, String>,
}

impl ObservedSpan {
    fn to_span(&self, default_session_id: SessionId) -> Span {
        let session_id = self
            .fields
            .get("session_id")
            .and_then(|value| SessionId::from_str(value).ok())
            .unwrap_or(default_session_id);
        let outcome = self
            .fields
            .get("outcome")
            .and_then(|value| parse_json_or_debug(value).ok())
            .unwrap_or(Outcome::Success);

        Span {
            span_id: self.span_id,
            parent_id: self.parent_id,
            session_id,
            task_id: self
                .fields
                .get("task_id")
                .and_then(|value| TaskId::from_str(value).ok()),
            truth_token: None,
            agent: self
                .fields
                .get("agent")
                .and_then(|value| parse_json_or_debug(value).ok()),
            model: self
                .fields
                .get("model")
                .and_then(|value| ModelId::new(value.clone()).ok()),
            tokens_in: self
                .fields
                .get("tokens_in")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            tokens_out: self
                .fields
                .get("tokens_out")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            cost_usd: self
                .fields
                .get("cost_usd")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            duration_ms: u64::try_from(self.started_instant.elapsed().as_millis())
                .unwrap_or(u64::MAX),
            started_at: self.started_at,
            outcome,
            error: self.fields.get("error_message").map(|message| ErrorRecord {
                kind: self
                    .fields
                    .get("error_kind")
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_string()),
                message: message.clone(),
                source_span: None,
            }),
        }
    }
}

#[derive(Debug, Default)]
struct SpanFieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for SpanFieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.fields.insert(
            field.name().to_string(),
            format!("{value:?}").trim_matches('"').to_string(),
        );
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

fn parse_json_or_debug<T>(value: &str) -> serde_json::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(value).or_else(|_| serde_json::from_str(&format!("\"{value}\"")))
}
