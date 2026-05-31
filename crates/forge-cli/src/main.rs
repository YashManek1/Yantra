//! # forge-cli: Yantra Command-Line Interface
//!
//! Entry point for the `yantra` binary. Uses clap for argument parsing and
//! ratatui for the interactive terminal UI. Delegates all runtime logic to
//! `forge-orchestrator`, `forge-night`, and `forge-canvas`.
//!
//! **Recommended entry point:** `yantra start` — launches the Yantra Console, a
//! unified TUI with a streaming conversation pane, a live CRG graph side panel,
//! and a real-time telemetry footer. All other subcommands remain available for
//! direct power-user invocation.
//!
//! ## Input
//! - CLI arguments: subcommand + flags
//! - Interactive terminal input during STVP questionnaires
//!
//! ## Output
//! - Terminal UI rendered via ratatui
//! - Exit code 0 on success, non-zero on task failure or user abort
//!
//! ## Related
//! - `forge-orchestrator` — receives task submissions
//! - `forge-night` — autonomous Night Mode with Dawn Digest
//! - `forge-canvas` — visual canvas editor and CRG graph viewer
//! - `forge-cli::commands::start` — unified product entrypoint

mod approval;
mod ask_verifier;
mod augmenter;
mod commands;
mod diff_preview;
mod tui;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use clap::{Parser, Subcommand};
use serde::Deserialize;
use yantra_core::{ModelCapability, ModelTier, ProjectRoot, SessionId};
use yantra_obs::{init_tracing, CostThresholds, TracingConfig};
use yantra_router::{
    CompletionRequest, CompletionResponse, GitHubModelsProvider, ModelProvider, OllamaProvider,
    OpenRouterProvider, ProviderError, ProviderStatus, Router, RoutingPolicy,
};

#[derive(Debug, Deserialize)]
struct RoutingConfig {
    providers: ProvidersConfig,
    budget: BudgetConfig,
}

#[derive(Debug, Deserialize)]
struct ProvidersConfig {
    #[serde(default)]
    tier0: Vec<ProviderConfigEntry>,
    #[serde(default)]
    tier1: Vec<ProviderConfigEntry>,
    #[serde(default)]
    tier2: Vec<ProviderConfigEntry>,
    #[serde(default)]
    tier3: Vec<ProviderConfigEntry>,
}

#[derive(Debug, Deserialize)]
struct ProviderConfigEntry {
    kind: String,
    base_url: Option<String>,
    default_model: String,
}

#[derive(Debug, Deserialize)]
struct BudgetConfig {
    soft_usd: f32,
    hard_usd: f32,
    kill_usd: f32,
}

/// Serializable snapshot of the most recent session's status, used by `yantra status`.
#[derive(Debug, serde::Serialize)]
struct StatusSnapshot {
    session_id: Option<String>,
    total_spans: i64,
    cumulative_cost_usd: f64,
    status: String,
}

impl StatusSnapshot {
    /// Constructs an empty snapshot (no sessions found) with status `"Ok"`.
    fn empty(cost_thresholds: &CostThresholds) -> Self {
        Self {
            session_id: None,
            total_spans: 0,
            cumulative_cost_usd: 0.0,
            status: classify_cost_status(0.0, cost_thresholds).to_string(),
        }
    }
}

/// Classifies a cumulative cost value into a human-readable status label.
fn classify_cost_status(
    cumulative_cost_usd: f64,
    cost_thresholds: &CostThresholds,
) -> &'static str {
    if cumulative_cost_usd >= f64::from(cost_thresholds.kill) {
        "Kill"
    } else if cumulative_cost_usd >= f64::from(cost_thresholds.hard) {
        "Pause"
    } else if cumulative_cost_usd >= f64::from(cost_thresholds.soft) {
        "Warn"
    } else {
        "Ok"
    }
}

/// Prints the status snapshot either as JSON or as a plain-text table.
///
/// # Errors
///
/// Returns `anyhow::Error` if JSON serialization fails.
fn print_status_snapshot(snapshot: &StatusSnapshot, as_json: bool) -> anyhow::Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(snapshot)?);
        return Ok(());
    }

    match &snapshot.session_id {
        Some(session_id) => {
            println!("Active Session: {session_id}");
            println!("Total Spans: {}", snapshot.total_spans);
        }
        None => {
            println!("No active sessions found.");
        }
    }
    println!("Cumulative Cost: ${:.6}", snapshot.cumulative_cost_usd);
    println!("Status: {}", snapshot.status);
    Ok(())
}

