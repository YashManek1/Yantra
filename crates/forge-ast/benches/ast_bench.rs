//! # forge-ast: AST Re-parse Performance Benchmark
//!
//! Creates a temporary directory containing five synthetic Rust source files
//! (each with 20 functions and 10 structs), benchmarks the total time to parse
//! and extract symbols from all five files per iteration, and asserts that the
//! p99 latency across 100 iterations stays below 150 milliseconds.
//!
//! ## Input
//! - Dynamically generated temporary directory with synthetic Rust files
//!
//! ## Output
//! - A markdown report file saved to `target/ast_bench_report.md`
//!
//! ## Related
//! - `forge-ast::parser` — `parse_file` function being benchmarked
//! - `forge-ast::extractor` — `extract_symbols` function being benchmarked

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use uuid::Uuid;
use yantra_ast::{extract_symbols, parse_file};

fn build_synthetic_rust_source(file_index: usize) -> String {
    let mut source_content = String::new();

    for struct_index in 1..=10 {
        source_content.push_str(&format!(
            "/// Synthetic struct {file_index}_{struct_index} for benchmark purposes.\npub struct BenchStruct_{file_index}_{struct_index} {{\n    pub field_alpha: u64,\n    pub field_beta: String,\n    pub field_gamma: bool,\n}}\n\n"
        ));
    }

    for function_index in 1..=20 {
        source_content.push_str(&format!(
            "/// Processes data for bench function {file_index}_{function_index}.\npub fn bench_function_{file_index}_{function_index}(input_value: u64) -> u64 {{\n    let doubled_value = input_value * 2;\n    let incremented_value = doubled_value + {function_index};\n    incremented_value\n}}\n\n"
        ));
    }

    source_content
}

fn main() {
    let temp_directory = std::env::temp_dir().join(format!("yantra-ast-bench-{}", Uuid::new_v4()));
    fs::create_dir_all(&temp_directory).unwrap();

    let mut rust_file_paths = Vec::new();
    let mut total_lines_of_code = 0usize;

    for file_index in 1..=5 {
        let source_content = build_synthetic_rust_source(file_index);
        let file_path = temp_directory.join(format!("bench_module_{file_index}.rs"));
        total_lines_of_code += source_content.lines().count();
        fs::write(&file_path, source_content).unwrap();
        rust_file_paths.push(file_path);
    }

    for _warmup_index in 0..5 {
        for source_path in &rust_file_paths {
            let parsed_file = parse_file(source_path).unwrap();
            let _symbols = extract_symbols(&parsed_file).unwrap();
        }
    }

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _benchmark_index in 0..100 {
        let iteration_start_time = Instant::now();
        for source_path in &rust_file_paths {
            let parsed_file = parse_file(source_path).unwrap();
            let _symbols = extract_symbols(&parsed_file).unwrap();
        }
        iteration_durations.push(iteration_start_time.elapsed());
    }

    iteration_durations.sort();

    let p99_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 benchmark iterations must produce 100 duration samples");

    assert!(
        p99_duration.as_millis() < 150,
        "AST re-parse p99 latency exceeded 150ms target: {p99_duration:?}",
    );

    let p50_duration = iteration_durations
        .get(49)
        .copied()
        .expect("100 benchmark iterations must produce 100 duration samples");

    let minimum_duration = iteration_durations
        .first()
        .copied()
        .expect("iteration durations must be non-empty");

    let maximum_duration = iteration_durations
        .last()
        .copied()
        .expect("iteration durations must be non-empty");

    let total_elapsed: Duration = iteration_durations.iter().sum();
    let mean_duration = total_elapsed / 100;

    let manifest_directory_string =
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let manifest_directory = Path::new(&manifest_directory_string);
    let workspace_root = if manifest_directory.ends_with("crates/forge-ast")
        || manifest_directory.ends_with("crates\\forge-ast")
    {
        manifest_directory
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    } else {
        manifest_directory.to_path_buf()
    };
    let target_directory = workspace_root.join("target");
    fs::create_dir_all(&target_directory).unwrap();
    let report_path = target_directory.join("ast_bench_report.md");

    let report_content = format!(
        "# Yantra AST Re-parse Performance Benchmark Report\n\n\
         - **Date/Time**: {}\n\
         - **Synthetic Files Parsed Per Iteration**: 5\n\
         - **Total Lines of Code (all files)**: {total_lines_of_code}\n\
         - **Benchmark Iterations**: 100\n\
         - **Warmup Iterations**: 5\n\
         - **Minimum Latency**: {:.4} ms\n\
         - **Mean Latency**: {:.4} ms\n\
         - **p50 Latency**: {:.4} ms\n\
         - **p99 Latency**: {:.4} ms\n\
         - **Maximum Latency**: {:.4} ms\n\
         - **Performance Status**: PASS (p99 < 150ms SLA met)\n",
        chrono::Utc::now().to_rfc3339(),
        minimum_duration.as_secs_f64() * 1000.0,
        mean_duration.as_secs_f64() * 1000.0,
        p50_duration.as_secs_f64() * 1000.0,
        p99_duration.as_secs_f64() * 1000.0,
        maximum_duration.as_secs_f64() * 1000.0,
    );

    fs::write(&report_path, report_content).unwrap();
    println!(
        "AST bench p99: {:.3} ms (< 150 ms target)",
        p99_duration.as_secs_f64() * 1000.0
    );
    println!("Report written to: {}", report_path.display());

    fs::remove_dir_all(&temp_directory).unwrap();
}
