//! # forge-cli::commands::context: Context Lens Command
//!
//! Renders the CRG subgraph that would be assembled for a given task
//! description, with a token ledger breaking the budget into components.
//! The compression ratio (full repo tokens vs context tokens) demonstrates
//! how the runtime saves tokens compared to naive whole-file dumping.
//!
//! ## Input
//! - `task_description: String` — natural language task description
//! - `--json` flag — emit machine-readable JSON instead of a terminal table
//! - `--budget` flag — override the token budget (default 4096)
//!
//! ## Output
//! - Token ledger table: system / repo-genome / subgraph / task components
//! - Compression ratio (estimated full repo tokens → context tokens)
//! - List of seed symbols included and why
//!
//! ## Related
//! - `forge-crg` — `extract_subgraph` produces the subgraph
//! - `forge-tokenizer` — `count_tokens` measures each component
//! - `forge-cli::commands::graph` — shares the GraphCache loading path

use clap::Args;
use rusqlite::Connection;
use serde::Serialize;
use yantra_tokenizer::count_tokens;

#[derive(Args, Debug)]
pub struct ContextArgs {
    /// Natural language description of the task to analyse context for.
    pub task_description: String,

    /// Token budget ceiling (default 4096).
    #[arg(long, default_value = "4096")]
    pub budget: usize,

    /// Output as JSON instead of a formatted table.
    #[arg(long)]
    pub json: bool,
}

/// Token ledger breaking down how a task's context budget is allocated.
#[derive(Debug, Serialize)]
pub struct TokenLedger {
    /// Tokens consumed by the task description itself.
    pub task_tokens: usize,
    /// Fixed overhead for system prompt and STVP boilerplate.
    pub system_prompt_tokens: usize,
    /// Tokens consumed by the extracted CRG subgraph.
    pub subgraph_tokens: usize,
    /// Total tokens that would be sent to the model.
    pub total_context_tokens: usize,
    /// Estimated token count if the full repository were dumped naively.
    pub estimated_full_repo_tokens: usize,
    /// Percentage of tokens saved versus a naive full-repo dump.
    pub compression_ratio_percent: f64,
    /// Number of seed symbols included in the subgraph.
    pub seed_count: usize,
}

impl TokenLedger {
    /// Computes the ledger from the task description and subgraph text.
    ///
    /// The `seed_count` field is left at zero and must be filled in by the
    /// caller after the subgraph is extracted.
    pub fn compute(
        task_description: &str,
        subgraph_text: &str,
        estimated_full_repo_tokens: usize,
    ) -> Self {
        const SYSTEM_PROMPT_OVERHEAD_TOKENS: usize = 512;

        let task_tokens = count_tokens(task_description);
        let subgraph_tokens = count_tokens(subgraph_text);
        let total_context_tokens = SYSTEM_PROMPT_OVERHEAD_TOKENS + task_tokens + subgraph_tokens;

        let compression_ratio_percent = if estimated_full_repo_tokens > 0 {
            let saved = estimated_full_repo_tokens.saturating_sub(total_context_tokens);
            (saved as f64 / estimated_full_repo_tokens as f64) * 100.0
        } else {
            0.0
        };

        Self {
            task_tokens,
            system_prompt_tokens: SYSTEM_PROMPT_OVERHEAD_TOKENS,
            subgraph_tokens,
            total_context_tokens,
            estimated_full_repo_tokens,
            compression_ratio_percent,
            seed_count: 0,
        }
    }
}

/// Runs the context lens command, printing a token ledger for the task.
///
/// # Errors
///
/// Returns `anyhow::Error` if the CRG database cannot be opened, any SQL
/// query fails, or JSON serialization fails.
pub async fn run_context(args: ContextArgs) -> anyhow::Result<()> {
    tracing::info!("running context lens for task: {}", args.task_description);

    let crg_database_path = std::path::Path::new(".yantra").join("crg.sqlite");

    let (subgraph_text, seed_count, estimated_full_repo_tokens) = if crg_database_path.exists() {
        load_subgraph_from_crg(&crg_database_path, &args.task_description, args.budget)?
    } else {
        tracing::warn!(
            "no .yantra/crg.sqlite found — run `yantra index` first for accurate context"
        );
        (String::new(), 0, 0)
    };

    let mut ledger = TokenLedger::compute(
        &args.task_description,
        &subgraph_text,
        estimated_full_repo_tokens,
    );
    ledger.seed_count = seed_count;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&ledger)?);
    } else {
        print_ledger_table(&ledger, &args.task_description);
    }

    Ok(())
}

