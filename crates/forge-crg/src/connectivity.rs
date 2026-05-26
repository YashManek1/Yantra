//! # Code-Review Graph: Connectivity Scoring
//!
//! Calculates connectivity scores for symbols in the Code-Review Graph. The score
//! represents:
//! - Incoming CALLS edges
//! - Outgoing CALLS edges
//! - Incoming IMPORTS edges referencing the symbol directly
//! - Incoming IMPORTS edges referencing the file containing the symbol
//!
//! ## Input
//! - Active `SQLite` database connection
//! - Target symbol identifier
//!
//! ## Output
//! - Combined connectivity score as an i32
//!
//! ## Related
//! - `forge-crg::builder` — triggers connectivity recalculation and persists results

use rusqlite::Connection;
use yantra_core::SymbolId;

pub fn compute_connectivity(connection: &Connection, symbol_id: &SymbolId) -> i32 {
    let symbol_string = symbol_id.to_string();

    let is_test: bool = connection
        .query_row(
            "SELECT s.name, f.path FROM symbols s JOIN files f ON s.file_id = f.id WHERE s.id = ?1",
            [&symbol_string],
            |row| {
                let name: String = row.get(0)?;
                let path: String = row.get(1)?;
                let name_lower = name.to_lowercase();
                let is_test_name = name_lower.starts_with("test_")
                    || name_lower.contains("_test_")
                    || name_lower == "test"
                    || name_lower.ends_with("_test");
                let file_name = path.split(['/', '\\']).next_back().unwrap_or("");
                let file_name_lower = file_name.to_lowercase();
                let is_test_file_name = file_name_lower.starts_with("test_")
                    || file_name_lower.contains("_test.")
                    || file_name_lower.contains(".test.");
                let path_has_test_segment = path.split(['/', '\\']).any(|segment| {
                    let segment_lower = segment.to_lowercase();
                    segment_lower == "tests"
                        || segment_lower == "benches"
                        || segment_lower == "fixtures"
                        || segment_lower == "examples"
                });
                Ok(is_test_name || is_test_file_name || path_has_test_segment)
            },
        )
        .unwrap_or_else(|_| {
            connection
                .query_row(
                    "SELECT file_id FROM symbols WHERE id = ?1",
                    [&symbol_string],
                    |row| {
                        let file_id: String = row.get(0)?;
                        let file_name = file_id.split(['/', '\\']).next_back().unwrap_or("");
                        let file_name_lower = file_name.to_lowercase();
                        let is_test_file_name = file_name_lower.starts_with("test_")
                            || file_name_lower.contains("_test.")
                            || file_name_lower.contains(".test.");
                        let path_has_test_segment = file_id.split(['/', '\\']).any(|segment| {
                            let segment_lower = segment.to_lowercase();
                            segment_lower == "tests"
                                || segment_lower == "benches"
                                || segment_lower == "fixtures"
                                || segment_lower == "examples"
                        });
                        Ok(is_test_file_name || path_has_test_segment)
                    },
                )
                .unwrap_or(false)
        });

    let calls_in_count: i32 = connection
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE to_id = ?1 AND edge_type = 'CALLS'",
            [&symbol_string],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let calls_out_count: i32 = connection
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_id = ?1 AND edge_type = 'CALLS'",
            [&symbol_string],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let direct_imports_count: i32 = connection
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE to_id = ?1 AND edge_type = 'IMPORTS'",
            [&symbol_string],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let file_imports_count: i32 = connection.query_row(
        "SELECT COUNT(*) FROM edges WHERE to_id = (SELECT file_id FROM symbols WHERE id = ?1) AND edge_type = 'IMPORTS'",
        [&symbol_string],
        |row| row.get(0),
    ).unwrap_or(0);

    if is_test {
        calls_in_count + direct_imports_count
    } else {
        calls_in_count * 3 + direct_imports_count * 2 + file_imports_count + calls_out_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_compute_connectivity() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "
            CREATE TABLE symbols (
                id TEXT PRIMARY KEY,
                file_id TEXT NOT NULL
            );
            CREATE TABLE edges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                from_id TEXT NOT NULL,
                to_id TEXT NOT NULL,
                edge_type TEXT NOT NULL,
                weight INTEGER DEFAULT 1
            );
        ",
            )
            .unwrap();

        let symbol_a = SymbolId::from_parts("file.rs", "FuncA", "Function");
        let symbol_b = SymbolId::from_parts("file.rs", "FuncB", "Function");
        let symbol_c = SymbolId::from_parts("file.rs", "FuncC", "Function");
        let file_a_id = SymbolId::from_parts("file.rs", "file", "file").to_string();

        connection
            .execute(
                "INSERT INTO symbols (id, file_id) VALUES (?1, ?2)",
                [symbol_a.to_string(), file_a_id.clone()],
            )
            .unwrap();

        // 1. Direct call edges: A -> B (CALLS), C -> A (CALLS)
        connection
            .execute(
                "INSERT INTO edges (from_id, to_id, edge_type) VALUES (?1, ?2, 'CALLS')",
                [symbol_a.to_string(), symbol_b.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO edges (from_id, to_id, edge_type) VALUES (?1, ?2, 'CALLS')",
                [symbol_c.to_string(), symbol_a.to_string()],
            )
            .unwrap();

        // 2. Import edges: importing_file -> A (IMPORTS), importing_file -> file_a (IMPORTS)
        connection
            .execute(
                "INSERT INTO edges (from_id, to_id, edge_type) VALUES (?1, ?2, 'IMPORTS')",
                ["import_file.rs".to_string(), symbol_a.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO edges (from_id, to_id, edge_type) VALUES (?1, ?2, 'IMPORTS')",
                ["import_file.rs".to_string(), file_a_id.clone()],
            )
            .unwrap();

        // Degree of A should be 7:
        // - 1 incoming CALLS (C -> A) => 1 * 3 = 3
        // - 1 direct incoming IMPORTS (import_file -> A) => 1 * 2 = 2
        // - 1 file-level incoming IMPORTS (import_file -> file_a) => 1
        // - 1 outgoing CALLS (A -> B) => 1
        assert_eq!(compute_connectivity(&connection, &symbol_a), 7);

        let symbol_test = SymbolId::from_parts("file_test.rs", "test_func", "Function");
        connection
            .execute(
                "INSERT INTO symbols (id, file_id) VALUES (?1, ?2)",
                [symbol_test.to_string(), "file_test.rs".to_string()],
            )
            .unwrap();

        connection
            .execute(
                "INSERT INTO edges (from_id, to_id, edge_type) VALUES (?1, ?2, 'CALLS')",
                [symbol_c.to_string(), symbol_test.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO edges (from_id, to_id, edge_type) VALUES (?1, ?2, 'CALLS')",
                [symbol_test.to_string(), symbol_b.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO edges (from_id, to_id, edge_type) VALUES (?1, ?2, 'IMPORTS')",
                ["import_file.rs".to_string(), symbol_test.to_string()],
            )
            .unwrap();

        // For a test symbol, score should be calls_in_count (1) + direct_imports_count (1) = 2.
        assert_eq!(compute_connectivity(&connection, &symbol_test), 2);
    }
}
