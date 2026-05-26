//! # forge-memory: Temporal Knowledge Graph (TKG)
//!
//! Maintains subject-predicate-object facts with validity windows. Each fact
//! has a `valid_from` timestamp and a `valid_until` that is set when the fact
//! is superseded. This enables point-in-time historical queries alongside
//! current-state queries.
//!
//! ## Input
//! - `(subject, predicate, object, valid_from)` tuples from agents and the
//!   orchestrator describing evolving codebase or task state
//!
//! ## Output
//! - `Vec<Fact>` for either current state or a historical snapshot
//!
//! ## Related
//! - `forge-crg::schema` — shares the same `graph_facts` table structure
//! - `forge-memory::heartbeat` — periodically prunes expired facts

use std::path::Path;

use chrono::{DateTime, Utc};
use yantra_core::db::{apply_migrations, connection_pool, DbPool};
use yantra_core::TaskId;

use crate::error::{MemoryError, MemoryResult};

const TKG_MIGRATION: &str = "
CREATE TABLE IF NOT EXISTS graph_facts (
    id             TEXT PRIMARY KEY,
    subject        TEXT NOT NULL,
    predicate      TEXT NOT NULL,
    object         TEXT NOT NULL,
    weight         REAL  DEFAULT 1.0,
    valid_from     TEXT  NOT NULL,
    valid_until    TEXT,
    source_session TEXT
);
CREATE INDEX IF NOT EXISTS idx_facts_subject   ON graph_facts(subject);
CREATE INDEX IF NOT EXISTS idx_facts_validity  ON graph_facts(valid_from, valid_until);
";

/// A subject-predicate-object fact with a temporal validity window.
#[derive(Debug, Clone)]
pub struct Fact {
    /// Unique fact identifier.
    pub id: String,
    /// The entity this fact is about (e.g. a file path or symbol name).
    pub subject: String,
    /// The relationship or attribute being asserted.
    pub predicate: String,
    /// The value of the attribute for this subject.
    pub object: String,
    /// Confidence or importance weight.
    pub weight: f64,
    /// When this fact became true.
    pub valid_from: DateTime<Utc>,
    /// When this fact was superseded; `None` means currently true.
    pub valid_until: Option<DateTime<Utc>>,
    /// Session that asserted this fact.
    pub source_session: Option<String>,
}

/// TKG-backed store that maintains bi-temporal facts about codebase state.
pub struct TemporalKnowledgeGraph {
    database_pool: DbPool,
}

impl TemporalKnowledgeGraph {
    /// Opens or creates the TKG database at `db_path`.
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
        apply_migrations(&database_guard, &[TKG_MIGRATION])?;
        drop(database_guard);
        Ok(Self { database_pool })
    }

    /// Asserts a new fact, closing any currently open fact for the same
    /// `(subject, predicate)` pair by setting its `valid_until` timestamp.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database write failure.
    pub fn assert_fact(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        valid_from: DateTime<Utc>,
    ) -> MemoryResult<()> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;

        let valid_from_string = valid_from.to_rfc3339();

        database_guard.execute(
            "UPDATE graph_facts
             SET valid_until = ?1
             WHERE subject = ?2 AND predicate = ?3 AND valid_until IS NULL",
            rusqlite::params![valid_from_string, subject, predicate],
        )?;

        let new_fact_id = TaskId::new().to_string();
        database_guard.execute(
            "INSERT INTO graph_facts (id, subject, predicate, object, valid_from)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![new_fact_id, subject, predicate, object, valid_from_string],
        )?;

        Ok(())
    }

    /// Returns all currently valid facts for `subject`.
    ///
    /// A fact is current when `valid_until IS NULL` — it has not been
    /// superseded by a later `assert_fact` call for the same subject+predicate.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database read failure.
    pub fn query_current(&self, subject: &str) -> MemoryResult<Vec<Fact>> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        query_facts(
            &database_guard,
            "SELECT id, subject, predicate, object, weight, valid_from, valid_until, source_session
             FROM graph_facts
             WHERE subject = ?1 AND valid_until IS NULL",
            rusqlite::params![subject],
        )
    }

    /// Returns all facts for `subject` that were valid at `timestamp`.
    ///
    /// A fact was valid at `timestamp` when
    /// `valid_from <= timestamp AND (valid_until IS NULL OR valid_until > timestamp)`.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database read failure.
    pub fn query_at(&self, subject: &str, timestamp: DateTime<Utc>) -> MemoryResult<Vec<Fact>> {
        let timestamp_string = timestamp.to_rfc3339();
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        query_facts(
            &database_guard,
            "SELECT id, subject, predicate, object, weight, valid_from, valid_until, source_session
             FROM graph_facts
             WHERE subject = ?1
               AND valid_from <= ?2
               AND (valid_until IS NULL OR valid_until > ?2)",
            rusqlite::params![subject, timestamp_string],
        )
    }
}

