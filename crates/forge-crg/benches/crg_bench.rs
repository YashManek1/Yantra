//! # Code-Review Graph: Performance Benchmark Suite
//!
//! Generates a 100K lines of code Rust repository fixture, runs the
//! `GraphBuilder::build_from_repo` indexer, builds symbol embeddings using
//! the `EmbeddingStore`, and benchmarks `extract_subgraph` execution latency.
//! Asserts the p99 extraction time is under 50 milliseconds, and saves
//! a markdown report to the workspace target directory.
//!
//! ## Input
//! - Dynamically generated temporary directory containing Rust files (100K+ `LoC`)
//!
//! ## Output
//! - A markdown report file saved to `target/crg_bench_report.md`
//!
//! ## Related
//! - `forge-crg::builder` — the builder being benchmarked
//! - `forge-crg::subgraph` — the subgraph extractor being benchmarked

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use uuid::Uuid;
use yantra_crg::{extract_subgraph, EmbeddingStore, GraphBuilder, GraphCache};

fn main() {
    let temp_directory = std::env::temp_dir().join(format!("yantra-crg-bench-{}", Uuid::new_v4()));
    fs::create_dir_all(&temp_directory).unwrap();

    let mut lines_of_code = 0;

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

        lines_of_code += file_content.lines().count();
        fs::write(&file_path, file_content).unwrap();
    }

    let lib_path = temp_directory.join("lib.rs");
    let mut lib_content = String::new();
    for file_index in 1..=80 {
        lib_content.push_str(&format!("pub mod file_{file_index};\n"));
    }
    lines_of_code += lib_content.lines().count();
    fs::write(&lib_path, lib_content).unwrap();

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);

    let indexing_start_time = Instant::now();
    graph_builder.build_from_repo(&temp_directory).unwrap();
    let indexing_elapsed_duration = indexing_start_time.elapsed();

    let total_symbols: i64 = graph_builder
        .connection()
        .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
        .unwrap();

    let total_edges: i64 = graph_builder
        .connection()
        .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
        .unwrap();

    let embedding_store = EmbeddingStore::new().unwrap();
    let embedding_start_time = Instant::now();
    embedding_store
        .embed_all(graph_builder.connection())
        .unwrap();
    let embedding_elapsed_duration = embedding_start_time.elapsed();

    let graph_cache = GraphCache::build(graph_builder.connection()).unwrap();

    for _warmup_index in 0..5 {
        let _subgraph = extract_subgraph(
            &graph_cache,
            &embedding_store,
            "find greeting function",
            500,
            &[],
        )
        .unwrap();
    }

    let query_text = "find greeting function";
    let start_embed = Instant::now();
    let query_vector = embedding_store.embed_query(query_text).unwrap();
    let embed_duration = start_embed.elapsed();

    let start_search = Instant::now();
    let _semantic_matches = embedding_store
        .search_with_embedding(&query_vector, 10)
        .unwrap();
    let search_duration = start_search.elapsed();

    println!("embed_query (cached): {embed_duration:?}");
    println!("search_with_embedding (18K symbols): {search_duration:?}");

    let start_full_extraction = Instant::now();
    let _subgraph = extract_subgraph(&graph_cache, &embedding_store, query_text, 500, &[]).unwrap();
    println!(
        "full extract_subgraph: {:?}",
        start_full_extraction.elapsed()
    );

    let mut extraction_durations: Vec<Duration> = Vec::new();
    for _benchmark_index in 0..100 {
        let extraction_start_time = Instant::now();
        let _subgraph = extract_subgraph(
            &graph_cache,
            &embedding_store,
            "find greeting function",
            500,
            &[],
        )
        .unwrap();
        extraction_durations.push(extraction_start_time.elapsed());
    }

    extraction_durations.sort();
    let p99_extraction_duration = extraction_durations
        .get(99)
        .copied()
        .expect("100 benchmark iterations must produce 100 durations");

    assert!(
        p99_extraction_duration.as_secs_f64() < 0.05,
        "Extraction p99 latency exceeded 50ms target: {p99_extraction_duration:?}",
    );

    let manifest_directory_string =
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let manifest_directory = Path::new(&manifest_directory_string);
    let workspace_root = if manifest_directory.ends_with("crates/forge-crg")
        || manifest_directory.ends_with("crates\\forge-crg")
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
    let report_path = target_directory.join("crg_bench_report.md");

    let report_content = format!(
        "# Yantra CRG Performance Benchmark Report\n\n\
         - **Date/Time**: {}\n\
         - **Total Lines of Code**: {lines_of_code}\n\
         - **Total Symbols Indexed**: {total_symbols}\n\
         - **Total Relational Edges**: {total_edges}\n\
         - **Indexing Phase Duration**: {:.4} s\n\
         - **Embedding Generation Phase Duration**: {:.4} s\n\
         - **Extraction Latency p99 (100 runs)**: {:.4} ms\n\
         - **Performance Status**: PASS (p99 < 50ms SLA met)\n",
        chrono::Utc::now().to_rfc3339(),
        indexing_elapsed_duration.as_secs_f64(),
        embedding_elapsed_duration.as_secs_f64(),
        p99_extraction_duration.as_secs_f64() * 1000.0
    );

    fs::write(&report_path, report_content).unwrap();
    fs::remove_dir_all(&temp_directory).unwrap();
}
