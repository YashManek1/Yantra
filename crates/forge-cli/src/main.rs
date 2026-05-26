//! # forge-cli: Yantra Command-Line Interface
//!
//! Entry point for the `yantra` binary. Uses clap for argument parsing and
//! ratatui for the interactive terminal UI. Delegates all runtime logic to
//! `forge-orchestrator`, `forge-night`, and `forge-serve`.
//!
//! ## Input
//! - CLI arguments: subcommand (index, ask, run, night), flags, and options
//! - Interactive terminal input during `STVP` questionnaires
//!
//! ## Output
//! - Terminal UI rendered via ratatui
//! - Exit code 0 on success, non-zero on task failure or user abort
//!
//! ## Related
//! - `forge-orchestrator` — receives task submissions
//! - `forge-night` — starts Night Mode on `yantra night`
//! - `forge-serve` — optionally launched for the Live Canvas UI

mod commands;

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use serde::Deserialize;
use yantra_core::{AgentKind, ModelTier, Outcome, ProjectRoot, SessionId, Span, SpanId, TaskId};
use yantra_obs::{init_tracing, record_span, CostThresholds, TracingConfig};
use yantra_router::routing::RoutedCompletionRequest;
use yantra_router::{
    CompletionRequest, GitHubModelsProvider, Message, MessageRole, ModelProvider, OllamaProvider,
    OpenRouterProvider, Router, RoutingPolicy,
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
    /// Prints cost gauge and active session info
    Status,
    /// Prints the current version
    Version,
}