fn query_facts(
    database: &rusqlite::Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> MemoryResult<Vec<Fact>> {
    let mut statement = database.prepare(sql)?;
    let facts = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?
        .filter_map(|row_result| {
            let (
                id,
                subject,
                predicate,
                object,
                weight,
                valid_from_str,
                valid_until_str,
                source_session,
            ) = row_result.ok()?;
            let valid_from = DateTime::parse_from_rfc3339(&valid_from_str)
                .ok()?
                .with_timezone(&Utc);
            let valid_until = valid_until_str
                .and_then(|timestamp_string| DateTime::parse_from_rfc3339(&timestamp_string).ok())
                .map(|datetime| datetime.with_timezone(&Utc));
            Some(Fact {
                id,
                subject,
                predicate,
                object,
                weight,
                valid_from,
                valid_until,
                source_session,
            })
        })
        .collect();
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use yantra_core::TaskId;

    use super::*;

    fn temp_db_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("yantra-tkg-test-{}", TaskId::new()))
            .join("tkg.sqlite")
    }

    #[test]
    fn assert_fact_and_query_current_round_trip() {
        let tkg = TemporalKnowledgeGraph::new(&temp_db_path()).expect("TKG created");
        let now = Utc::now();

        tkg.assert_fact("src/auth.rs", "owner", "AuthTeam", now)
            .expect("fact asserted");

        let facts = tkg.query_current("src/auth.rs").expect("query succeeds");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].predicate, "owner");
        assert_eq!(facts[0].object, "AuthTeam");
    }

    #[test]
    fn assert_fact_closes_conflicting_current_fact() {
        let tkg = TemporalKnowledgeGraph::new(&temp_db_path()).expect("TKG created");
        let time_a = Utc::now();
        let time_b = time_a + Duration::seconds(1);

        tkg.assert_fact("src/auth.rs", "owner", "TeamAlpha", time_a)
            .expect("first fact asserted");
        tkg.assert_fact("src/auth.rs", "owner", "TeamBeta", time_b)
            .expect("second fact asserted");

        let current_facts = tkg.query_current("src/auth.rs").expect("query succeeds");
        assert_eq!(
            current_facts.len(),
            1,
            "only one current fact for same subject+predicate"
        );
        assert_eq!(current_facts[0].object, "TeamBeta");
    }

    #[test]
    fn query_at_returns_historical_fact() {
        let tkg = TemporalKnowledgeGraph::new(&temp_db_path()).expect("TKG created");
        let time_a = Utc::now();
        let time_b = time_a + Duration::seconds(10);
        let historical_query_time = time_a + Duration::seconds(5);

        tkg.assert_fact("src/lib.rs", "status", "stable", time_a)
            .expect("first fact asserted");
        tkg.assert_fact("src/lib.rs", "status", "breaking", time_b)
            .expect("second fact asserted");

        let historical_facts = tkg
            .query_at("src/lib.rs", historical_query_time)
            .expect("historical query succeeds");
        assert_eq!(historical_facts.len(), 1);
        assert_eq!(
            historical_facts[0].object, "stable",
            "at t=5s the first fact should be active"
        );
    }

    #[test]
    fn no_overlapping_current_facts_for_same_subject_and_predicate() {
        let tkg = TemporalKnowledgeGraph::new(&temp_db_path()).expect("TKG created");
        let base_time = Utc::now();

        for (index, value) in ["v1", "v2", "v3", "v4"].iter().enumerate() {
            let fact_time = base_time + Duration::seconds(index as i64);
            tkg.assert_fact("entity", "version", value, fact_time)
                .expect("fact asserted");
        }

        let current_facts = tkg.query_current("entity").expect("query succeeds");
        let open_facts: Vec<&Fact> = current_facts
            .iter()
            .filter(|fact| fact.valid_until.is_none())
            .collect();
        assert_eq!(
            open_facts.len(),
            1,
            "only one open fact must exist at a time"
        );
        assert_eq!(open_facts[0].object, "v4");
    }
}
