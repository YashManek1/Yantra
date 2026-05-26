//! # forge-cli: `yantra run` Command
//!
//! Drives the full Day-3 pipeline for a single task:
//!
//! 1. Load or generate the session signing key.
//! 2. Run the STVP interrogator (questionnaire via `inquire`).
//! 3. Execute validators; re-prompt on fixable testability violations.
//! 4. Compile each `success_criterion` into a spec-test skeleton.
//! 5. Show the truth artifact preview and generated test paths.
//! 6. Ask the user to confirm (or go back and refine).
//! 7. Issue the Ed25519 truth token.
//! 8. Submit the signed task to the orchestrator.
//! 9. Extract the CRG subgraph if the index exists.
//! 10. Call the Coder model and print the produced diff.
//!
//! ## Input
//! - `description` — natural-language task from the CLI argument
//! - `project_root` — resolved from `std::env::current_dir()`
//! - `router` — pre-built model router from routing config
//!
//! ## Output
//! - Printed diff (user applies manually for Day 3; auto-apply comes Day 4)
//!
//! ## Related
//! - `forge-stvp` — STVP interrogator, validators, spec compiler, token
//! - `forge-orchestrator` — STVP-gated task queue
//! - `forge-crg` — CRG subgraph extraction for Coder context

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use yantra_core::{AgentKind, ModelTier, ProjectRoot, TaskNode, TaskStatus};
use yantra_crg::{EmbeddingStore, GraphCache};
use yantra_orchestrator::Orchestrator;
use yantra_router::routing::RoutedCompletionRequest;
use yantra_router::{CompletionRequest, Message, MessageRole, Router, TaskDescription};
use yantra_stvp::{
    issue_token, run_all, Interrogator, Language, ProjectContext, Question, QuestionnaireUi,
    SigningKey, SourceTruth, SpecCompiler, StvpError, ViolationSeverity,
};

/// `QuestionnaireUi` implementation that uses `inquire::Text` for each prompt.
struct CliQuestionnaireUi;

impl QuestionnaireUi for CliQuestionnaireUi {
    fn prompt(&self, question: &Question) -> Result<String, StvpError> {
        inquire::Text::new(&question.text)
            .prompt()
            .map_err(|_| StvpError::QuestionnaireAborted)
    }
}