fn add_providers_from_config(
    config_entries: &[ProviderConfigEntry],
    model_providers: &mut Vec<Arc<dyn ModelProvider>>,
) {
    for config_entry in config_entries {
        if config_entry.kind == "ollama" {
            let endpoint = config_entry
                .base_url
                .as_deref()
                .unwrap_or("http://localhost:11434");
            let provider = OllamaProvider::with_endpoint(&config_entry.default_model, endpoint);
            model_providers.push(Arc::new(provider));
        } else if config_entry.kind == "openrouter" {
            let endpoint = config_entry
                .base_url
                .as_deref()
                .unwrap_or("https://openrouter.ai/api/v1/chat/completions");
            let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
            let provider = OpenRouterProvider::new(api_key, &config_entry.default_model, endpoint);
            model_providers.push(Arc::new(provider));
        } else if config_entry.kind == "github_models" {
            let endpoint = config_entry
                .base_url
                .as_deref()
                .unwrap_or("https://models.inference.ai.azure.com/chat/completions");
            let api_key = std::env::var("GITHUB_TOKEN").unwrap_or_default();
            let provider =
                GitHubModelsProvider::new(api_key, &config_entry.default_model, endpoint);
            model_providers.push(Arc::new(provider));
        }
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
    add_providers_from_config(&routing_config.providers.tier0, &mut model_providers);
    add_providers_from_config(&routing_config.providers.tier1, &mut model_providers);
    add_providers_from_config(&routing_config.providers.tier2, &mut model_providers);
    add_providers_from_config(&routing_config.providers.tier3, &mut model_providers);

    let routing_policy = RoutingPolicy::default();
    let router = Router::new(routing_policy, model_providers);

    match cli_arguments.command {
        Commands::Run { description } => {
            commands::run::run_command(description, project_root, Arc::new(router)).await?;
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
            let crg_database_path = project_root.as_path().join(".yantra").join("crg.sqlite");
            let crg_cache_path = project_root
                .as_path()
                .join(".yantra")
                .join("crg_cache.json");
            let mut subgraph_text = String::new();

            if crg_database_path.exists() {
                if let Ok(database_connection) = rusqlite::Connection::open(&crg_database_path) {
                    let _ = yantra_crg::schema::create_crg_schema(&database_connection);

                    let mut graph_cache_option = None;
                    if crg_cache_path.exists() {
                        if let Ok(cache_text) = fs::read_to_string(&crg_cache_path) {
                            if let Ok(deserialized_graph_cache) =
                                serde_json::from_str::<yantra_crg::GraphCache>(&cache_text)
                            {
                                graph_cache_option = Some(deserialized_graph_cache);
                            }
                        }
                    }

                    let graph_cache = if let Some(cached_graph) = graph_cache_option {
                        cached_graph
                    } else {
                        let newly_built_graph =
                            yantra_crg::GraphCache::build(&database_connection)?;
                        if let Ok(serialized_graph) = serde_json::to_string(&newly_built_graph) {
                            let _ = fs::write(&crg_cache_path, serialized_graph);
                        }
                        newly_built_graph
                    };

                    let embedding_store = yantra_crg::EmbeddingStore::new()?;
                    embedding_store.load_vectors_from_db(&database_connection)?;
                    let rendered_subgraph = yantra_crg::extract_subgraph(
                        &graph_cache,
                        &embedding_store,
                        &question,
                        8192,
                        &[],
                    )?;
                    subgraph_text = rendered_subgraph.text;
                }
            }

            if subgraph_text.is_empty() {
                println!("(No CRG Subgraph found or extracted. Proceeding with question context only.)\n");
            } else {
                println!("## Extracted CRG Subgraph:\n{subgraph_text}\n");
            }

            let system_prompt = fs::read_to_string("crates/forge-agents/prompts/coder.md")
                .unwrap_or_else(|_| "You write code in the dialect of the local repo. Use only symbols visible in the provided code-review subgraph.".to_string());

            let code_context_section = if subgraph_text.is_empty() {
                "No CRG index available for this repository.".to_string()
            } else {
                subgraph_text.clone()
            };

            let user_content = format!(
                "## Source Truth\nTask ID: ask\nClass: Docstring\nStrictness: Trust\n\n\
                 ## Code Context (CRG Subgraph)\n{code_context_section}\n\n\
                 ## Task\n{question}\n\n\
                 ## Instructions\n\
                 CRITICAL CONSTRAINT: You MUST only cite file paths and symbol names that are \
                 explicitly listed verbatim in the CRG Subgraph block above. \
                 Do NOT invent, guess, or hallucinate any file paths, struct names, function \
                 names, or module paths that do not appear in the subgraph.\n\
                 1. Identify the PRIMARY ARCHITECTURAL INSERTION POINT from the subgraph: the \
                 struct, trait, or impl block where the new code belongs. \
                 High-connectivity [SEED] symbols are strong candidates. \
                 Low-hop [hop:1] symbols adjacent to seeds indicate the call flow.\n\
                 2. Trace the connection/flow from seed symbols to the insertion point using \
                 only the edges listed at the bottom of the subgraph (X -calls→ Y).\n\
                 3. State your answer with the exact file path and symbol name as shown in the \
                 subgraph. If the exact insertion point is not present in the subgraph, identify \
                 the closest visible parent module or related struct and suggest where the new \
                 code should reside relative to that visible structure, marking your answer clearly \
                 as \"Context-Inferred\". Do not completely refuse if related structural seeds are present."
            );

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
                max_tokens: None,
                temperature: 0.2,
                tools: None,
                stop_sequences: Vec::new(),
            };
            let mut routed_request = RoutedCompletionRequest {
                required_tier: ModelTier::Tier0,
                completion_request,
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
                })?;

            let start_instant = std::time::Instant::now();
            let completion_response = provider.complete(routed_request.completion_request).await?;
            let duration_milliseconds =
                u64::try_from(start_instant.elapsed().as_millis()).unwrap_or(u64::MAX);

            println!("{}", completion_response.content);

            let cost_per_thousand_input = provider.capability().cost_per_1k_in;
            let cost_per_thousand_output = provider.capability().cost_per_1k_out;
            let input_cost = (completion_response.tokens_in as f64 / 1000.0)
                * f64::from(cost_per_thousand_input);
            let output_cost = (completion_response.tokens_out as f64 / 1000.0)
                * f64::from(cost_per_thousand_output);
            let total_call_cost = input_cost + output_cost;

            println!("Cost: ${total_call_cost:.6}");

            // Record this model execution span in the trace database
            let trace_database_path = project_root.as_path().join(".yantra").join("traces.sqlite");
            if let Some(parent_directory) = trace_database_path.parent() {
                fs::create_dir_all(parent_directory)?;
            }
            let trace_connection = rusqlite::Connection::open(&trace_database_path)?;

            let span_record = Span {
                span_id: SpanId::new(),
                parent_id: None,
                session_id,
                task_id: Some(TaskId::new()),
                truth_token: None,
                agent: Some(AgentKind::Coder),
                model: yantra_core::ModelId::new(provider.id()).ok(),
                tokens_in: completion_response.tokens_in,
                tokens_out: completion_response.tokens_out,
                cost_usd: total_call_cost,
                duration_ms: duration_milliseconds,
                started_at: chrono::Utc::now(),
                outcome: Outcome::Success,
                error: None,
            };
            record_span(&trace_connection, &span_record)?;
        }
        Commands::Status => {
            let trace_database_path = project_root.as_path().join(".yantra").join("traces.sqlite");
            if !trace_database_path.exists() {
                println!("No active sessions found (traces database does not exist).");
                println!("Cumulative Cost: $0.000000");
                println!("Status: Ok");
                return Ok(());
            }

            let trace_connection = rusqlite::Connection::open(&trace_database_path)?;

            let table_exists: bool = trace_connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='spans')",
                [],
                |row| row.get(0),
            ).unwrap_or(false);

            if !table_exists {
                println!("No active sessions found (traces database is empty).");
                println!("Cumulative Cost: $0.000000");
                println!("Status: Ok");
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
                let cost_thresholds = CostThresholds {
                    soft: routing_config.budget.soft_usd,
                    hard: routing_config.budget.hard_usd,
                    kill: routing_config.budget.kill_usd,
                };

                // Print session information
                println!("Active Session: {parsed_session_id}");
                println!("Total Spans: {spans_count}");
                println!("Cumulative Cost: ${total_cost:.6}");

                // Cost status classification
                let status_label = if total_cost >= f64::from(cost_thresholds.kill) {
                    "Kill"
                } else if total_cost >= f64::from(cost_thresholds.hard) {
                    "Pause"
                } else if total_cost >= f64::from(cost_thresholds.soft) {
                    "Warn"
                } else {
                    "Ok"
                };
                println!("Status: {status_label}");
            } else {
                println!("No spans found in traces database.");
                println!("Cumulative Cost: $0.000000");
                println!("Status: Ok");
            }
        }
        Commands::Version => {
            println!("yantra 0.1.0");
        }
    }

    Ok(())
}
