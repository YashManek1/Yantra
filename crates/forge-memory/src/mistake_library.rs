//! # forge-memory: Mistake Library
//!
//! Records past agent failures and converts them to prevention rules that are
//! injected into future prompts for the same task class. The `times_referenced`
//! counter gives visibility into which rules are most frequently consulted.
//!
//! ## Input
//! - `MistakeRecord` values written by agents or the Boolean Exit Gate when a
//!   task fails post-verification
//!
//! ## Output
//! - `Vec<MistakeRecord>` filtered by `TaskClass` for prompt injection
//!
//! ## Related
//! - `forge-memory::working` — injects prevention rules into the assembled prompt
//! - `forge-orchestrator` — writes failure records on task halt

use std::path::Path;

use chrono::{DateTime, Utc};
use yantra_core::db::{apply_migrations, connection_pool, DbPool};
use yantra_core::{TaskClass, TaskId};

use crate::error::{MemoryError, MemoryResult};

const MISTAKE_MIGRATION: &str = "
CREATE TABLE IF NOT EXISTS mistake_records (
    id               TEXT PRIMARY KEY,
    task_class       TEXT NOT NULL,
    failure_mode     TEXT NOT NULL,
    root_cause       TEXT NOT NULL,
    prevention_rule  TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    times_referenced INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_mistakes_class ON mistake_records(task_class);
";

/// A recorded failure with a derived prevention rule.
#[derive(Debug, Clone)]
pub struct MistakeRecord {
    /// Unique mistake identifier (UUID).
    pub id: String,
    /// Class of task that produced this failure.
    pub task_class: TaskClass,
    /// Short description of how the failure manifested.
    pub failure_mode: String,
    /// Diagnosed underlying cause.
    pub root_cause: String,
    /// Actionable rule to inject into future prompts.
    pub prevention_rule: String,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// How many times this rule has been returned by `query_for_task_class`.
    pub times_referenced: i64,
}

impl MistakeRecord {
    /// Creates a new `MistakeRecord` with a generated ID and current timestamp.
    pub fn new(
        task_class: TaskClass,
        failure_mode: impl Into<String>,
        root_cause: impl Into<String>,
        prevention_rule: impl Into<String>,
    ) -> Self {
        Self {
            id: TaskId::new().to_string(),
            task_class,
            failure_mode: failure_mode.into(),
            root_cause: root_cause.into(),
            prevention_rule: prevention_rule.into(),
            created_at: Utc::now(),
            times_referenced: 0,
        }
    }
}

/// Persistent store for failure records and prevention rules.
pub struct MistakeLibraryStore {
    database_pool: DbPool,
}

impl MistakeLibraryStore {
    /// Opens or creates the mistake library database at `db_path`.
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
        apply_migrations(&database_guard, &[MISTAKE_MIGRATION])?;
        drop(database_guard);
        Ok(Self { database_pool })
    }

    /// Persists a new failure record.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database write failure.
    pub fn record_failure(&self, record: &MistakeRecord) -> MemoryResult<()> {
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;
        database_guard.execute(
            "INSERT OR REPLACE INTO mistake_records
             (id, task_class, failure_mode, root_cause, prevention_rule, created_at, times_referenced)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                record.id,
                task_class_to_str(record.task_class),
                record.failure_mode,
                record.root_cause,
                record.prevention_rule,
                record.created_at.to_rfc3339(),
                record.times_referenced,
            ],
        )?;
        Ok(())
    }

    /// Returns all prevention rules for `task_class` and increments each
    /// record's `times_referenced` counter.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` on database failure.
    pub fn query_for_task_class(&self, task_class: TaskClass) -> MemoryResult<Vec<MistakeRecord>> {
        let task_class_string = task_class_to_str(task_class).to_owned();
        let database_guard = self
            .database_pool
            .lock()
            .map_err(|_| MemoryError::LockPoisoned)?;

        let mut statement = database_guard.prepare(
            "SELECT id, task_class, failure_mode, root_cause, prevention_rule,
                    created_at, times_referenced
             FROM mistake_records
             WHERE task_class = ?1
             ORDER BY times_referenced DESC",
        )?;

        let records: Vec<MistakeRecord> = statement
            .query_map(rusqlite::params![task_class_string], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .filter_map(|row_result| {
                let (
                    id,
                    task_class_str,
                    failure_mode,
                    root_cause,
                    prevention_rule,
                    created_at_str,
                    times_referenced,
                ) = row_result.ok()?;
                let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                    .ok()?
                    .with_timezone(&Utc);
                Some(MistakeRecord {
                    id,
                    task_class: task_class_from_str(&task_class_str),
                    failure_mode,
                    root_cause,
                    prevention_rule,
                    created_at,
                    times_referenced,
                })
            })
            .collect();

        let referenced_ids: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
        for record_id in &referenced_ids {
            database_guard.execute(
                "UPDATE mistake_records SET times_referenced = times_referenced + 1 WHERE id = ?1",
                rusqlite::params![record_id],
            )?;
        }

        Ok(records)
    }
}

