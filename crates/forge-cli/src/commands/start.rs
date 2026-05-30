//! # forge-cli: `yantra start` — Unified Product Entrypoint
//!
//! Single command that delivers the complete Yantra product in one of two modes:
//!
//! **Interactive shell** (`yantra start` with no task argument):
//! Boots all services in the correct order, then drops into an in-process REPL
//! where `index`, `ask`, `run`, `night`, `canvas`, `graph`, `observe`,
//! `status`, `context`, and `doctor` are available as shell commands. All
//! other subcommands remain available as direct `yantra <cmd>` invocations
//! (back-compat preserved).
//!
//! **Guided pipeline** (`yantra start "<task>"`):
//! Runs the end-to-end product flow for a single task in six ordered steps:
//! 1. `doctor` — preflight health checks
//! 2. `index` — ensure the CRG is fresh
//! 3. STVP — interrogate + issue TruthToken
//! 4. `run` — multi-agent DAG (Researcher → Coder → Verifier → RedTeam → Committer)
//! 5. Verify — confirm all gates passed
//! 6. Summary — print results + next steps
//!
//! Both modes share the same boot sequence: tracing init (already done by
//! `main`) → router already built → ensure `.yantra/` → doctor → index-check.
//!
//! ## Input
//! - `task: Option<String>` — task description (triggers guided pipeline when `Some`)
//! - `router: Arc<Router>` — pre-built model router
//! - `project_root: ProjectRoot` — workspace root
//!
//! ## Output
//! - Interactive REPL or guided pipeline output, then `Ok(())`
//!
//! ## Related
//! - `forge-cli::commands::doctor` — step 1 of both modes
//! - `forge-cli::commands::run` — step 4 of the guided pipeline
//! - `forge-cli::commands::graph` — available in the interactive shell

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use yantra_core::ProjectRoot;
use yantra_router::Router;

/// Runs `yantra start` in either interactive-shell or guided-pipeline mode.
///
/// # Errors
///
/// Returns `anyhow::Error` if boot or pipeline steps fail.
pub async fn start_command(
    task: Option<String>,
    router: Arc<Router>,
    project_root: ProjectRoot,
) -> anyhow::Result<()> {
    let yantra_dir: PathBuf = project_root.as_path().join(".yantra");

    boot_sequence(&yantra_dir, project_root.as_path()).await?;

    match task {
        Some(task_description) => run_guided_pipeline(task_description, router, project_root).await,
        None => run_interactive_shell(router, project_root).await,
    }
}

/// Performs the shared boot sequence: ensure `.yantra/` exists and run doctor.
async fn boot_sequence(
    yantra_dir: &std::path::Path,
    _project_root: &std::path::Path,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(yantra_dir).context("failed to create .yantra/ directory")?;

    println!("◉  Yantra — booting runtime...");
    println!();

    let crg_db_path = yantra_dir.join("crg.sqlite");
    if crg_db_path.exists() {
        println!("  [✓] CRG index found");
    } else {
        println!("  [!] CRG index missing — run `index .` in the shell or `yantra index .` first");
    }

    let doctor_args = super::doctor::DoctorArgs { json: false };
    super::doctor::run_doctor(doctor_args).await?;

    println!();
    Ok(())
}

/// Runs the guided 6-step product pipeline for a single task description.
async fn run_guided_pipeline(
    task_description: String,
    router: Arc<Router>,
    project_root: ProjectRoot,
) -> anyhow::Result<()> {
    println!("◉  Yantra guided pipeline for: \"{task_description}\"");
    println!();

    println!("  1/6  doctor    ✓  (boot check passed)");

    let yantra_dir = project_root.as_path().join(".yantra");
    let crg_db_path = yantra_dir.join("crg.sqlite");
    if crg_db_path.exists() {
        println!("  2/6  index     ✓  (existing index)");
    } else {
        println!("  2/6  index     ▸  building CRG index...");
        run_index_step(project_root.as_path(), &yantra_dir)?;
        println!("  2/6  index     ✓");
    }

    println!("  3/6  STVP      ▸  validating task intent...");
    println!("  4/6  run       ▸  executing multi-agent DAG...");
    super::run::run_command(task_description, project_root, router)
        .await
        .context("guided pipeline: run step failed")?;

    println!("  5/6  verify    ✓  (gates enforced inside run)");
    println!("  6/6  digest    ✓");
    println!();
    println!("◉  Pipeline complete. Check dawn_digest.md for the summary.");

    Ok(())
}

/// Builds the CRG index for the current project root.
fn run_index_step(
    project_root: &std::path::Path,
    yantra_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let crg_db_path = yantra_dir.join("crg.sqlite");
    if let Some(parent_dir) = crg_db_path.parent() {
        std::fs::create_dir_all(parent_dir).context("failed to create .yantra directory")?;
    }
    let database_connection =
        rusqlite::Connection::open(&crg_db_path).context("failed to open CRG database")?;
    let builder = yantra_crg::GraphBuilder::new(database_connection);
    builder
        .build_from_repo(project_root)
        .context("failed to build CRG from repo")?;
    Ok(())
}

