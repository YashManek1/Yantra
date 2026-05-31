//! # Code-Review Graph: Performance Benchmark Suite
//!
//! Generates a large Rust repository fixture (~100K lines of code across
//! 80 files), runs the `GraphBuilder::build_from_repo` indexer, builds
//! symbol embeddings via `EmbeddingStore`, and benchmarks the
//! `extract_subgraph` latency under a fixed natural-language query.
//!
//! The p99 SLA target for subgraph extraction is ≤ 50 ms.
//!
//! ## Input
//! - Dynamically generated temporary directory containing 80 synthetic Rust
//!   files (each with 75 traits, structs, impls, and free functions)
//!
//! ## Output
//! - Criterion statistical report: mean, standard deviation, and percentile
//!   estimates for the `extract_subgraph` hot path
//!
//! ## Related
//! - `forge-crg::builder`   — `GraphBuilder` under test
//! - `forge-crg::subgraph`  — `extract_subgraph` function under test
//! - `forge-crg::embedding` — `EmbeddingStore` under test

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use rusqlite::Connection;
use uuid::Uuid;
use yantra_crg::{extract_subgraph, EmbeddingStore, GraphBuilder, GraphCache};

struct BenchFixture {
    _temp_directory: PathBuf,
    graph_cache: GraphCache,
    embedding_store: EmbeddingStore,
}

impl BenchFixture {
    fn create() -> Self {
        let temp_directory =
            std::env::temp_dir().join(format!("yantra-crg-bench-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_directory)
            .expect("creating benchmark temp directory must succeed");

        for file_index in 1..=80 {
            let file_path = temp_directory.join(format!("file_{file_index}.rs"));
            let mut file_content = String::new();

            for symbol_index in 1..=75 {
                file_content.push_str(&format!(
                    "pub trait Trait_{file_index}_{symbol_index} {{\n    fn method_{file_index}_{symbol_index}(&self);\n}}\n\n",
                ));
                file_content.push_str(&format!(
                    "pub struct Struct_{file_index}_{symbol_index};\n\n",
                ));
                file_content.push_str(&format!(
                    "impl Trait_{file_index}_{symbol_index} for Struct_{file_index}_{symbol_index} {{\n    fn method_{file_index}_{symbol_index}(&self) {{\n        function_{file_index}_{symbol_index}();\n    }}\n}}\n\n",
                ));
                file_content.push_str(&format!(
                    "pub fn function_{file_index}_{symbol_index}() {{\n",
                ));

                if symbol_index > 1 {
                    file_content.push_str(&format!(
                        "    function_{file_index}_{}();\n",
                        symbol_index - 1
                    ));
                } else if file_index > 1 {
                    file_content.push_str(&format!("    function_{}_75();\n", file_index - 1));
                }

                file_content.push_str("}\n\n");
            }

            fs::write(&file_path, file_content)
                .expect("writing synthetic crg bench source file must succeed");
        }

        let lib_path = temp_directory.join("lib.rs");
        let mut lib_content = String::new();
        for file_index in 1..=80 {
            lib_content.push_str(&format!("pub mod file_{file_index};\n"));
        }
        fs::write(&lib_path, lib_content)
            .expect("writing lib.rs for crg bench fixture must succeed");

        let sqlite_connection = Connection::open_in_memory()
            .expect("in-memory SQLite connection must open for crg benchmark");
        let graph_builder = GraphBuilder::new(sqlite_connection);

        graph_builder
            .build_from_repo(&temp_directory)
            .expect("graph indexing must succeed for crg benchmark fixture");

        let embedding_store =
            EmbeddingStore::new().expect("EmbeddingStore initialisation must succeed");
        embedding_store
            .embed_all(graph_builder.connection())
            .expect("embedding generation must succeed for crg benchmark fixture");

        let graph_cache = GraphCache::build(graph_builder.connection())
            .expect("GraphCache construction must succeed");

        Self {
            _temp_directory: temp_directory,
            graph_cache,
            embedding_store,
        }
    }
}

impl Drop for BenchFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self._temp_directory);
    }
}

fn bench_crg_extract_subgraph(benchmark_context: &mut Criterion) {
    let bench_fixture = BenchFixture::create();

    let benchmark_query = "find greeting function";

    for _warmup_index in 0..5 {
        let _subgraph = extract_subgraph(
            &bench_fixture.graph_cache,
            &bench_fixture.embedding_store,
            benchmark_query,
            500,
            &[],
        )
        .expect("warm-up subgraph extraction must succeed");
    }

    benchmark_context.bench_function("crg_extract_subgraph_18k_symbols", |bencher| {
        bencher.iter(|| {
            extract_subgraph(
                &bench_fixture.graph_cache,
                &bench_fixture.embedding_store,
                benchmark_query,
                500,
                &[],
            )
            .expect("benchmark subgraph extraction must succeed")
        });
    });
}

criterion_group! {
    name    = crg_benches;
    config  = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100);
    targets = bench_crg_extract_subgraph
}
criterion_main!(crg_benches);

#[test]
fn crg_extract_subgraph_p99_meets_50ms_sla() {
    use std::time::Instant;

    let bench_fixture = BenchFixture::create();

    let benchmark_query = "find greeting function";

    for _warmup_index in 0..5 {
        let _subgraph = extract_subgraph(
            &bench_fixture.graph_cache,
            &bench_fixture.embedding_store,
            benchmark_query,
            500,
            &[],
        )
        .expect("warm-up subgraph extraction must succeed");
    }

    let mut extraction_durations: Vec<Duration> = Vec::new();

    for _benchmark_index in 0..100 {
        let extraction_start_time = Instant::now();
        let _subgraph = extract_subgraph(
            &bench_fixture.graph_cache,
            &bench_fixture.embedding_store,
            benchmark_query,
            500,
            &[],
        )
        .expect("sla-test subgraph extraction must succeed");
        extraction_durations.push(extraction_start_time.elapsed());
    }

    extraction_durations.sort();

    let p99_extraction_duration = extraction_durations
        .get(99)
        .copied()
        .expect("100 benchmark iterations must produce 100 duration samples");

    assert!(
        p99_extraction_duration.as_secs_f64() < 0.05,
        "Extraction p99 latency exceeded 50ms target: {p99_extraction_duration:?}",
    );
}
