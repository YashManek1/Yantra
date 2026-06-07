//! # Code-Review Graph: Token-Reduction Measurement
//!
//! Measures the actual BPE token-count reduction that Yantra's CRG delivers
//! versus the naive whole-repo-in-context approach used by tools such as Claude
//! Code and Cursor. Produces a [`ReductionReport`] with per-task breakdown and
//! aggregate statistics, serialisable to Markdown for documentation and CI
//! artefacts.
//!
//! ## Input
//! - A repository root path (the CRG is built over its indexed source files)
//! - A slice of [`TaskSpec`] entries describing tasks and their expected symbol sets
//! - A `token_budget` limiting each rendered subgraph
//!
//! ## Output
//! - [`ReductionReport`] — baseline token count, per-task measurements, aggregate
//!   reduction ratio and recall across all tasks
//!
//! ## Related
//! - `forge-crg::builder`  — `GraphBuilder` used to construct the graph
//! - `forge-crg::subgraph` — `extract_subgraph` and `GraphCache`
//! - `forge-tokenizer`     — `count_tokens` (cl100k_base BPE)

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::builder::GraphBuilder;
use crate::embedding::EmbeddingStore;
use crate::error::CrgResult;
use crate::subgraph::{extract_subgraph, GraphCache};

/// A single task whose token reduction is to be measured.
pub struct TaskSpec {
    /// Natural-language task description forwarded to `extract_subgraph`.
    pub description: String,
    /// Symbol names (exact `name` field matches in the graph) expected to
    /// appear in the compressed subgraph. Used to compute per-task recall.
    pub expected_symbols: Vec<String>,
}

/// Token-reduction and recall results for a single [`TaskSpec`].
pub struct TaskMeasurement {
    /// The original task description.
    pub task_description: String,
    /// Total BPE tokens across all indexed source files (the naive baseline).
    pub baseline_tokens: usize,
    /// BPE tokens in the rendered CRG subgraph for this task.
    pub compressed_tokens: usize,
    /// `1.0 − compressed / baseline` — fraction of tokens saved.
    pub reduction_ratio: f64,
    /// Fraction of `expected_symbols` whose exact name appears in the subgraph.
    pub recall: f64,
    /// Number of symbols retained in the rendered subgraph.
    pub included_symbol_count: usize,
}

/// Aggregate token-reduction report across all measured tasks.
pub struct ReductionReport {
    /// Total BPE tokens across all indexed source files.
    pub baseline_tokens: usize,
    /// Number of source files included in the baseline count.
    pub file_count: usize,
    /// Per-task breakdown.
    pub per_task: Vec<TaskMeasurement>,
    /// Mean compressed token count across all tasks.
    pub mean_compressed_tokens: f64,
    /// Mean reduction ratio across all tasks (1.0 = perfect; 0.0 = no reduction).
    pub mean_reduction_ratio: f64,
    /// Median reduction ratio across all tasks.
    pub median_reduction_ratio: f64,
    /// Mean recall across all tasks (1.0 = all expected symbols found).
    pub mean_recall: f64,
}

/// Walks `repo_root` and returns the total cl100k_base BPE token count and
/// file count for every source file that the CRG would index.
///
/// # Errors
///
/// Returns an error if a directory entry or file read fails.
pub fn compute_baseline_tokens(repo_root: &Path) -> anyhow::Result<(usize, usize)> {
    let mut source_file_paths: Vec<PathBuf> = Vec::new();
    walk_source_files(repo_root, &mut source_file_paths)?;

    let mut total_token_count: usize = 0;
    let mut file_count: usize = 0;

    for source_file_path in &source_file_paths {
        if let Ok(source_text) = fs::read_to_string(source_file_path) {
            total_token_count += yantra_tokenizer::count_tokens(&source_text);
            file_count += 1;
        } else {
            // Skip files that cannot be read as UTF-8 (binary artefacts, etc.)
        }
    }

    Ok((total_token_count, file_count))
}