fn task_class_to_str(task_class: TaskClass) -> &'static str {
    match task_class {
        TaskClass::NewFeature => "NewFeature",
        TaskClass::BugFix => "BugFix",
        TaskClass::Refactor => "Refactor",
        TaskClass::Migration => "Migration",
        TaskClass::Integration => "Integration",
        TaskClass::Exploration => "Exploration",
        TaskClass::Chore => "Chore",
        TaskClass::Docstring => "Docstring",
        TaskClass::Style => "Style",
    }
}

fn task_class_from_str(value: &str) -> TaskClass {
    match value {
        "BugFix" => TaskClass::BugFix,
        "Refactor" => TaskClass::Refactor,
        "Migration" => TaskClass::Migration,
        "Integration" => TaskClass::Integration,
        "Exploration" => TaskClass::Exploration,
        "Chore" => TaskClass::Chore,
        "Docstring" => TaskClass::Docstring,
        "Style" => TaskClass::Style,
        _ => TaskClass::NewFeature,
    }
}

#[cfg(test)]
mod tests {
    use yantra_core::{TaskClass, TaskId};

    use super::*;

    fn temp_db_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("yantra-mistakes-test-{}", TaskId::new()))
            .join("mistakes.sqlite")
    }

    #[test]
    fn record_and_query_failure_round_trip() {
        let library = MistakeLibraryStore::new(&temp_db_path()).expect("store created");
        let record = MistakeRecord::new(
            TaskClass::BugFix,
            "off-by-one in pagination",
            "loop range was 0..n instead of 0..n-1",
            "always verify boundary values with a unit test returning count == expected",
        );
        library.record_failure(&record).expect("record stored");

        let retrieved_records = library
            .query_for_task_class(TaskClass::BugFix)
            .expect("query succeeds");
        assert_eq!(retrieved_records.len(), 1);
        assert!(retrieved_records[0].prevention_rule.contains("boundary"));
    }

    #[test]
    fn query_for_task_class_only_returns_matching_class() {
        let library = MistakeLibraryStore::new(&temp_db_path()).expect("store created");
        library
            .record_failure(&MistakeRecord::new(
                TaskClass::BugFix,
                "null dereference",
                "missing Option check",
                "always unwrap with ? or match",
            ))
            .expect("bug fix record stored");
        library
            .record_failure(&MistakeRecord::new(
                TaskClass::NewFeature,
                "missing test coverage",
                "skipped edge cases",
                "write tests before implementation",
            ))
            .expect("new feature record stored");

        let bug_fix_rules = library
            .query_for_task_class(TaskClass::BugFix)
            .expect("query succeeds");
        assert_eq!(bug_fix_rules.len(), 1);
        assert!(bug_fix_rules[0].failure_mode.contains("null"));

        let feature_rules = library
            .query_for_task_class(TaskClass::NewFeature)
            .expect("query succeeds");
        assert_eq!(feature_rules.len(), 1);
    }

    #[test]
    fn times_referenced_increments_on_each_query() {
        let library = MistakeLibraryStore::new(&temp_db_path()).expect("store created");
        let record = MistakeRecord::new(
            TaskClass::Refactor,
            "broke existing tests",
            "changed public API without updating callers",
            "run cargo test before and after refactor",
        );
        library.record_failure(&record).expect("record stored");

        library
            .query_for_task_class(TaskClass::Refactor)
            .expect("first query");
        library
            .query_for_task_class(TaskClass::Refactor)
            .expect("second query");
        let after_two_queries = library
            .query_for_task_class(TaskClass::Refactor)
            .expect("third query");

        assert_eq!(
            after_two_queries[0].times_referenced, 2,
            "counter should reflect queries before this one"
        );
    }
}
