//! # forge-obs: Telemetry Audit Binary
//!
//! Reads `.yantra/traces.sqlite` and produces a per-span field-coverage report
//! so developers can identify spans missing required telemetry fields.
//!
//! ## Input
//! - Path to `traces.sqlite` (CLI argument, defaults to `.yantra/traces.sqlite`)
//! - The `spans` table schema as defined by `forge-obs::traces`
//!
//! ## Output
//! - A formatted report printed to `stdout` listing system spans, agent spans,
//!   and a summary of missing fields per span
//! - Exit code 0 if all spans are fully populated, 1 if any spans have issues
//!
//! ## Related
//! - `forge-obs::traces` — defines the `spans` table schema
//! - `forge-obs::error`  — error types for the obs crate

use std::path::{Path, PathBuf};
use std::process;

use rusqlite::{Connection, OpenFlags};

struct SpanRow {
    span_id: String,
    session_id: String,
    agent: Option<String>,
    model: Option<String>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    cost_usd: Option<f64>,
    duration_ms: Option<i64>,
    started_at: String,
    outcome: Option<String>,
}

fn resolve_database_path() -> PathBuf {
    std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(".yantra/traces.sqlite"), PathBuf::from)
}

fn open_database(database_path: &Path) -> Result<Connection, String> {
    if !database_path.exists() {
        return Err(format!(
            "Error: traces database not found at {}\nHint: run 'yantra run <task>' first to generate telemetry.",
            database_path.display()
        ));
    }

    Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(
        |open_error| {
            format!(
                "Error: failed to open traces database at {}: {open_error}",
                database_path.display(),
            )
        },
    )
}

fn table_exists(database_connection: &Connection, table_name: &str) -> bool {
    let row_count: i64 = database_connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table_name],
            |row| row.get(0),
        )
        .unwrap_or(0);
    row_count > 0
}

fn load_all_spans(database_connection: &Connection) -> Result<Vec<SpanRow>, String> {
    let mut statement = database_connection
        .prepare(
            "SELECT span_id, session_id, agent, model, tokens_in, tokens_out, \
             cost_usd, duration_ms, started_at, outcome \
             FROM spans ORDER BY started_at ASC",
        )
        .map_err(|prepare_error| format!("Failed to prepare spans query: {prepare_error}"))?;

    let span_row_iter = statement
        .query_map([], |row| {
            Ok(SpanRow {
                span_id: row.get(0)?,
                session_id: row.get(1)?,
                agent: row.get(2)?,
                model: row.get(3)?,
                tokens_in: row.get(4)?,
                tokens_out: row.get(5)?,
                cost_usd: row.get(6)?,
                duration_ms: row.get(7)?,
                started_at: row.get(8)?,
                outcome: row.get(9)?,
            })
        })
        .map_err(|query_error| format!("Failed to query spans: {query_error}"))?;

    let mut loaded_spans = Vec::new();
    for mapped_row in span_row_iter {
        let span_row =
            mapped_row.map_err(|row_error| format!("Failed to read span row: {row_error}"))?;
        loaded_spans.push(span_row);
    }
    Ok(loaded_spans)
}

fn collect_missing_fields(span_row: &SpanRow) -> Vec<&'static str> {
    let mut missing_fields: Vec<&'static str> = Vec::new();

    if span_row.outcome.is_none() {
        missing_fields.push("outcome");
    }

    let duration_is_invalid = span_row
        .duration_ms
        .is_none_or(|duration_value| duration_value <= 0);
    if duration_is_invalid {
        missing_fields.push("duration_ms");
    }

    let is_agent_span = span_row.agent.is_some();
    let has_token_input = span_row
        .tokens_in
        .is_some_and(|token_count| token_count > 0);

    if is_agent_span || has_token_input {
        if span_row.model.is_none() {
            missing_fields.push("model");
        }
        if span_row.tokens_in.is_none() {
            missing_fields.push("tokens_in");
        }
        if span_row.tokens_out.is_none() {
            missing_fields.push("tokens_out");
        }
        if span_row.cost_usd.is_none() {
            missing_fields.push("cost_usd");
        }
    }

    missing_fields
}

fn format_span_id_prefix(full_span_id: &str) -> &str {
    let prefix_length = full_span_id.len().min(8);
    &full_span_id[..prefix_length]
}

fn format_optional_string(optional_value: Option<&String>) -> &str {
    optional_value.map_or("\u{2014}", String::as_str)
}

fn format_optional_i64(optional_value: Option<i64>) -> String {
    optional_value.map_or_else(
        || "\u{2014}".to_string(),
        |integer_value| integer_value.to_string(),
    )
}