/// Loads a token-budgeted subgraph sample from the CRG database and returns
/// `(subgraph_text, seed_count, estimated_full_repo_tokens)`.
///
/// Uses the top symbols by `connectivity_score` as a proxy for relevance when
/// a full semantic extraction is not available. The caller is responsible for
/// ensuring `crg_database_path` exists before calling.
///
/// # Errors
///
/// Returns `anyhow::Error` if the database cannot be opened or any SQL
/// statement fails to prepare or execute.
fn load_subgraph_from_crg(
    crg_database_path: &std::path::Path,
    task_description: &str,
    token_budget: usize,
) -> anyhow::Result<(String, usize, usize)> {
    let database_connection = Connection::open(crg_database_path)?;

    let total_symbol_count: i64 = database_connection
        .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
        .unwrap_or(0);

    // Approximate full-repo token size: average symbol is ~40 tokens
    // (signature + docstring). This is an intentional estimate; a precise
    // per-file measurement is not stored in the CRG.
    let estimated_full_repo_tokens = (total_symbol_count as usize) * 40;

    let candidate_lines: Vec<String> = {
        let mut statement = database_connection.prepare(
            "SELECT s.name, s.kind, f.path FROM symbols s
             JOIN files f ON s.file_id = f.id
             ORDER BY s.connectivity_score DESC
             LIMIT 20",
        )?;
        let collected: Vec<String> = statement
            .query_map([], |row| {
                let symbol_name: String = row.get(0)?;
                let symbol_kind: String = row.get(1)?;
                let file_path: String = row.get(2)?;
                Ok(format!("{symbol_kind} {symbol_name} ({file_path})"))
            })?
            .filter_map(std::result::Result::ok)
            .collect();
        collected
    };

    let total_candidate_tokens = count_tokens(&candidate_lines.join("\n"));
    let seed_count = candidate_lines.len();

    let bounded_subgraph_text = if total_candidate_tokens <= token_budget {
        candidate_lines.join("\n")
    } else {
        budget_trim_lines(candidate_lines, task_description, token_budget)
    };

    Ok((
        bounded_subgraph_text,
        seed_count,
        estimated_full_repo_tokens,
    ))
}

/// Trims `candidate_lines` to fit within `token_budget` by accumulating lines
/// until adding the next line would exceed the budget.
fn budget_trim_lines(
    candidate_lines: Vec<String>,
    _task_description: &str,
    token_budget: usize,
) -> String {
    let mut accumulated_tokens: usize = 0;
    let mut kept_lines: Vec<String> = Vec::new();

    for line in candidate_lines {
        let line_tokens = count_tokens(&line);
        if accumulated_tokens + line_tokens > token_budget {
            break;
        }
        accumulated_tokens += line_tokens;
        kept_lines.push(line);
    }

    kept_lines.join("\n")
}

/// Prints the token ledger as a formatted table to stdout.
fn print_ledger_table(ledger: &TokenLedger, task_description: &str) {
    println!("\nContext Lens — \"{task_description}\"");
    println!("{:-<60}", "");
    println!("{:<35} {:>10}", "Component", "Tokens");
    println!("{:-<60}", "");
    println!(
        "{:<35} {:>10}",
        "System prompt (overhead)", ledger.system_prompt_tokens
    );
    println!("{:<35} {:>10}", "Task description", ledger.task_tokens);
    println!("{:<35} {:>10}", "CRG subgraph", ledger.subgraph_tokens);
    println!("{:-<60}", "");
    println!(
        "{:<35} {:>10}",
        "Total context", ledger.total_context_tokens
    );
    println!(
        "{:<35} {:>10}",
        "Est. full-repo tokens", ledger.estimated_full_repo_tokens
    );
    println!(
        "{:<35} {:>9.1}%",
        "Compression ratio", ledger.compression_ratio_percent
    );
    println!("{:-<60}", "");
    println!("{:<35} {:>10}", "Seed symbols", ledger.seed_count);
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_ledger_compute_sums_components_correctly() {
        let task = "add a new endpoint";
        let subgraph = "function foo (crates/a/src/lib.rs)";
        let estimated_full_repo_tokens = 10_000;

        let ledger = TokenLedger::compute(task, subgraph, estimated_full_repo_tokens);

        assert_eq!(ledger.system_prompt_tokens, 512);
        assert_eq!(ledger.task_tokens, count_tokens(task));
        assert_eq!(ledger.subgraph_tokens, count_tokens(subgraph));
        assert_eq!(
            ledger.total_context_tokens,
            512 + ledger.task_tokens + ledger.subgraph_tokens
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn token_ledger_compression_ratio_is_zero_when_no_full_repo_tokens() {
        let ledger = TokenLedger::compute("task", "subgraph", 0);
        assert_eq!(ledger.compression_ratio_percent, 0.0);
    }

    #[test]
    fn token_ledger_compression_ratio_is_positive_for_large_repo() {
        let ledger = TokenLedger::compute("task", "", 100_000);
        assert!(ledger.compression_ratio_percent > 0.0);
        assert!(ledger.compression_ratio_percent <= 100.0);
    }

    #[test]
    fn budget_trim_lines_keeps_lines_within_budget() {
        let lines: Vec<String> = (0..10)
            .map(|line_index| format!("function symbol_{line_index} (crates/a/src/lib.rs)"))
            .collect();
        let trimmed = budget_trim_lines(lines, "task", 5);
        let trimmed_token_count = count_tokens(&trimmed);
        assert!(trimmed_token_count <= 5, "got {trimmed_token_count} tokens");
    }

    #[test]
    fn budget_trim_lines_returns_empty_when_budget_is_zero() {
        let lines = vec!["function foo (crates/a/src/lib.rs)".to_string()];
        let trimmed = budget_trim_lines(lines, "task", 0);
        assert!(trimmed.is_empty());
    }

    #[tokio::test]
    async fn run_context_succeeds_when_no_crg_database_exists() {
        let args = ContextArgs {
            task_description: "add error handling".to_string(),
            budget: 4096,
            json: true,
        };
        let result = run_context(args).await;
        assert!(
            result.is_ok(),
            "context lens must not fail without a CRG: {result:?}"
        );
    }
}
