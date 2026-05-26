//! # forge-core: `SQLite` Helpers
//!
//! Provides minimal `SQLite` connection setup shared by crates that persist
//! Yantra runtime state. Connections are opened with WAL mode and migrations
//! are applied idempotently through a version table.
//!
//! ## Input
//! - `SQLite` database file paths
//! - Ordered SQL migration strings
//!
//! ## Output
//! - `DbPool` handles and applied migration schemas
//!
//! ## Related
//! - `forge-core::error` — wraps `SQLite` failures

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::error::CoreResult;

/// SQL used to create the migration version table.
pub const MIGRATION_VERSION_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
)";

/// Shared `SQLite` connection handle.
pub type DbPool = Arc<Mutex<Connection>>;

/// Opens a `SQLite` database and configures it for WAL-mode access.
///
/// # Errors
///
/// Returns `CoreError::Database` if the database cannot be opened or if WAL
/// and foreign-key pragmas cannot be applied.
pub fn connection_pool(path: &Path) -> CoreResult<DbPool> {
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(Arc::new(Mutex::new(connection)))
}

/// Applies ordered SQL migrations exactly once.
///
/// # Errors
///
/// Returns `CoreError::Database` if the migration version table cannot be
/// created, a migration cannot be checked, or migration SQL cannot be applied.
pub fn apply_migrations(database: &Connection, migrations: &[&str]) -> CoreResult<()> {
    database.execute_batch(MIGRATION_VERSION_TABLE_SQL)?;

    for (migration_index, migration_sql) in migrations.iter().enumerate() {
        let migration_version = i64::try_from(migration_index + 1).unwrap_or(i64::MAX);
        let already_applied = migration_already_applied(database, migration_version)?;
        if !already_applied {
            database.execute_batch(migration_sql)?;
            database.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [migration_version],
            )?;
        }
    }

    Ok(())
}

fn migration_already_applied(database: &Connection, migration_version: i64) -> CoreResult<bool> {
    let applied_count: i64 = database.query_row(
        "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
        [migration_version],
        |row| row.get(0),
    )?;
    Ok(applied_count > 0)
}
