//! # Trace Store Tamper Vulnerability Tests
//!
//! Verifies that the SQLite-backed trace store used by `forge-obs` degrades
//! gracefully when the database file is corrupt, missing expected schema, or
//! contains NULL values in cost columns. Tests exercise the `rusqlite` query
//! layer directly so we catch regressions without depending on the full
//! observability stack.
//!
//! ## Input
//! - In-memory or on-disk SQLite databases with: no schema, NULL cost rows,
//!   garbage file bytes, or a correctly initialised but empty spans table
//!
//! ## Output
//! - Assertions that invalid database states produce errors (not panics),
//!   and that NULL-coalescing queries return zero-cost totals without error.
//!
//! ## Related
//! - `forge-obs` — the production consumer of the spans schema
//! - `rusqlite` — the database driver exercised by these tests

use rusqlite::Connection;

const SPANS_DDL: &str = "
    CREATE TABLE IF NOT EXISTS spans (
        id          TEXT PRIMARY KEY,
        trace_id    TEXT NOT NULL,
        name        TEXT NOT NULL,
        started_at  INTEGER NOT NULL,
        duration_ms INTEGER,
        cost_usd    REAL
    )
";

#[test]
fn test_missing_spans_table_does_not_panic() {
    let temp_dir = tempfile::tempdir().unwrap();
    let database_path = temp_dir.path().join("no-schema.sqlite");

    let database_connection = Connection::open(&database_path).unwrap();

    let query_result =
        database_connection.query_row("SELECT COUNT(*) FROM spans", [], |row| row.get::<_, i64>(0));

    assert!(
        query_result.is_err(),
        "querying a non-existent 'spans' table must return an error, not panic"
    );
}

#[test]
fn test_null_cost_rows_can_be_read_with_coalesce() {
    let database_connection = Connection::open_in_memory().unwrap();
    database_connection.execute_batch(SPANS_DDL).unwrap();

    database_connection
        .execute(
            "INSERT INTO spans (id, trace_id, name, started_at, duration_ms, cost_usd)
             VALUES ('span-1', 'trace-1', 'llm-call', 1717000000, 42, NULL)",
            [],
        )
        .unwrap();

    let coalesced_cost: f64 = database_connection
        .query_row(
            "SELECT COALESCE(cost_usd, 0.0) FROM spans WHERE id = 'span-1'",
            [],
            |row| row.get(0),
        )
        .expect("COALESCE query on NULL cost_usd must succeed without error");

    assert_eq!(
        coalesced_cost, 0.0,
        "COALESCE of a NULL cost_usd must return exactly 0.0"
    );
}

#[test]
fn test_truncated_sqlite_file_returns_error_not_panic() {
    let temp_dir = tempfile::tempdir().unwrap();
    let garbage_file_path = temp_dir.path().join("garbage.sqlite");

    std::fs::write(
        &garbage_file_path,
        b"this is not a sqlite database file \x00\xff",
    )
    .unwrap();

    let open_result = Connection::open(&garbage_file_path);

    match open_result {
        Err(_) => {
            // Connection::open may immediately detect the corruption — expected.
        }
        Ok(broken_connection) => {
            let query_result =
                broken_connection.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| {
                    row.get::<_, i64>(0)
                });
            assert!(
                query_result.is_err(),
                "querying a garbage SQLite file must return an error, not a valid result"
            );
        }
    }
}

#[test]
fn test_empty_spans_table_returns_zero_cost() {
    let database_connection = Connection::open_in_memory().unwrap();
    database_connection.execute_batch(SPANS_DDL).unwrap();

    let total_span_count: i64 = database_connection
        .query_row("SELECT COUNT(*) FROM spans", [], |row| row.get(0))
        .expect("COUNT(*) on an empty spans table must succeed");

    assert_eq!(
        total_span_count, 0,
        "empty spans table must report a count of zero"
    );

    let total_cost: f64 = database_connection
        .query_row(
            "SELECT COALESCE(SUM(cost_usd), 0.0) FROM spans",
            [],
            |row| row.get(0),
        )
        .expect("SUM with COALESCE on empty spans table must succeed");

    assert_eq!(
        total_cost, 0.0,
        "total cost of an empty spans table must be 0.0"
    );
}

#[test]
fn test_multiple_null_and_real_costs_aggregate_correctly() {
    let database_connection = Connection::open_in_memory().unwrap();
    database_connection.execute_batch(SPANS_DDL).unwrap();

    database_connection
        .execute_batch(
            "INSERT INTO spans (id, trace_id, name, started_at, cost_usd) VALUES
             ('s1', 't1', 'call-a', 1, 0.001),
             ('s2', 't1', 'call-b', 2, NULL),
             ('s3', 't1', 'call-c', 3, 0.002),
             ('s4', 't1', 'call-d', 4, NULL)",
        )
        .unwrap();

    let aggregate_cost: f64 = database_connection
        .query_row(
            "SELECT COALESCE(SUM(cost_usd), 0.0) FROM spans",
            [],
            |row| row.get(0),
        )
        .expect("SUM with NULL rows must succeed via COALESCE");

    assert!(
        (aggregate_cost - 0.003).abs() < 1e-9,
        "aggregate cost must be 0.003 (NULL rows treated as zero); got {aggregate_cost}"
    );
}