#[derive(Parser)]
#[command(
    name = "yantra",
    version = "0.1.0",
    about = "Yantra: Agentic Coding Runtime"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Builds the symbol index for the given path
    Index {
        /// Optional path to index (defaults to current directory)
        path: Option<String>,
    },
    /// Sends a question to the routed LLM
    Ask {
        /// The question to send to the routed LLM
        question: String,
    },
    /// Runs the full STVP pipeline for a task description
    Run {
        /// Natural-language task description
        description: String,
    },
    /// Opens the browser-based visual canvas editor for a website
    Canvas {
        /// Optional URL to clone into an editable React + Tailwind project
        url: Option<String>,
        /// Port for the local canvas server
        #[arg(long, default_value_t = 8088)]
        port: u16,
        /// Use headless Chromium to execute JavaScript before cloning
        /// (requires the `render-js` feature to be compiled in)
        #[arg(long, default_value_t = false)]
        render_js: bool,
    },
    /// Opens the CRG dashboard TUI (press `g` for the browser graph viewer)
    Graph {
        /// Optional symbol id to focus the browser graph viewer on
        #[arg(long)]
        focus: Option<String>,
        /// Port for the local graph viewer server
        #[arg(long, default_value_t = 8088)]
        port: u16,
    },
    /// Opens the live observability TUI backed by `.yantra/traces.sqlite`
    Observe,
    /// Prints cost gauge and active session info
    Status {
        /// Output as JSON instead of a formatted table.
        #[arg(long)]
        json: bool,
    },
    /// Shows the token ledger for a task description (Context Lens)
    Context(commands::context::ContextArgs),
    /// Runs preflight health checks for the Yantra runtime environment
    Doctor(commands::doctor::DoctorArgs),
    /// Runs the autonomous Night Mode pipeline (Twilight → Night Run → Dawn Digest)
    Night {
        /// Comma-separated task descriptions to plan (interactive prompt when omitted)
        #[arg(long, value_delimiter = ',')]
        tasks: Vec<String>,
        /// Skip LLM calls — use the no-op dry-run executor for smoke testing
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Unified entry point: launches the Yantra Console (conversation + live graph + telemetry)
    Start {
        /// Optional task description — auto-submitted as `run <task>` when provided
        task: Option<String>,
    },
    /// Prints the current version
    Version,
}

struct ConfiguredTierProvider {
    inner: Arc<dyn ModelProvider>,
    tier: ModelTier,
}

impl std::fmt::Debug for ConfiguredTierProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfiguredTierProvider")
            .field("id", &self.inner.id())
            .field("tier", &self.tier)
            .finish()
    }
}

#[async_trait]
impl ModelProvider for ConfiguredTierProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn tier(&self) -> ModelTier {
        self.tier
    }

    fn capability(&self) -> ModelCapability {
        self.inner.capability()
    }

    async fn status(&self) -> ProviderStatus {
        self.inner.status().await
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        self.inner.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> Result<
        std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<String, ProviderError>> + Send>>,
        ProviderError,
    > {
        self.inner.complete_stream(request).await
    }
}

