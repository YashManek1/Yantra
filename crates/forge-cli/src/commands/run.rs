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

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use futures_util::StreamExt;
use yantra_agents::{
    parse_diffs_from_response, Agent, CoderAgent, CommitSigningKey, CommitterAgent, RedTeamAgent,
    ResearcherAgent, VerifierAgent,
};
use yantra_core::{AgentKind, ProjectRoot, SessionId, TaskNode, TaskStatus, WorkspaceMode};
use yantra_crg::{EmbeddingStore, GraphCache};
use yantra_orchestrator::{CircuitBreaker, EventBus, Orchestrator, Scheduler, TaskDag};
use yantra_router::routing::RoutedCompletionRequest;
use yantra_router::{CompletionRequest, Message, MessageRole, Router, TaskDescription};
use yantra_stvp::{
    issue_token, run_all, Interrogator, Language, ProjectContext, Question, QuestionnaireUi,
    SigningKey, SourceTruth, SpecCompiler, StvpError, ViolationSeverity,
};
use yantra_tools::apply_diff_to_file;
use yantra_verifier::{
    Diff as VerifierDiff, VerificationContext, VerificationPipeline, VerificationResult,
};

/// `QuestionnaireUi` implementation that uses `inquire::Text` for each prompt.
struct CliQuestionnaireUi;

