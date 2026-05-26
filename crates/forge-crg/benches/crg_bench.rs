//! # Code-Review Graph: Performance Benchmark
//!
//! Generates a 15K lines of code Rust repository fixture, runs the
//! `GraphBuilder::build_from_repo` indexer, measures execution duration,
//! asserts the target performance SLA of under 2.0 seconds, and writes
//! a performance report to the target workspace directory.
//!
//! ## Input
//! - Dynamically generated temporary directory containing Rust files
//!
//! ## Output
//! - A markdown report file saved to `target/crg_bench_report.md`
//!
//! ## Related
//! - `forge-crg::builder` — the builder being benchmarked

use std::fs;
use std::path::Path;
use std::time::Instant;
use uuid::Uuid;
use rusqlite::Connection;
use yantra_crg::GraphBuilder;

fn main() {
    let temp_directory = std::env::temp_dir().join(format!("yantra-crg-bench-{}", Uuid::new_v4()));
    fs::create_dir_all(&temp_directory).unwrap();

    let mut lines_of_code = 0;

    for file_index in 1..=20 {
        let file_path = temp_directory.join(format!("file_{}.rs", file_index));
        let mut file_content = String::new();

        for symbol_index in 1..=60 {
            file_content.push_str(&format!(
                "pub trait Trait_{}_{} {{\n    fn method_{}_{}(&self);\n}}\n\n",
                file_index, symbol_index, file_index, symbol_index
            ));
            file_content.push_str(&format!(
                "pub struct Struct_{}_{};\n\n",
                file_index, symbol_index
            ));
            file_content.push_str(&format!(
                "impl Trait_{}_{} for Struct_{}_{} {{\n    fn method_{}_{}(&self) {{\n        function_{}_{}();\n    }}\n}}\n\n",
                file_index, symbol_index, file_index, symbol_index, file_index, symbol_index, file_index, symbol_index
            ));
            file_content.push_str(&format!(
                "pub fn function_{}_{}() {{\n",
                file_index, symbol_index
            ));

            if symbol_index > 1 {
                file_content.push_str(&format!(
                    "    function_{}_{}();\n",
                    file_index, symbol_index - 1
                ));
            } else if file_index > 1 {
                file_content.push_str(&format!(
                    "    function_{}_{}();\n",
                    file_index - 1, 60
                ));
            }

            file_content.push_str("}\n\n");
        }

        lines_of_code += file_content.lines().count();
        fs::write(&file_path, file_content).unwrap();
    }

    let lib_path = temp_directory.join("lib.rs");
    let mut lib_content = String::new();
    for file_index in 1..=20 {
        lib_content.push_str(&format!("pub mod file_{};\n", file_index));
    }
    lines_of_code += lib_content.lines().count();
    fs::write(&lib_path, lib_content).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);

    let start_time = Instant::now();
    graph_builder.build_from_repo(&temp_directory).unwrap();
    let elapsed_duration = start_time.elapsed();

    let total_symbols: i64 = graph_builder.connection().query_row(
        "SELECT COUNT(*) FROM symbols",
        [],
        |row| row.get(0)
    ).unwrap();

    let total_edges: i64 = graph_builder.connection().query_row(
        "SELECT COUNT(*) FROM edges",
        [],
        |row| row.get(0)
    ).unwrap();

    assert!(
        elapsed_duration.as_secs_f64() < 2.0,
        "Benchmark took longer than 2.0 seconds: {:?}",
        elapsed_duration
    );

    let manifest_dir_str = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let manifest_dir = Path::new(&manifest_dir_str);
    let workspace_root = if manifest_dir.ends_with("crates/forge-crg") || manifest_dir.ends_with("crates\\forge-crg") {
        manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
    } else {
        manifest_dir.to_path_buf()
    };
    let target_directory = workspace_root.join("target");
    fs::create_dir_all(&target_directory).unwrap();
    let report_path = target_directory.join("crg_bench_report.md");

    let report_content = format!(
        "# Yantra CRG Performance Benchmark Report\n\n\
         - **Date/Time**: {}\n\
         - **Total Lines of Code**: {}\n\
         - **Total Symbols Indexed**: {}\n\
         - **Total Relational Edges**: {}\n\
         - **Benchmark Execution Time**: {:.4} s\n\
         - **Performance Status**: PASS (< 2.0s target met)\n",
        chrono::Utc::now().to_rfc3339(),
        lines_of_code,
        total_symbols,
        total_edges,
        elapsed_duration.as_secs_f64()
    );

    fs::write(&report_path, report_content).unwrap();
    fs::remove_dir_all(&temp_directory).unwrap();
}
