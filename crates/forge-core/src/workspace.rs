//! # Workspace Mode Detection
//!
//! Defines the workspace mode (Greenfield vs. Incremental) and provides
//! a detection method based on the code-review graph (CRG) database.
//!
//! ## Input
//! - `project_root: &Path` — path to the root of the project being worked on
//!
//! ## Output
//! - `WorkspaceMode` — the detected mode of the workspace
//!
//! ## Related
//! - `yantra-core::ProjectRoot` — wrapper around absolute project paths

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Represents whether the current workspace is empty/new or has existing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkspaceMode {
    /// Greenfield mode: workspace is empty or has minimal code.
    Greenfield,
    /// Incremental mode: workspace has an existing codebase.
    Incremental,
}

impl WorkspaceMode {
    /// Detects the workspace mode by inspecting the CRG SQLite database.
    pub fn detect(project_root: &Path) -> Self {
        let crg_database_path = project_root.join(".yantra").join("crg.sqlite");
        if !crg_database_path.exists() {
            return Self::Greenfield;
        }

        if let Ok(database_connection) = rusqlite::Connection::open(&crg_database_path) {
            if let Ok(mut sql_statement) =
                database_connection.prepare("SELECT COUNT(*) FROM symbols")
            {
                if let Ok(symbol_count) =
                    sql_statement.query_row([], |database_row| database_row.get::<_, i64>(0))
                {
                    if symbol_count < 3 {
                        return Self::Greenfield;
                    }
                    return Self::Incremental;
                }
            }
        }

        Self::Greenfield
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_detect_no_db_returns_greenfield() {
        let temporary_directory =
            std::env::temp_dir().join(format!("yantra-core-test-{}", uuid::Uuid::new_v4()));
        assert_eq!(
            WorkspaceMode::detect(&temporary_directory),
            WorkspaceMode::Greenfield
        );
    }

    #[test]
    fn test_detect_empty_db_returns_greenfield() {
        let temporary_directory =
            std::env::temp_dir().join(format!("yantra-core-test-{}", uuid::Uuid::new_v4()));
        let yantra_subdirectory = temporary_directory.join(".yantra");
        fs::create_dir_all(&yantra_subdirectory).unwrap();
        let crg_database_path = yantra_subdirectory.join("crg.sqlite");

        let database_connection = rusqlite::Connection::open(&crg_database_path).unwrap();
        database_connection
            .execute("CREATE TABLE symbols (name TEXT)", [])
            .unwrap();

        assert_eq!(
            WorkspaceMode::detect(&temporary_directory),
            WorkspaceMode::Greenfield
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn test_detect_with_symbols_returns_incremental() {
        let temporary_directory =
            std::env::temp_dir().join(format!("yantra-core-test-{}", uuid::Uuid::new_v4()));
        let yantra_subdirectory = temporary_directory.join(".yantra");
        fs::create_dir_all(&yantra_subdirectory).unwrap();
        let crg_database_path = yantra_subdirectory.join("crg.sqlite");

        let database_connection = rusqlite::Connection::open(&crg_database_path).unwrap();
        database_connection
            .execute("CREATE TABLE symbols (name TEXT)", [])
            .unwrap();
        database_connection
            .execute("INSERT INTO symbols (name) VALUES ('foo')", [])
            .unwrap();
        database_connection
            .execute("INSERT INTO symbols (name) VALUES ('bar')", [])
            .unwrap();
        database_connection
            .execute("INSERT INTO symbols (name) VALUES ('baz')", [])
            .unwrap();

        assert_eq!(
            WorkspaceMode::detect(&temporary_directory),
            WorkspaceMode::Incremental
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }
}