fn format_optional_f64(optional_value: Option<f64>) -> String {
    optional_value.map_or_else(
        || "\u{2014}".to_string(),
        |float_value| format!("{float_value:.6}"),
    )
}

fn print_report(database_path: &Path, all_spans: &[SpanRow]) {
    println!(
        "Yantra Telemetry Audit \u{2014} {}",
        database_path.display()
    );
    println!("{}", "\u{2500}".repeat(56));
    println!("Total spans: {}", all_spans.len());

    let system_spans: Vec<&SpanRow> = all_spans
        .iter()
        .filter(|span_row| span_row.agent.is_none())
        .collect();

    let agent_spans: Vec<&SpanRow> = all_spans
        .iter()
        .filter(|span_row| span_row.agent.is_some())
        .collect();

    println!();
    println!("=== SYSTEM SPANS (agent IS NULL) ===");
    println!(
        "{:<10} | {:<36} | {:<24} | {:<10} | {:<11} | issues",
        "span_id", "session", "started_at", "outcome", "duration_ms",
    );
    println!("{}", "\u{2500}".repeat(120));

    for system_span in &system_spans {
        let missing_fields = collect_missing_fields(system_span);
        let issues_display = if missing_fields.is_empty() {
            "OK".to_string()
        } else {
            format!("MISSING: [{}]", missing_fields.join(", "))
        };
        println!(
            "{:<10} | {:<36} | {:<24} | {:<10} | {:<11} | {}",
            format_span_id_prefix(&system_span.span_id),
            system_span.session_id,
            system_span.started_at,
            format_optional_string(system_span.outcome.as_ref()),
            format_optional_i64(system_span.duration_ms),
            issues_display,
        );
    }

    println!();
    println!("=== AGENT SPANS ===");
    println!(
        "{:<10} | {:<36} | {:<20} | {:<20} | {:<9} | {:<10} | {:<12} | {:<10} | issues",
        "span_id", "session", "agent", "model", "tokens_in", "tokens_out", "cost_usd", "outcome",
    );
    println!("{}", "\u{2500}".repeat(160));

    for agent_span in &agent_spans {
        let missing_fields = collect_missing_fields(agent_span);
        let issues_display = if missing_fields.is_empty() {
            "OK".to_string()
        } else {
            format!("MISSING: [{}]", missing_fields.join(", "))
        };
        println!(
            "{:<10} | {:<36} | {:<20} | {:<20} | {:<9} | {:<10} | {:<12} | {:<10} | {}",
            format_span_id_prefix(&agent_span.span_id),
            agent_span.session_id,
            format_optional_string(agent_span.agent.as_ref()),
            format_optional_string(agent_span.model.as_ref()),
            format_optional_i64(agent_span.tokens_in),
            format_optional_i64(agent_span.tokens_out),
            format_optional_f64(agent_span.cost_usd),
            format_optional_string(agent_span.outcome.as_ref()),
            issues_display,
        );
    }

    let spans_with_issues: Vec<(&SpanRow, Vec<&'static str>)> = all_spans
        .iter()
        .filter_map(|span_row| {
            let missing_fields = collect_missing_fields(span_row);
            if missing_fields.is_empty() {
                None
            } else {
                Some((span_row, missing_fields))
            }
        })
        .collect();

    let fully_populated_count = all_spans.len() - spans_with_issues.len();

    println!();
    println!("=== SUMMARY ===");
    println!(
        "Spans with all required fields populated: {}/{}",
        fully_populated_count,
        all_spans.len()
    );

    if spans_with_issues.is_empty() {
        println!("All spans passed field coverage check.");
    } else {
        println!("Spans missing one or more fields:");
        for (problem_span, missing_field_list) in &spans_with_issues {
            println!(
                "  {}: missing [{}]",
                format_span_id_prefix(&problem_span.span_id),
                missing_field_list.join(", ")
            );
        }
    }
}

fn main() {
    let database_path = resolve_database_path();

    let database_connection = match open_database(&database_path) {
        Ok(opened_connection) => opened_connection,
        Err(open_error_message) => {
            eprintln!("{open_error_message}");
            process::exit(1);
        }
    };

    if !table_exists(&database_connection, "spans") {
        println!(
            "No spans table found in {}. No telemetry has been recorded yet.",
            database_path.display()
        );
        process::exit(0);
    }

    let all_spans = match load_all_spans(&database_connection) {
        Ok(loaded_span_rows) => loaded_span_rows,
        Err(load_error_message) => {
            eprintln!("Error reading spans: {load_error_message}");
            process::exit(1);
        }
    };

    print_report(&database_path, &all_spans);

    let any_span_has_issues = all_spans
        .iter()
        .any(|span_row| !collect_missing_fields(span_row).is_empty());

    if any_span_has_issues {
        process::exit(1);
    }
}