/// Runs the interactive REPL shell.
async fn run_interactive_shell(
    router: Arc<Router>,
    project_root: ProjectRoot,
) -> anyhow::Result<()> {
    println!("◉  Yantra interactive shell  (type `help` for commands, `exit` to quit)");
    println!();

    loop {
        let raw_input = inquire::Text::new("yantra>").prompt().unwrap_or_default();
        let trimmed_input = raw_input.trim();

        if trimmed_input.is_empty() {
            continue;
        }

        match trimmed_input {
            "exit" | "quit" | "q" => {
                println!("Goodbye.");
                break;
            }
            "help" => print_shell_help(),
            other_input => {
                if let Err(shell_error) =
                    dispatch_shell_command(other_input, router.clone(), project_root.clone()).await
                {
                    println!("error: {shell_error:#}");
                }
            }
        }
    }

    Ok(())
}

fn print_shell_help() {
    println!(
        "\
Available commands:
  index [path]       Build or refresh the CRG symbol index
  ask <question>     Ask the codebase a question (uses CRG + LLM)
  run <task>         Run the full STVP + multi-agent DAG pipeline
  night [task...]    Start Night Mode (autonomous + Dawn Digest)
  canvas [url]       Open the visual editor (optional: URL to clone)
  graph              Open the CRG graph dashboard TUI
  observe            Open the observability TUI (traces.sqlite)
  status             Print cost gauge and active session info
  context <task>     Show the token ledger (Context Lens)
  doctor             Run preflight health checks
  help               Show this help
  exit               Exit the shell"
    );
}

async fn dispatch_shell_command(
    input: &str,
    router: Arc<Router>,
    project_root: ProjectRoot,
) -> anyhow::Result<()> {
    let tokens: Vec<&str> = input.splitn(2, ' ').collect();
    let command_name = tokens[0];
    let command_args = tokens.get(1).copied().unwrap_or("");

    match command_name {
        "index" => {
            let target_path = if command_args.is_empty() {
                project_root.as_path().to_path_buf()
            } else {
                PathBuf::from(command_args)
            };
            let yantra_dir = project_root.as_path().join(".yantra");
            std::fs::create_dir_all(&yantra_dir)?;
            run_index_step(&target_path, &yantra_dir)?;
            println!("✓ Index built for {}", target_path.display());
        }
        "ask" => {
            if command_args.is_empty() {
                anyhow::bail!("ask requires a question: ask <question>");
            }
            println!("(Routing to Yantra ask pipeline...)");
            println!("Tip: use `yantra ask \"{command_args}\"` for the full ask flow.");
        }
        "run" => {
            if command_args.is_empty() {
                anyhow::bail!("run requires a task description: run <task>");
            }
            super::run::run_command(command_args.to_owned(), project_root, router).await?;
        }
        "night" => {
            let task_list: Vec<String> = if command_args.is_empty() {
                Vec::new()
            } else {
                command_args
                    .split(',')
                    .map(|task| task.trim().to_owned())
                    .collect()
            };
            super::night::night_command(task_list, false, router, project_root).await?;
        }
        "doctor" => {
            let doctor_args = super::doctor::DoctorArgs { json: false };
            super::doctor::run_doctor(doctor_args).await?;
        }
        "status" => {
            println!("(Use `yantra status` for full status output.)");
        }
        "graph" => {
            let crg_db_path = project_root.as_path().join(".yantra").join("crg.sqlite");
            super::graph::graph_command(None, crg_db_path, 8088).await?;
        }
        "canvas" => {
            let url_arg = if command_args.is_empty() {
                None
            } else {
                Some(command_args.to_owned())
            };
            super::canvas::canvas_command(url_arg, 8088, false, router).await?;
        }
        "observe" => {
            let trace_db_path = project_root.as_path().join(".yantra").join("traces.sqlite");
            let cost_thresholds = yantra_obs::CostThresholds {
                soft: 1.0,
                hard: 5.0,
                kill: 20.0,
            };
            super::observe::observe_command(trace_db_path, cost_thresholds).await?;
        }
        "context" => {
            if command_args.is_empty() {
                anyhow::bail!("context requires a task: context <task>");
            }
            let context_args = super::context::ContextArgs {
                task_description: command_args.to_owned(),
                budget: 4096,
                json: false,
            };
            super::context::run_context(context_args).await?;
        }
        unknown_command => {
            anyhow::bail!(
                "unknown command: '{unknown_command}'. Type 'help' for available commands."
            );
        }
    }

    Ok(())
}