/// Executes the full `yantra run` pipeline for `description`.
///
/// # Errors
///
/// Returns `anyhow::Error` if any pipeline stage fails and the user does not
/// retry or abort explicitly.
pub async fn run_command(
    description: String,
    project_root: ProjectRoot,
    router: Arc<Router>,
) -> anyhow::Result<()> {
    let yantra_dir: PathBuf = project_root.as_path().join(".yantra");

    // ── Step 1: Session signing key ──────────────────────────────────────────
    let signing_key = SigningKey::load_or_generate(&yantra_dir)
        .context("failed to load or generate signing key")?;

    let interrogator = Interrogator::new(project_root.clone());
    let validation_context = ProjectContext {
        project_root: project_root.clone(),
    };

    // ── Step 2-3: STVP interrogation loop (retries on testability failures) ──
    let truth: SourceTruth = loop {
        let source_truth = interrogator
            .ask(&description, &CliQuestionnaireUi)
            .context("STVP questionnaire failed or was aborted")?;

        let validation_report = run_all(&source_truth, &validation_context);

        if validation_report.pass {
            break source_truth;
        }

        // Classify the failure so we know whether a retry can help.
        let all_violations_are_testability = validation_report
            .violations
            .iter()
            .all(|violation| violation.validator == "Testability");

        println!("\nValidation violations:");
        for violation in &validation_report.violations {
            let tag = match violation.severity {
                ViolationSeverity::Error => "[ERROR]",
                ViolationSeverity::Warning => "[WARN ]",
            };
            println!("  {tag} {}: {}", violation.validator, violation.message);
        }

        if !all_violations_are_testability {
            anyhow::bail!(
                "Validation failed with non-testability errors. \
                 Fix the task description and retry."
            );
        }

        println!(
            "\nHint: your success criterion needs measurable, testable language.\n\
             Example: \"returns HTTP 200 when the token is valid\""
        );

        let retry = inquire::Confirm::new("Refine the criterion and retry?")
            .with_default(true)
            .prompt()
            .unwrap_or(false);

        if !retry {
            anyhow::bail!("Task aborted by user.");
        }
    };

    // ── Step 4: Spec-as-Tests compilation ───────────────────────────────────
    let spec_tests_dir = project_root.as_path().join("tests");
    let generated_tests = SpecCompiler::compile(&truth, Language::Rust, &spec_tests_dir)
        .context("spec compiler failed")?;

    // ── Step 5: Preview pane ─────────────────────────────────────────────────
    println!();
    print_truth_preview(&truth);

    if !generated_tests.is_empty() {
        println!("\nGenerated spec-test skeletons:");
        for generated_test in &generated_tests {
            println!("  {}", generated_test.file_path.display());
        }
        println!("\nReview the skeletons and fill in real assertions before the run finishes.");
    }

    // ── Step 6: User confirmation ────────────────────────────────────────────
    let confirmed = inquire::Confirm::new("Proceed with this task specification?")
        .with_default(true)
        .prompt()
        .context("confirmation prompt failed")?;

    if !confirmed {
        anyhow::bail!("Task aborted by user.");
    }

    // ── Step 7: Issue truth token ────────────────────────────────────────────
    let truth_token = issue_token(&truth, &signing_key).context("failed to issue truth token")?;

    println!("\n✓ Truth token issued for task {}", truth.task_id);

    // ── Step 8: Submit to orchestrator ───────────────────────────────────────
    let task_node = TaskNode {
        id: truth.task_id,
        description: description.clone(),
        status: TaskStatus::Pending,
        class: truth.task_class,
        dependencies: Vec::new(),
        assigned_agent: Some(AgentKind::Coder),
        truth_token: Some(truth_token.clone()),
        parent_decision_id: None,
    };

    let orchestrator = Orchestrator::new(&yantra_dir);
    orchestrator
        .schedule_task(task_node.clone())
        .context("orchestrator rejected the task")?;

    println!("✓ Task {} scheduled", truth.task_id);

    // ── Step 9: CRG subgraph (best-effort) ───────────────────────────────────
    let subgraph_text = try_extract_crg_subgraph(&project_root, &description);
    if subgraph_text.is_empty() {
        println!("\n(No CRG index found — Coder will run without subgraph context.)");
    }

    // ── Step 10: Coder run ───────────────────────────────────────────────────
    println!("\nRunning Coder…");

    let coder_response =
        call_coder_via_router(&router, &truth, &subgraph_text, &description).await?;

    println!("\n{}", "─".repeat(72));
    println!("Coder output:");
    println!("{coder_response}");
    println!("{}", "─".repeat(72));
    println!("\n(Day 4: diff verification and auto-apply coming next)");

    Ok(())
}

/// Renders the `SourceTruth` artifact in a boxed ASCII preview pane.
fn print_truth_preview(truth: &SourceTruth) {
    let width = 72_usize;
    let inner = width - 2;
    let top = format!("┌─ SOURCE TRUTH {}", "─".repeat(width - 17));
    let bottom = format!("└{}", "─".repeat(width - 1));

    println!("{top}");
    println!("│ {:<inner$} │", format!("Task ID:    {}", truth.task_id));
    println!(
        "│ {:<inner$} │",
        format!("Class:      {:?}", truth.task_class)
    );
    println!(
        "│ {:<inner$} │",
        format!("Strictness: {:?}", truth.strictness)
    );
    println!("│ {:<inner$} │", "");
    println!("│ {:<inner$} │", "Description:");

    for chunk in chunk_text(&truth.description, inner.saturating_sub(4)) {
        println!("│   {:<width$} │", chunk, width = inner.saturating_sub(4));
    }

    if !truth.answers.is_empty() {
        println!("│ {:<inner$} │", "");
        println!("│ {:<inner$} │", "Answers:");
        for (question_key, answer_value) in &truth.answers {
            let header = format!("  {question_key}:");
            println!("│ {header:<inner$} │");
            for chunk in chunk_text(answer_value, inner.saturating_sub(6)) {
                println!("│     {:<width$} │", chunk, width = inner.saturating_sub(6));
            }
        }
    }

    println!("{bottom}");
}

