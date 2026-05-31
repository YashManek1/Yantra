//! # forge-ast: AST Re-Parse Benchmark
//!
//! Measures the latency of parsing and extracting symbols from a set of
//! synthetic Rust source files via Tree-sitter. The benchmark exercises
//! both `parse_file` and `extract_symbols` per iteration across five
//! generated files (each containing 20 functions and 10 structs).
//!
//! The p99 SLA target is ≤ 150 ms; the achieved measurement on the
//! `feat/canvas-graph-observe` branch is approximately 7.2 ms.
//!
//! ## Input
//! - Dynamically generated temporary directory with synthetic Rust files
//!   (5 files × 30 items each)
//!
//! ## Output
//! - Criterion statistical report: mean, standard deviation, p50, p99 estimates
//!
//! ## Related
//! - `forge-ast::parser`    — `parse_file` function under test
//! - `forge-ast::extractor` — `extract_symbols` function under test

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
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

struct BenchFixture {
    temp_directory: PathBuf,
    rust_file_paths: Vec<PathBuf>,
}

impl BenchFixture {
    fn create() -> Self {
        let temp_directory =
            std::env::temp_dir().join(format!("yantra-ast-bench-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_directory)
            .expect("creating benchmark temp directory must succeed");

        let mut rust_file_paths = Vec::new();

        for file_index in 1..=5 {
            let source_content = build_synthetic_rust_source(file_index);
            let file_path = temp_directory.join(format!("bench_module_{file_index}.rs"));
            fs::write(&file_path, &source_content)
                .expect("writing synthetic bench source file must succeed");
            rust_file_paths.push(file_path);
        }

        Self {
            temp_directory,
            rust_file_paths,
        }
    }
}

impl Drop for BenchFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_directory);
    }
}

fn bench_ast_reparse(benchmark_context: &mut Criterion) {
    let bench_fixture = BenchFixture::create();

    for _warmup_index in 0..5 {
        for source_path in &bench_fixture.rust_file_paths {
            let parsed_file = parse_file(source_path).expect("warm-up parse must succeed");
            let _symbols =
                extract_symbols(&parsed_file).expect("warm-up symbol extraction must succeed");
        }
    }

    benchmark_context.bench_function("ast_reparse_five_files", |bencher| {
        bencher.iter(|| {
            for source_path in &bench_fixture.rust_file_paths {
                let parsed_file =
                    parse_file(source_path).expect("benchmark parse_file must succeed");
                let _symbols =
                    extract_symbols(&parsed_file).expect("benchmark extract_symbols must succeed");
            }
        });
    });
}

criterion_group! {
    name    = ast_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100);
    targets = bench_ast_reparse
}
criterion_main!(ast_benches);

#[test]
fn ast_reparse_p99_meets_150ms_sla() {
    use std::time::Instant;

    let bench_fixture = BenchFixture::create();

    for _warmup_index in 0..5 {
        for source_path in &bench_fixture.rust_file_paths {
            let parsed_file = parse_file(source_path).expect("warm-up parse must succeed");
            let _symbols =
                extract_symbols(&parsed_file).expect("warm-up symbol extraction must succeed");
        }
    }

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _benchmark_index in 0..100 {
        let iteration_start_time = Instant::now();
        for source_path in &bench_fixture.rust_file_paths {
            let parsed_file = parse_file(source_path).expect("sla-test parse must succeed");
            let _symbols =
                extract_symbols(&parsed_file).expect("sla-test symbol extraction must succeed");
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
}