impl QuestionnaireUi for CliQuestionnaireUi {
    fn prompt(&self, question: &Question) -> Result<String, StvpError> {
        let mut text_prompt = inquire::Text::new(&question.text);
        if let Some(ref placeholder) = question.suggested_answer {
            text_prompt = text_prompt.with_placeholder(placeholder);
        }
        if let Some(ref help) = question.help_text {
            text_prompt = text_prompt.with_help_message(help);
        }
        text_prompt
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

    let augmenter = Arc::new(crate::augmenter::CliQuestionAugmenter::new(router.clone()));
    let interrogator = Interrogator::new(project_root.clone()).with_augmenter(augmenter);
    let validation_context = ProjectContext {
        project_root: project_root.clone(),
    };

    // ── Step 2-3: STVP interrogation loop (retries on testability failures) ──
    let truth: SourceTruth = loop {
        let source_truth = interrogator
            .ask(&description, &CliQuestionnaireUi)
            .await
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

    let is_complex = truth.task_class == yantra_core::TaskClass::NewFeature
        || truth.task_class == yantra_core::TaskClass::Refactor
        || truth
            .answers
            .get("sacred_files")
            .is_some_and(|list| !list.trim().is_empty());

    if is_complex {
        println!(
            "\nComplex or Sacred task class ({:?}) detected.",
            truth.task_class
        );
        println!("Initializing Multi-Agent DAG scheduler...");

        let dag = Arc::new(TaskDag::open(&yantra_dir)?);
        dag.clear()?;

        let session_id = SessionId::from_uuid(truth.task_id.as_uuid());
        let event_bus = EventBus::open(&yantra_dir, session_id)?;
        let circuit_breaker = Arc::new(CircuitBreaker::new(100));

        let mut agents: std::collections::HashMap<AgentKind, Arc<dyn Agent>> =
            std::collections::HashMap::new();
        agents.insert(AgentKind::Researcher, Arc::new(ResearcherAgent::new()));
        agents.insert(AgentKind::Coder, Arc::new(CoderAgent::new()));
        agents.insert(AgentKind::IntegrityChecker, Arc::new(VerifierAgent::new()));
        agents.insert(AgentKind::RedTeam, Arc::new(RedTeamAgent::new()));

        let signing_key_commit =
            CommitSigningKey::generate().context("failed to generate commit key")?;
        agents.insert(
            AgentKind::Committer,
            Arc::new(CommitterAgent::new(signing_key_commit)),
        );

        let db_path = yantra_dir.join("memory.sqlite");
        let memory_service = Arc::new(yantra_memory::MemoryService::new(&db_path)?);

        let mut mcp_router = yantra_tools::McpRouter::new([
            "crg.subgraph".to_owned(),
            "fs.read_file".to_owned(),
            "fs.write_file".to_owned(),
            "fs.apply_diff".to_owned(),
            "git.commit".to_owned(),
            "git.log".to_owned(),
            "git.status".to_owned(),
        ]);

        let fs_server = Arc::new(yantra_tools::FsMcpServer::new(project_root.clone()));
        mcp_router.register_server(fs_server);

        let git_server = Arc::new(yantra_tools::GitMcpServer::new(project_root.clone()));
        mcp_router.register_server(git_server);

        let lsp_bridge = yantra_lsp::LspBridge::new(project_root.as_path());
        let lsp_server = Arc::new(yantra_tools::LspMcpServer::new(lsp_bridge));
        mcp_router.register_server(lsp_server);

        if let Ok(connection) = rusqlite::Connection::open(yantra_dir.join("crg.sqlite")) {
            let embedding_store = yantra_crg::EmbeddingStore::new().ok();
            let graph_cache = yantra_crg::GraphCache::build(&connection).ok();
            if let (Some(store), Some(cache)) = (embedding_store, graph_cache) {
                let _ = store.load_vectors_from_db(&connection);
                let crg_server =
                    Arc::new(yantra_tools::CrgMcpServer::new(connection, store, cache));
                mcp_router.register_server(crg_server);
            }
        }

        let workspace_mode = WorkspaceMode::detect(project_root.as_path());
        let (scheduler, _review_receiver) = Scheduler::new(
            dag.clone(),
            agents,
            event_bus.clone(),
            circuit_breaker,
            router.clone(),
            mcp_router,
            session_id,
            std::time::Duration::from_millis(50),
            memory_service.clone(),
            workspace_mode,
        );

        let researcher_task_id = yantra_core::TaskId::new();
        let researcher_node = TaskNode {
            id: researcher_task_id,
            description: format!("Research the implementation details of: {description}"),
            status: TaskStatus::Pending,
            class: truth.task_class,
            dependencies: Vec::new(),
            assigned_agent: Some(AgentKind::Researcher),
            truth_token: Some(truth_token.clone()),
            parent_decision_id: None,
        };

        let coder_task_id = yantra_core::TaskId::new();
        let coder_node = TaskNode {
            id: coder_task_id,
            description: description.clone(),
            status: TaskStatus::Pending,
            class: truth.task_class,
            dependencies: vec![researcher_task_id],
            assigned_agent: Some(AgentKind::Coder),
            truth_token: Some(truth_token.clone()),
            parent_decision_id: None,
        };

        let verifier_task_id = yantra_core::TaskId::new();
        let verifier_node = TaskNode {
            id: verifier_task_id,
            description: format!("Verify changes from coder task {coder_task_id}"),
            status: TaskStatus::Pending,
            class: truth.task_class,
            dependencies: vec![coder_task_id],
            assigned_agent: Some(AgentKind::IntegrityChecker),
            truth_token: Some(truth_token.clone()),
            parent_decision_id: None,
        };

        let red_team_task_id = yantra_core::TaskId::new();
        let red_team_node = TaskNode {
            id: red_team_task_id,
            description: format!(
                "Perform security and robustness audit on changes from coder task {coder_task_id}"
            ),
            status: TaskStatus::Pending,
            class: truth.task_class,
            dependencies: vec![verifier_task_id],
            assigned_agent: Some(AgentKind::RedTeam),
            truth_token: Some(truth_token.clone()),
            parent_decision_id: None,
        };

        let committer_task_id = yantra_core::TaskId::new();
        let committer_node = TaskNode {
            id: committer_task_id,
            description: format!(
                "Commit successfully verified changes from coder task {coder_task_id}"
            ),
            status: TaskStatus::Pending,
            class: truth.task_class,
            dependencies: vec![red_team_task_id],
            assigned_agent: Some(AgentKind::Committer),
            truth_token: Some(truth_token.clone()),
            parent_decision_id: None,
        };

        scheduler.register_task(researcher_node)?;
        scheduler.register_task(coder_node)?;
        scheduler.register_task(verifier_node)?;
        scheduler.register_task(red_team_node)?;
        scheduler.register_task(committer_node)?;

        println!(
            "✓ Multi-Agent DAG registered: Researcher -> Coder -> Verifier -> RedTeam -> Committer"
        );
        println!("Running DAG scheduler to completion...");

        let results = scheduler
            .run_to_completion()
            .await
            .context("DAG scheduler run failed")?;
        println!("✓ Multi-Agent DAG finished execution successfully!");
        for result in results {
            println!("  - {}: {}", result.task_id, result.summary);
        }
        return Ok(());
    }

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

    // ── Step 10: Coder run with closed-loop verification & commit ─────────────
    let is_greenfield = WorkspaceMode::detect(project_root.as_path()) == WorkspaceMode::Greenfield;
    if is_greenfield {
        println!(
            "(Greenfield Scaffolding Mode active — workspace is empty or has minimal symbols)"
        );
    }

    let db_path = project_root.as_path().join(".yantra").join("memory.sqlite");
    let mut memory_context = String::new();
    if let Ok(memory_service) = yantra_memory::MemoryService::new(&db_path) {
        if let Ok(assembled) = memory_service.assemble_agent_context(
            uuid::Uuid::new_v4(),
            truth.task_class,
            &description,
            4096,
        ) {
            memory_context = assembled;
        }
    }

    let mut retry_count = 0;
    let mut feedback_context: Option<String> = None;

    loop {
        if retry_count > 0 {
            println!("\nRetrying Coder agent (attempt {}/3)...", retry_count + 1);
        } else {
            println!("\nRunning Coder…");
        }

        let coder_response = call_coder_via_router(
            &router,
            &truth,
            &subgraph_text,
            &description,
            is_greenfield,
            feedback_context.as_deref(),
            &memory_context,
        )
        .await?;

        println!("\n{}", "─".repeat(72));
        println!("Coder output (attempt {}):", retry_count + 1);
        println!("{coder_response}");
        println!("{}", "─".repeat(72));

        println!("\nParsing diffs from response...");
        let file_diffs = parse_diffs_from_response(&coder_response);
        if file_diffs.is_empty() {
            println!("No valid diffs or file creations found in the response.");
            if retry_count >= 2 {
                if let Ok(memory_service) = yantra_memory::MemoryService::new(&db_path) {
                    let record = yantra_memory::MistakeRecord::new(
                        truth.task_class,
                        "parsing failed",
                        "no valid diff blocks found in coder response",
                        "the model did not output code blocks matching standard prefix",
                    );
                    let _ = memory_service.mistake_library().record_failure(&record);
                }
                anyhow::bail!(
                    "Verification failed: max retries reached and no diffs could be parsed."
                );
            }
            feedback_context = Some("Error: Your response did not contain any valid '// File: <path>' or '// Create File: <path>' blocks. Ensure you prefix each code block with the correct path annotation.".to_owned());
            retry_count += 1;
            continue;
        }

        println!("Applying diffs to disk...");
        let mut apply_error: Option<String> = None;
        for file_diff in &file_diffs {
            let canonical_path = match yantra_core::canonicalize_within(
                &project_root,
                std::path::Path::new(&file_diff.file_path),
            ) {
                Ok(path) => path,
                Err(err) => {
                    apply_error = Some(format!(
                        "Path traversal detected or invalid path for {}: {}",
                        file_diff.file_path, err
                    ));
                    break;
                }
            };
            if let Err(err) = apply_diff_to_file(&canonical_path, &file_diff.unified_diff) {
                apply_error = Some(format!(
                    "Failed to apply diff for {}: {}",
                    file_diff.file_path, err
                ));
                break;
            }
        }

        if let Some(err) = apply_error {
            println!("Diff application failed: {err}");
            if retry_count >= 2 {
                if let Ok(memory_service) = yantra_memory::MemoryService::new(&db_path) {
                    let record = yantra_memory::MistakeRecord::new(
                        truth.task_class,
                        "diff application failed",
                        err.clone(),
                        "hunk context match failed or file write permission error",
                    );
                    let _ = memory_service.mistake_library().record_failure(&record);
                }
                anyhow::bail!(
                    "Verification failed: max retries reached. Diff application error: {err}"
                );
            }
            feedback_context = Some(format!("Error: Diff application failed on disk:\n{err}\nPlease check file structures and produce standard unified diff hunks matching exact context lines."));
            retry_count += 1;
            continue;
        }

        let mut diff_text = String::new();
        for file_diff in &file_diffs {
            if is_greenfield {
                diff_text.push_str(&format!(
                    "// Create File: {}\n{}\n",
                    file_diff.file_path, file_diff.unified_diff
                ));
            } else {
                diff_text.push_str(&format!(
                    "// File: {}\n{}\n",
                    file_diff.file_path, file_diff.unified_diff
                ));
            }
        }

        let verifier_diff = VerifierDiff {
            text: diff_text,
            task_class: truth.task_class,
        };

        let verifier_ctx = VerificationContext {
            project_root: project_root.as_path().to_path_buf(),
            is_sacred_diff: truth
                .answers
                .get("sacred_files")
                .is_some_and(|list| !list.trim().is_empty()),
            crg_db_path: Some(project_root.as_path().join(".yantra").join("crg.sqlite")),
        };

        println!("Running verification pipeline...");
        let verify_result =
            VerificationPipeline::verify(&verifier_diff, &truth, &verifier_ctx, retry_count)
                .await?;

        match verify_result {
            VerificationResult::Pass => {
                println!("\n✓ Verification passed!");

                println!("Committing changes...");
                let git_server = yantra_tools::GitMcpServer::new(project_root.clone());
                let commit_subject = format!(
                    "feat: {}",
                    description.lines().next().unwrap_or("automated task")
                );
                let commit_body = format!(
                    "{}\n\nTask ID: {}\nOutcome: Success\n",
                    commit_subject, truth.task_id
                );
                let commit_res = git_server.commit(&serde_json::json!({
                    "message": commit_body,
                }));
                match commit_res {
                    Ok(val) => {
                        let hash = val
                            .get("hash")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        println!("✓ Created signed commit: {hash}");
                    }
                    Err(e) => {
                        println!("⚠️ Failed to create commit: {}", e.message);
                    }
                }

                if let Ok(memory_service) = yantra_memory::MemoryService::new(&db_path) {
                    let turn_id = uuid::Uuid::new_v4().to_string();
                    let session_id_str = truth.task_id.to_string();
                    let _ = memory_service
                        .recall()
                        .record_turn(&yantra_memory::ConversationTurn {
                            id: turn_id,
                            session_id: session_id_str.clone(),
                            timestamp: chrono::Utc::now(),
                            role: "assistant".to_owned(),
                            content: coder_response.clone(),
                            summary: Some(format!("Completed task: {description}")),
                            tokens: None,
                        });
                    let _ = memory_service.recall().summarize_session(
                        &session_id_str,
                        &format!("Successfully completed task: {description}"),
                        Some(&format!("Accomplished: {description}")),
                        None,
                        None,
                    );
                }

                break;
            }
            VerificationResult::Reject { stage, violations } => {
                println!("⚠️ Verification rejected at stage {stage:?}");
                for violation in &violations {
                    println!(
                        "  - [{:?}] in {:?}: {}",
                        violation.severity, violation.file_path, violation.message
                    );
                }

                if retry_count >= 2 {
                    if let Ok(memory_service) = yantra_memory::MemoryService::new(&db_path) {
                        let violation_messages: Vec<String> =
                            violations.iter().map(|v| v.message.clone()).collect();
                        let record = yantra_memory::MistakeRecord::new(
                            truth.task_class,
                            format!("verification rejected at {stage:?}"),
                            violation_messages.join("; "),
                            "code changes violated truth constraints, static analysis checks, or failed test suites",
                        );
                        let _ = memory_service.mistake_library().record_failure(&record);
                    }
                    anyhow::bail!("Verification failed: max retries reached without passing.");
                }

                let mut feedback = format!(
                    "Your previous changes were rejected at stage {stage:?}.\nViolations:\n"
                );
                for violation in &violations {
                    feedback.push_str(&format!("- {}\n", violation.message));
                }
                feedback_context = Some(feedback);
                retry_count += 1;
            }
            VerificationResult::NeedHumanReview { reason } => {
                println!("🚨 Escalating to human review: {reason}");
                anyhow::bail!("Verification failed: need human review.");
            }
        }
    }

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
    is_greenfield: bool,
    feedback_context: Option<&str>,
    memory_context: &str,
) -> anyhow::Result<String> {
    let mut truth_summary = format!(
        "Task ID:     {}\nClass:       {:?}\nStrictness:  {:?}\nDescription: {}",
        truth.task_id, truth.task_class, truth.strictness, truth.description
    );
    for (answer_key, answer_value) in &truth.answers {
        if !answer_value.trim().is_empty() {
            truth_summary.push_str(&format!("\n{answer_key}: {answer_value}"));
        }
    }

    let mut user_content = if is_greenfield {
        format!(
            "## Source Truth\n{truth_summary}\n\n\
             ## Task\n{description}\n\n\
             ## Greenfield Scaffolding Mode Active\n\
             The workspace is currently EMPTY or has no symbols. Do NOT look for existing code or try to write a diff on existing functions.\n\
             Instead, design the architecture and create the project files (e.g. Cargo.toml, src/main.rs) as COMPLETE new files.\n\
             Follow the two-phase protocol from your system prompt. \
             Phase 1: list zero grounded symbols (GROUNDED SYMBOLS: none) and specify 'INSERTION POINT: none'.\n\
             Phase 2: produce full-file creation blocks prefixed with '// Create File: <relative/path>'."
        )
    } else {
        format!(
            "## Source Truth\n{truth_summary}\n\n\
             ## Code Context (CRG Subgraph)\n{subgraph_text}\n\n\
             ## Task\n{description}\n\n\
             Follow the two-phase protocol from your system prompt. \
             Phase 1: extract grounded symbols. Phase 2: produce unified diffs."
        )
    };

    if !memory_context.is_empty() {
        user_content.push_str(&format!("\n\n{memory_context}"));
    }

    if let Some(feedback) = feedback_context {
        user_content.push_str(&format!(
            "\n\n## Verification Feedback (Fix these violations from your previous attempt):\n{feedback}"
        ));
    }

    let system_prompt = std::fs::read_to_string("crates/forge-agents/prompts/coder.md")
        .unwrap_or_else(|_| {
            "You are a Rust engineer. Write minimal, correct diffs for the task.".to_owned()
        });

    let touches_sacred_files = truth
        .answers
        .get("sacred_files")
        .is_some_and(|sacred_files_list| !sacred_files_list.trim().is_empty());

    let prompt_tokens = yantra_tokenizer::count_tokens(&user_content)
        + yantra_tokenizer::count_tokens(&system_prompt);
    let tokens_estimated = prompt_tokens + 1024;

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
        tokens_estimated,
        tool_calls_predicted: 0,
        touches_sacred_files,
        multi_file: false,
    };

    let routed_request = RoutedCompletionRequest {
        required_tier: router.policy().classify(&task_desc),
        completion_request: completion_request.clone(),
    };

    let provider = router
        .route(&routed_request)
        .await
        .context("no model provider available — check configs/routing.toml")?;

    let mut stream = provider
        .complete_stream(routed_request.completion_request)
        .await
        .context("model provider returned an error")?;

    let mut accumulated_content = String::new();
    while let Some(token_result) = stream.next().await {
        let token = token_result.context("failed to stream token")?;
        print!("{token}");
        let _ = std::io::stdout().flush();
        accumulated_content.push_str(&token);
    }
    println!();

    Ok(accumulated_content)
}