fn chunk_text(text: &str, width: usize) -> Vec<&str> {
    if width == 0 {
        return vec![text];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + width).min(text.len());
        chunks.push(&text[start..end]);
        start = end;
    }
    if chunks.is_empty() {
        chunks.push("");
    }
    chunks
}

/// Attempts to extract a CRG subgraph for `query` from the local index.
///
/// Returns an empty string when the CRG database or cache does not exist.
fn try_extract_crg_subgraph(project_root: &ProjectRoot, query: &str) -> String {
    let crg_db_path = project_root.as_path().join(".yantra").join("crg.sqlite");
    let crg_cache_path = project_root
        .as_path()
        .join(".yantra")
        .join("crg_cache.json");

    if !crg_db_path.exists() {
        return String::new();
    }

    let database_connection = match rusqlite::Connection::open(&crg_db_path) {
        Ok(connection) => connection,
        Err(_) => return String::new(),
    };

    let graph_cache = if crg_cache_path.exists() {
        std::fs::read_to_string(&crg_cache_path)
            .ok()
            .and_then(|text| serde_json::from_str::<GraphCache>(&text).ok())
            .unwrap_or_else(|| {
                GraphCache::build(&database_connection)
                    .unwrap_or_else(|_| GraphCache::build(&database_connection).unwrap())
            })
    } else {
        match GraphCache::build(&database_connection) {
            Ok(cache) => cache,
            Err(_) => return String::new(),
        }
    };

    let embedding_store = EmbeddingStore::new().unwrap_or_else(|_| EmbeddingStore::new().unwrap());
    let _ = embedding_store.load_vectors_from_db(&database_connection);

    yantra_crg::extract_subgraph(&graph_cache, &embedding_store, query, 4096, &[])
        .map(|subgraph| subgraph.text)
        .unwrap_or_default()
}

/// Calls the model router with a Coder-formatted prompt for `description`.
async fn call_coder_via_router(
    router: &Router,
    truth: &SourceTruth,
    subgraph_text: &str,
    description: &str,
) -> anyhow::Result<String> {
    let truth_summary = format!(
        "Task ID: {}\nClass: {:?}\nStrictness: {:?}",
        truth.task_id, truth.task_class, truth.strictness
    );

    let user_content = format!(
        "## Source Truth\n{truth_summary}\n\n\
         ## Code Context (CRG Subgraph)\n{subgraph_text}\n\n\
         ## Task\n{description}\n\n\
         Provide your changes as unified diffs, \
         each preceded by a `// File: <relative/path>` comment."
    );

    let system_prompt = std::fs::read_to_string("crates/forge-agents/prompts/coder.md")
        .unwrap_or_else(|_| {
            "You are a Rust engineer. Write minimal, correct diffs for the task.".to_owned()
        });

    let messages = vec![
        Message {
            role: MessageRole::System,
            content: system_prompt,
            tool_calls: Vec::new(),
        },
        Message {
            role: MessageRole::User,
            content: user_content,
            tool_calls: Vec::new(),
        },
    ];

    let completion_request = CompletionRequest {
        messages,
        max_tokens: Some(2048),
        temperature: 0.2,
        tools: None,
        stop_sequences: Vec::new(),
    };

    let task_desc = TaskDescription {
        description: description.to_owned(),
        class: truth.task_class,
        tokens_estimated: 2048,
        tool_calls_predicted: 0,
        touches_sacred_files: false,
        multi_file: false,
    };

    let mut routed_request = RoutedCompletionRequest {
        required_tier: router.policy().classify(&task_desc),
        completion_request: completion_request.clone(),
    };

    let provider = router
        .route(&routed_request)
        .or_else(|_| {
            routed_request.required_tier = ModelTier::Tier1;
            router.route(&routed_request)
        })
        .or_else(|_| {
            routed_request.required_tier = ModelTier::Tier2;
            router.route(&routed_request)
        })
        .or_else(|_| {
            routed_request.required_tier = ModelTier::Tier3;
            router.route(&routed_request)
        })
        .context("no model provider available — check configs/routing.toml")?;

    let response = provider
        .complete(routed_request.completion_request)
        .await
        .context("model provider returned an error")?;

    Ok(response.content)
}
