//! # Token-Reduction Integration Test
//!
//! Measures the actual BPE token-count reduction that Yantra's CRG delivers on
//! the real Yantra codebase, asserts reduction and recall thresholds, and
//! writes a Markdown report to `target/crg_token_reduction_report.md`.
//!
//! ## Input
//! - The Yantra workspace root (resolved via `CARGO_MANIFEST_DIR`)
//! - A golden corpus of representative tasks with expected symbols
//!
//! ## Output
//! - Assertion: mean token reduction ≥ 90%
//! - Assertion: mean recall ≥ 80%
//! - File: `target/crg_token_reduction_report.md`
//!
//! ## Related
//! - `forge-crg::token_reduction` — measurement pipeline under test

use std::fs;
use std::path::Path;

use yantra_crg::token_reduction::{measure_token_reduction, render_markdown_report, TaskSpec};

#[test]
fn test_crg_delivers_significant_token_reduction_on_yantra_repo() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("CARGO_MANIFEST_DIR must have a grandparent (workspace root)");

    let golden_tasks = vec![
        TaskSpec {
            description: "extract a token-budgeted subgraph from the code review graph".to_string(),
            expected_symbols: vec!["extract_subgraph".to_string(), "GraphCache".to_string()],
        },
        TaskSpec {
            description: "count BPE tokens in text before forwarding to a language model"
                .to_string(),
            expected_symbols: vec!["count_tokens".to_string()],
        },
        TaskSpec {
            description: "build the code review graph by indexing a repository directory"
                .to_string(),
            expected_symbols: vec!["GraphBuilder".to_string()],
        },
        TaskSpec {
            description: "embed symbols for semantic similarity search in the graph".to_string(),
            expected_symbols: vec!["EmbeddingStore".to_string()],
        },
        TaskSpec {
            description: "rendered subgraph text output with included nodes manifest".to_string(),
            expected_symbols: vec!["RenderedSubgraph".to_string()],
        },
        TaskSpec {
            description: "detect Louvain communities in the code review graph".to_string(),
            expected_symbols: vec!["detect_communities".to_string()],
        },
        TaskSpec {
            description: "create the SQLite schema for the code review graph".to_string(),
            expected_symbols: vec!["create_crg_schema".to_string()],
        },
        TaskSpec {
            description: "seed extraction from task description for subgraph queries".to_string(),
            expected_symbols: vec!["SeedExtractor".to_string()],
        },
    ];

    let report = measure_token_reduction(workspace_root, &golden_tasks, 4000)
        .expect("token-reduction measurement must succeed on the Yantra repo");

    let report_markdown = render_markdown_report(&report);
    println!("\n{report_markdown}");

    let report_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("crg_token_reduction_report.md");
    fs::create_dir_all(report_path.parent().unwrap()).ok();
    fs::write(&report_path, &report_markdown).ok();
    println!("Report written to: {}", report_path.display());

    println!(
        "\nMeasured: baseline={} tokens, {} files",
        report.baseline_tokens, report.file_count,
    );
    println!(
        "Mean compressed: {:.0} tokens  |  mean reduction: {:.1}%  |  mean recall: {:.1}%",
        report.mean_compressed_tokens,
        report.mean_reduction_ratio * 100.0,
        report.mean_recall * 100.0,
    );

    assert!(
        report.mean_reduction_ratio >= 0.95,
        "Expected CRG to reduce tokens by ≥95%, got {:.1}% (baseline={}, mean_compressed={:.0})",
        report.mean_reduction_ratio * 100.0,
        report.baseline_tokens,
        report.mean_compressed_tokens,
    );

    assert!(
        report.mean_recall >= 0.95,
        "Expected ≥95% recall of expected symbols, got {:.1}%",
        report.mean_recall * 100.0,
    );
}