/// Builds a CRG over `repo_root`, runs [`extract_subgraph`] for each entry in
/// `tasks`, and returns a [`ReductionReport`] comparing the compressed subgraph
/// token cost against the whole-repo baseline.
///
/// Recall for each task is the fraction of [`TaskSpec::expected_symbols`] whose
/// exact `name` field appears among the symbols retained in the subgraph,
/// looked up via [`GraphCache::symbol_details`].
///
/// # Errors
///
/// Returns an error if graph construction, embedding, or extraction fails.
pub fn measure_token_reduction(
    repo_root: &Path,
    tasks: &[TaskSpec],
    token_budget: usize,
) -> anyhow::Result<ReductionReport> {
    let (baseline_tokens, file_count) = compute_baseline_tokens(repo_root)?;

    let sqlite_connection = Connection::open_in_memory()?;
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder.build_from_repo(repo_root)?;

    let embedding_store = EmbeddingStore::new()?;
    embedding_store.embed_all(graph_builder.connection())?;

    let graph_cache = GraphCache::build(graph_builder.connection())?;

    let mut per_task_measurements: Vec<TaskMeasurement> = Vec::new();

    for task_spec in tasks {
        let rendered_subgraph = extract_subgraph(
            &graph_cache,
            &embedding_store,
            &task_spec.description,
            token_budget,
            &[],
        )?;

        let compressed_tokens = yantra_tokenizer::count_tokens(&rendered_subgraph.text);
        let reduction_ratio = if baseline_tokens == 0 {
            0.0
        } else {
            1.0 - (compressed_tokens as f64 / baseline_tokens as f64)
        };

        let included_symbol_names: HashSet<String> = rendered_subgraph
            .included_nodes
            .iter()
            .filter_map(|symbol_id| graph_cache.symbol_details.get(symbol_id))
            .map(|symbol_detail| symbol_detail.name.clone())
            .collect();

        let matched_count = task_spec
            .expected_symbols
            .iter()
            .filter(|expected_symbol| included_symbol_names.contains(*expected_symbol))
            .count();

        let recall = if task_spec.expected_symbols.is_empty() {
            1.0
        } else {
            matched_count as f64 / task_spec.expected_symbols.len() as f64
        };

        per_task_measurements.push(TaskMeasurement {
            task_description: task_spec.description.clone(),
            baseline_tokens,
            compressed_tokens,
            reduction_ratio,
            recall,
            included_symbol_count: rendered_subgraph.included_nodes.len(),
        });
    }

    let task_count = per_task_measurements.len();

    let mean_compressed_tokens = if task_count == 0 {
        0.0
    } else {
        per_task_measurements
            .iter()
            .map(|measurement| measurement.compressed_tokens as f64)
            .sum::<f64>()
            / task_count as f64
    };

    let mean_reduction_ratio = if task_count == 0 {
        0.0
    } else {
        per_task_measurements
            .iter()
            .map(|measurement| measurement.reduction_ratio)
            .sum::<f64>()
            / task_count as f64
    };

    let mut sorted_reduction_ratios: Vec<f64> = per_task_measurements
        .iter()
        .map(|measurement| measurement.reduction_ratio)
        .collect();
    sorted_reduction_ratios.sort_by(|ratio_a, ratio_b| {
        ratio_a
            .partial_cmp(ratio_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let median_reduction_ratio = if task_count == 0 {
        0.0
    } else if task_count % 2 == 0 {
        (sorted_reduction_ratios[task_count / 2 - 1] + sorted_reduction_ratios[task_count / 2])
            / 2.0
    } else {
        sorted_reduction_ratios[task_count / 2]
    };

    let mean_recall = if task_count == 0 {
        0.0
    } else {
        per_task_measurements
            .iter()
            .map(|measurement| measurement.recall)
            .sum::<f64>()
            / task_count as f64
    };

    Ok(ReductionReport {
        baseline_tokens,
        file_count,
        per_task: per_task_measurements,
        mean_compressed_tokens,
        mean_reduction_ratio,
        median_reduction_ratio,
        mean_recall,
    })
}

/// Renders `report` as a Markdown document with a comparison table, per-task
/// rows, and a methodology section explaining the measurement approach.
pub fn render_markdown_report(report: &ReductionReport) -> String {
    let mut output = String::new();

    output.push_str("# Yantra CRG Token-Reduction Report\n\n");

    output.push_str("## Summary\n\n");
    output.push_str("| Metric | Value |\n");
    output.push_str("|--------|-------|\n");
    output.push_str(&format!(
        "| Baseline — whole-repo naive | {:>10} tokens ({} files) |\n",
        report.baseline_tokens, report.file_count,
    ));
    output.push_str(&format!(
        "| Mean compressed — CRG subgraph | {:>10.0} tokens |\n",
        report.mean_compressed_tokens,
    ));
    output.push_str(&format!(
        "| Mean token reduction | {:>9.1}% |\n",
        report.mean_reduction_ratio * 100.0,
    ));
    output.push_str(&format!(
        "| Median token reduction | {:>9.1}% |\n",
        report.median_reduction_ratio * 100.0,
    ));
    output.push_str(&format!(
        "| Mean recall (expected symbols found) | {:>9.1}% |\n",
        report.mean_recall * 100.0,
    ));

    output.push_str("\n## Per-Task Breakdown\n\n");
    output.push_str("| Task | Baseline | Compressed | Reduction | Recall | Symbols |\n");
    output.push_str("|------|----------|------------|-----------|--------|---------|\n");

    for measurement in &report.per_task {
        let short_description = if measurement.task_description.len() > 50 {
            format!("{}...", &measurement.task_description[..47])
        } else {
            measurement.task_description.clone()
        };
        output.push_str(&format!(
            "| {} | {} | {} | {:.1}% | {:.0}% | {} |\n",
            short_description,
            measurement.baseline_tokens,
            measurement.compressed_tokens,
            measurement.reduction_ratio * 100.0,
            measurement.recall * 100.0,
            measurement.included_symbol_count,
        ));
    }

    output.push_str("\n## Methodology\n\n");
    output.push_str(
        "**Baseline** — sum of cl100k_base BPE token counts across every source file \
         that `GraphBuilder` indexes (same file set and same directory exclusions). \
         Represents the naive whole-repo-in-context approach that tools such as Claude \
         Code and Cursor approximate when loading a repository.\n\n",
    );
    output.push_str(
        "**Compressed** — cl100k_base BPE token count of the `RenderedSubgraph.text` \
         returned by `extract_subgraph` at the configured `token_budget`. The subgraph \
         contains only the symbols most relevant to the task query, ranked by embedding \
         similarity and graph connectivity.\n\n",
    );
    output.push_str(
        "**Recall** — fraction of per-task expected symbols whose exact name appears \
         among the subgraph's `included_nodes`, resolved via `GraphCache::symbol_details`. \
         The recall gate prevents gaming the reduction metric by returning an empty subgraph.\n\n",
    );
    output.push_str(
        "**Reproduce** — \
         `cargo test -p yantra-crg --test token_reduction -- --nocapture`\n\n",
    );
    output.push_str(
        "> Note: live token savings in production agents will vary. Real tools send \
         partial context (recent conversation, file snippets), so the absolute baseline \
         may be lower. The reduction ratio versus a full-repo dump is the headline metric.\n",
    );

    output
}

/// Recursively collects source files under `path` using the same inclusion and
/// directory-exclusion rules as `GraphBuilder::build_from_repo`. Skips hidden
/// directories, `target/`, `node_modules/`, `tests/`, `benches/`, `examples/`,
/// and `fixtures/`.
fn walk_source_files(path: &Path, files: &mut Vec<PathBuf>) -> CrgResult<()> {
    if path.is_file() {
        if yantra_ast::LanguageRegistry::language_for_path(path).is_some() {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        for dir_entry in fs::read_dir(path)? {
            let dir_entry = dir_entry?;
            let child_path = dir_entry.path();
            if let Some(entry_name) = child_path.file_name().and_then(|name| name.to_str()) {
                if entry_name.starts_with('.')
                    || entry_name == "target"
                    || entry_name == "node_modules"
                    || entry_name == "tests"
                    || entry_name == "benches"
                    || entry_name == "examples"
                    || entry_name == "fixtures"
                {
                    continue;
                }
            }
            walk_source_files(&child_path, files)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn create_small_synthetic_repo() -> PathBuf {
        let temp_directory =
            std::env::temp_dir().join(format!("yantra-token-reduction-unit-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_directory).unwrap();
        fs::write(
            temp_directory.join("lib.rs"),
            "pub fn hello() -> &'static str { \"hello\" }",
        )
        .unwrap();
        temp_directory
    }

    #[test]
    fn test_compute_baseline_tokens_counts_rust_source_files() {
        let temp_directory = create_small_synthetic_repo();
        let (total_tokens, file_count) = compute_baseline_tokens(&temp_directory).unwrap();
        assert!(total_tokens > 0, "baseline token count must be non-zero");
        assert_eq!(file_count, 1, "synthetic repo has exactly one source file");
        fs::remove_dir_all(&temp_directory).unwrap();
    }

    #[test]
    fn test_render_markdown_report_contains_required_sections() {
        let report = ReductionReport {
            baseline_tokens: 100_000,
            file_count: 50,
            per_task: vec![TaskMeasurement {
                task_description: "example task".to_string(),
                baseline_tokens: 100_000,
                compressed_tokens: 2_000,
                reduction_ratio: 0.98,
                recall: 1.0,
                included_symbol_count: 8,
            }],
            mean_compressed_tokens: 2_000.0,
            mean_reduction_ratio: 0.98,
            median_reduction_ratio: 0.98,
            mean_recall: 1.0,
        };

        let markdown_content = render_markdown_report(&report);
        assert!(markdown_content.contains("# Yantra CRG Token-Reduction Report"));
        assert!(markdown_content.contains("## Summary"));
        assert!(markdown_content.contains("## Per-Task Breakdown"));
        assert!(markdown_content.contains("## Methodology"));
        assert!(markdown_content.contains("98.0%"));
    }

    #[test]
    fn test_walk_source_files_skips_target_directory() {
        let temp_directory =
            std::env::temp_dir().join(format!("yantra-walk-test-{}", Uuid::new_v4()));
        let src_directory = temp_directory.join("src");
        let target_directory = temp_directory.join("target");
        fs::create_dir_all(&src_directory).unwrap();
        fs::create_dir_all(&target_directory).unwrap();
        fs::write(src_directory.join("lib.rs"), "pub fn hello() {}").unwrap();
        fs::write(target_directory.join("generated.rs"), "// generated").unwrap();

        let mut collected_files: Vec<PathBuf> = Vec::new();
        walk_source_files(&temp_directory, &mut collected_files).unwrap();

        assert_eq!(
            collected_files.len(),
            1,
            "only src/lib.rs should be collected; target/ must be excluded"
        );
        fs::remove_dir_all(&temp_directory).unwrap();
    }
}