fn add_providers_from_config(
    config_entries: &[ProviderConfigEntry],
    model_providers: &mut Vec<Arc<dyn ModelProvider>>,
    tier: ModelTier,
) {
    for config_entry in config_entries {
        let provider: Arc<dyn ModelProvider> = if config_entry.kind == "ollama" {
            let endpoint = config_entry
                .base_url
                .as_deref()
                .unwrap_or("http://localhost:11434");
            Arc::new(OllamaProvider::with_endpoint(
                &config_entry.default_model,
                endpoint,
            ))
        } else if config_entry.kind == "openrouter" {
            let endpoint = config_entry
                .base_url
                .as_deref()
                .unwrap_or("https://openrouter.ai/api/v1/chat/completions");
            let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
            Arc::new(OpenRouterProvider::new(
                api_key,
                &config_entry.default_model,
                endpoint,
            ))
        } else if config_entry.kind == "github_models" {
            let endpoint = config_entry
                .base_url
                .as_deref()
                .unwrap_or("https://models.inference.ai.azure.com/chat/completions");
            let api_key = std::env::var("GITHUB_TOKEN").unwrap_or_default();
            Arc::new(GitHubModelsProvider::new(
                api_key,
                &config_entry.default_model,
                endpoint,
            ))
        } else {
            continue;
        };

        let tiered_provider = ConfiguredTierProvider {
            inner: provider,
            tier,
        };
        model_providers.push(Arc::new(tiered_provider));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli_arguments = Cli::parse();

    // 1. Initialize Tracing
    let current_directory = std::env::current_dir()?;
    let project_root = ProjectRoot::new(current_directory)?;
    let session_id = SessionId::new();

    let tracing_config = TracingConfig {
        project_root: project_root.clone(),
        session_id,
        env_filter: std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        pretty_stderr: true,
    };
    let _tracing_guard = init_tracing(tracing_config)?;

    // 2. Load Routing Configuration
    let config_file_path = Path::new("configs/routing.toml");
    let routing_config: RoutingConfig = if config_file_path.exists() {
        let config_text = fs::read_to_string(config_file_path)?;
        toml::from_str(&config_text)?
    } else {
        RoutingConfig {
            providers: ProvidersConfig {
                tier0: vec![ProviderConfigEntry {
                    kind: "ollama".to_string(),
                    base_url: Some("http://localhost:11434".to_string()),
                    default_model: "qwen2.5-coder:7b".to_string(),
                }],
                tier1: vec![],
                tier2: vec![],
                tier3: vec![],
            },
            budget: BudgetConfig {
                soft_usd: 1.0,
                hard_usd: 5.0,
                kill_usd: 20.0,
            },
        }
    };

    // 3. Construct Providers and Router
    let mut model_providers: Vec<Arc<dyn ModelProvider>> = Vec::new();
    add_providers_from_config(
        &routing_config.providers.tier0,
        &mut model_providers,
        ModelTier::Tier0,
    );
    add_providers_from_config(
        &routing_config.providers.tier1,
        &mut model_providers,
        ModelTier::Tier1,
    );
    add_providers_from_config(
        &routing_config.providers.tier2,
        &mut model_providers,
        ModelTier::Tier2,
    );
    add_providers_from_config(
        &routing_config.providers.tier3,
        &mut model_providers,
        ModelTier::Tier3,
    );

    let routing_policy = RoutingPolicy::from_file(config_file_path).unwrap_or_default();
    let router = Arc::new(Router::new(routing_policy, model_providers));

    let cost_thresholds = CostThresholds {
        soft: routing_config.budget.soft_usd,
        hard: routing_config.budget.hard_usd,
        kill: routing_config.budget.kill_usd,
    };

    match cli_arguments.command {
        Commands::Run { description } => {
            commands::run::run_command(description, project_root, router.clone()).await?;
        }
        Commands::Index { path } => {
            let target_path_str = path.unwrap_or_else(|| ".".to_string());
            let target_path = PathBuf::from(target_path_str);
            println!("Building symbol index for path: {target_path:?}");

            let crg_database_path = project_root.as_path().join(".yantra").join("crg.sqlite");
            if let Some(parent_directory) = crg_database_path.parent() {
                fs::create_dir_all(parent_directory)?;
            }
            let database_connection = rusqlite::Connection::open(&crg_database_path)?;

            let graph_builder = yantra_crg::GraphBuilder::new(database_connection);
            graph_builder.build_from_repo(&target_path)?;

            let database_connection = graph_builder.into_connection();
            let indexed_symbols_count: i64 = database_connection.query_row(
                "SELECT COUNT(*) FROM symbols WHERE kind != 'file'",
                [],
                |row| row.get(0),
            )?;
            println!("Successfully indexed {indexed_symbols_count} symbols.");

            println!("Building graph cache and generating embeddings...");
            let graph_cache = yantra_crg::GraphCache::build(&database_connection)?;
            let crg_cache_path = project_root
                .as_path()
                .join(".yantra")
                .join("crg_cache.json");
            let serialized_graph = serde_json::to_string(&graph_cache)?;
            fs::write(crg_cache_path, serialized_graph)?;

            let embedding_store = yantra_crg::EmbeddingStore::new()?;
            embedding_store.embed_all(&database_connection)?;
            println!("All embeddings and cache built successfully.");
        }
        Commands::Ask { question } => {
            let (ask_event_sender, mut ask_event_receiver) =
                tokio::sync::mpsc::unbounded_channel::<commands::ask::AskEvent>();
            let router_clone = router.clone();
            let project_root_clone = project_root.clone();
            let ask_join_handle = tokio::spawn(async move {
                commands::ask::run_ask(
                    &question,
                    router_clone,
                    &project_root_clone,
                    session_id,
                    ask_event_sender,
                )
                .await
            });
            while let Some(ask_event) = ask_event_receiver.recv().await {
                match ask_event {
                    commands::ask::AskEvent::Subgraph(subgraph_text) => {
                        if subgraph_text.starts_with("(No CRG") {
                            println!("{subgraph_text}\n");
                        } else {
                            println!("## Extracted CRG Subgraph:\n{subgraph_text}\n");
                        }
                    }
                    commands::ask::AskEvent::Token(token) => {
                        print!("{token}");
                        let _ = std::io::stdout().flush();
                    }
                    commands::ask::AskEvent::GroundingWarning(warning_text) => {
                        println!("\n\x1b[33m⚠ {warning_text}\x1b[0m");
                    }
                    commands::ask::AskEvent::Cost(cost_usd) => {
                        println!("\nCost: ${cost_usd:.6}");
                    }
                }
            }
            ask_join_handle.await??;
        }
        Commands::Canvas {
            url,
            port,
            render_js,
        } => {
            commands::canvas::canvas_command(url, port, render_js, router.clone()).await?;
        }
        Commands::Graph { focus, port } => {
            let crg_database_path = project_root.as_path().join(".yantra").join("crg.sqlite");
            commands::graph::graph_command(focus, crg_database_path, port).await?;
        }
        Commands::Observe => {
            let trace_database_path = project_root.as_path().join(".yantra").join("traces.sqlite");
            commands::observe::observe_command(trace_database_path, cost_thresholds).await?;
        }
        Commands::Status { json } => {
            let trace_database_path = project_root.as_path().join(".yantra").join("traces.sqlite");

            if !trace_database_path.exists() {
                let empty_status = StatusSnapshot::empty(&cost_thresholds);
                print_status_snapshot(&empty_status, json)?;
                return Ok(());
            }

            let trace_connection = rusqlite::Connection::open(&trace_database_path)?;

            let table_exists: bool = trace_connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='spans')",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);

            if !table_exists {
                let empty_status = StatusSnapshot::empty(&cost_thresholds);
                print_status_snapshot(&empty_status, json)?;
                return Ok(());
            }

            let mut latest_session_query = trace_connection.prepare(
                "SELECT session_id, SUM(cost_usd), COUNT(*) FROM spans GROUP BY session_id ORDER BY MAX(started_at) DESC LIMIT 1"
            )?;
            let mut rows = latest_session_query.query([])?;
            if let Some(row) = rows.next()? {
                let session_id_str: String = row.get(0)?;
                let total_cost: f64 = row.get(1)?;
                let spans_count: i64 = row.get(2)?;

                let parsed_session_id = SessionId::from_str(&session_id_str)?;
                let status_label = classify_cost_status(total_cost, &cost_thresholds);
                let status_snapshot = StatusSnapshot {
                    session_id: Some(parsed_session_id.to_string()),
                    total_spans: spans_count,
                    cumulative_cost_usd: total_cost,
                    status: status_label.to_string(),
                };
                print_status_snapshot(&status_snapshot, json)?;
            } else {
                let empty_status = StatusSnapshot::empty(&cost_thresholds);
                print_status_snapshot(&empty_status, json)?;
            }
        }
        Commands::Context(context_args) => {
            commands::context::run_context(context_args).await?;
        }
        Commands::Doctor(doctor_args) => {
            commands::doctor::run_doctor(doctor_args).await?;
        }
        Commands::Night { tasks, dry_run } => {
            commands::night::night_command(tasks, dry_run, router.clone(), project_root).await?;
        }
        Commands::Start { task } => {
            commands::start::start_command(
                task,
                router.clone(),
                project_root,
                cost_thresholds,
                session_id,
            )
            .await?;
        }
        Commands::Version => {
            println!("yantra 0.3.0");
        }
    }

    Ok(())
}
