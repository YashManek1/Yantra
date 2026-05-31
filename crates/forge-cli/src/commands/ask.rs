//! # forge-cli::commands::ask: Reusable Streaming Ask Pipeline
//!
//! Implements the CRG-grounded ask flow as a channel-based streaming function
//! so both the `yantra ask` CLI subcommand and the unified Yantra Console can
//! share one implementation (CLAUDE §3.2 — no duplication). The function emits
//! structured events to an unbounded channel instead of printing directly, so
//! each caller can apply its own rendering (coloured stdout for the CLI, pane
//! lines for the Console TUI).
//!
//! ## Input
//! - `question: &str` — natural-language question about the codebase
//! - `router: Arc<Router>` — pre-built model router for Tier-0 completion
//! - `project_root: &ProjectRoot` — workspace root (locates `.yantra/crg.sqlite`)
//! - `session_id: SessionId` — current session (for span recording)
//! - `event_sender: mpsc::UnboundedSender<AskEvent>` — channel for streamed events
//!
//! ## Output
//! - `anyhow::Result<()>` — all results are delivered via `AskEvent` on the channel
//! - Side-effects: emits `AskEvent` variants on `event_sender` during execution;
//!   records a span in `.yantra/traces.sqlite` when a model call completes.
//!
//! ## Related
//! - `forge-cli::main` — `Commands::Ask` uses this via a stdout-rendering wrapper
//! - `forge-cli::commands::console` — Console TUI uses this to stream into its pane
//! - `forge-crg::extract_subgraph` — CRG subgraph extraction used here

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use yantra_core::{AgentKind, ModelTier, Outcome, ProjectRoot, SessionId, Span, SpanId, TaskId};
use yantra_obs::record_span;
use yantra_router::{
    routing::RoutedCompletionRequest, CompletionRequest, Message, MessageRole, Router,
};

/// An event emitted to the caller's channel during a streaming ask invocation.
pub(crate) enum AskEvent {
    /// The CRG subgraph preamble text (or an empty-index notice).
    Subgraph(String),
    /// One streamed model output token.
    Token(String),
    /// A grounding or cross-role warning (plain text, no ANSI escapes).
    GroundingWarning(String),
    /// Total USD cost of the model call (emitted once, after streaming ends).
    Cost(f64),
}

/// Intermediate context produced by the synchronous CRG-subgraph extraction phase.
struct AskContext {
    subgraph_text: String,
    rendered_subgraph: Option<yantra_crg::RenderedSubgraph>,
    graph_cache: Option<yantra_crg::GraphCache>,
    system_prompt: String,
}

/// Builds the message list for the ask model call.
///
/// Separated from the main flow so unit tests can verify the message structure
/// without running a model.
pub(crate) fn build_ask_messages(
    subgraph_text: &str,
    question: &str,
    system_prompt: &str,
) -> Vec<Message> {
    let code_context_section = if subgraph_text.is_empty() {
        "No CRG index available for this repository.".to_owned()
    } else {
        subgraph_text.to_owned()
    };

    let user_content = format!(
        "## Code Context (CRG Subgraph)\n{code_context_section}\n\n\
         ## Question\n{question}"
    );

    vec![
        Message {
            role: MessageRole::System,
            content: system_prompt.to_owned(),
            tool_calls: Vec::new(),
        },
        Message {
            role: MessageRole::User,
            content: user_content,
            tool_calls: Vec::new(),
        },
    ]
}

/// Estimates the USD cost of a model call from token counts and per-1k rates.
///
/// Separated from the main flow so unit tests can verify cost arithmetic.
pub(crate) fn estimate_cost(
    tokens_in: u64,
    tokens_out: u64,
    cost_per_1k_in: f32,
    cost_per_1k_out: f32,
) -> f64 {
    let input_cost = (tokens_in as f64 / 1000.0) * f64::from(cost_per_1k_in);
    let output_cost = (tokens_out as f64 / 1000.0) * f64::from(cost_per_1k_out);
    input_cost + output_cost
}

/// Extracts the CRG subgraph synchronously and loads the system prompt.
///
/// Runs inside `tokio::task::block_in_place` so it does not block async tasks
/// on the runtime's I/O threads while performing synchronous rusqlite and
/// fastembed work.
fn extract_ask_context(
    question: &str,
    crg_database_path: &std::path::Path,
    crg_cache_path: &std::path::Path,
) -> AskContext {
    let system_prompt = std::fs::read_to_string("crates/forge-agents/prompts/ask.md")
        .unwrap_or_else(|_| {
            "You are the Ask agent inside Yantra, an honest and precise architectural navigator."
                .to_owned()
        });

    if !crg_database_path.exists() {
        return AskContext {
            subgraph_text: String::new(),
            rendered_subgraph: None,
            graph_cache: None,
            system_prompt,
        };
    }

    let database_connection = match rusqlite::Connection::open(crg_database_path) {
        Ok(connection) => connection,
        Err(db_error) => {
            tracing::warn!(error = %db_error, "could not open CRG database for ask");
            return AskContext {
                subgraph_text: String::new(),
                rendered_subgraph: None,
                graph_cache: None,
                system_prompt,
            };
        }
    };

    let _ = yantra_crg::schema::create_crg_schema(&database_connection);

    let graph_cache = {
        let cached_graph = if crg_cache_path.exists() {
            std::fs::read_to_string(crg_cache_path)
                .ok()
                .and_then(|cache_text| {
                    serde_json::from_str::<yantra_crg::GraphCache>(&cache_text).ok()
                })
        } else {
            None
        };
        match cached_graph {
            Some(deserialized_cache) => deserialized_cache,
            None => match yantra_crg::GraphCache::build(&database_connection) {
                Ok(newly_built) => {
                    if let Ok(serialized) = serde_json::to_string(&newly_built) {
                        let _ = std::fs::write(crg_cache_path, serialized);
                    }
                    newly_built
                }
                Err(build_error) => {
                    tracing::warn!(error = %build_error, "GraphCache::build failed");
                    return AskContext {
                        subgraph_text: String::new(),
                        rendered_subgraph: None,
                        graph_cache: None,
                        system_prompt,
                    };
                }
            },
        }
    };

    let embedding_store = match yantra_crg::EmbeddingStore::new() {
        Ok(store) => store,
        Err(embed_error) => {
            tracing::warn!(error = %embed_error, "EmbeddingStore::new failed");
            return AskContext {
                subgraph_text: String::new(),
                rendered_subgraph: None,
                graph_cache: Some(graph_cache),
                system_prompt,
            };
        }
    };

    if let Err(load_error) = embedding_store.load_vectors_from_db(&database_connection) {
        tracing::warn!(error = %load_error, "embedding vector load failed");
        return AskContext {
            subgraph_text: String::new(),
            rendered_subgraph: None,
            graph_cache: Some(graph_cache),
            system_prompt,
        };
    }

    match yantra_crg::extract_subgraph(&graph_cache, &embedding_store, question, 8192, &[]) {
        Ok(rendered_subgraph) => {
            let subgraph_text = rendered_subgraph.text.clone();
            AskContext {
                subgraph_text,
                rendered_subgraph: Some(rendered_subgraph),
                graph_cache: Some(graph_cache),
                system_prompt,
            }
        }
        Err(extract_error) => {
            tracing::warn!(error = %extract_error, "subgraph extraction failed");
            AskContext {
                subgraph_text: String::new(),
                rendered_subgraph: None,
                graph_cache: Some(graph_cache),
                system_prompt,
            }
        }
    }
}

/// Runs the full CRG-grounded ask pipeline, emitting events to `event_sender`.
///
/// The synchronous CRG/rusqlite/fastembed work runs via
/// `tokio::task::block_in_place` so it does not block other async tasks.
/// The model streaming loop is fully async. Span recording at the end also
/// uses `block_in_place`.
///
/// Errors from `event_sender.send(...)` are ignored silently (receiver
/// dropped means the caller is gone).
///
/// # Errors
///
/// Returns `anyhow::Error` when the model router cannot be obtained or the
/// streaming call fails.
pub(crate) async fn run_ask(
    question: &str,
    router: Arc<Router>,
    project_root: &ProjectRoot,
    session_id: SessionId,
    event_sender: mpsc::UnboundedSender<AskEvent>,
) -> anyhow::Result<()> {
    let question_owned = question.to_owned();
    let crg_database_path = project_root.as_path().join(".yantra").join("crg.sqlite");
    let crg_cache_path = project_root
        .as_path()
        .join(".yantra")
        .join("crg_cache.json");

    let ask_context = tokio::task::block_in_place(|| {
        extract_ask_context(&question_owned, &crg_database_path, &crg_cache_path)
    });

    if ask_context.subgraph_text.is_empty() {
        let _ = event_sender.send(AskEvent::Subgraph(
            "(No CRG index found or extracted. Proceeding with question context only.)".to_owned(),
        ));
    } else {
        let _ = event_sender.send(AskEvent::Subgraph(ask_context.subgraph_text.clone()));
    }

    let messages = build_ask_messages(
        &ask_context.subgraph_text,
        &question_owned,
        &ask_context.system_prompt,
    );

    let completion_request = CompletionRequest {
        messages,
        max_tokens: Some(1024),
        temperature: 0.2,
        tools: None,
        stop_sequences: Vec::new(),
    };
    let routed_request = RoutedCompletionRequest {
        required_tier: ModelTier::Tier0,
        completion_request,
    };

    let provider = router.route(&routed_request).await?;

    let start_instant = std::time::Instant::now();
    let mut token_stream = provider
        .complete_stream(routed_request.completion_request.clone())
        .await?;

    let mut accumulated_answer = String::new();
    while let Some(token_result) = token_stream.next().await {
        let token = token_result?;
        accumulated_answer.push_str(&token);
        let _ = event_sender.send(AskEvent::Token(token));
    }

    let elapsed_milliseconds =
        u64::try_from(start_instant.elapsed().as_millis()).unwrap_or(u64::MAX);

    if let (Some(rendered_subgraph), Some(global_graph_cache)) =
        (&ask_context.rendered_subgraph, &ask_context.graph_cache)
    {
        let unverified_symbols = crate::ask_verifier::SymbolAllowlistVerifier::verify(
            rendered_subgraph,
            global_graph_cache,
            &accumulated_answer,
        );
        for unverified_symbol in unverified_symbols {
            let _ = event_sender.send(AskEvent::GroundingWarning(format!(
                "Unverified symbol/path (may be hallucinated): `{unverified_symbol}`"
            )));
        }

        let cross_role_warnings =
            crate::ask_verifier::SymbolAllowlistVerifier::check_cross_crate_conflations(
                rendered_subgraph,
                global_graph_cache,
                &accumulated_answer,
            );
        for warning in cross_role_warnings {
            let _ = event_sender.send(AskEvent::GroundingWarning(format!(
                "Cross-role warning: {warning}"
            )));
        }
    }

    let provider_capability = provider.capability();
    let tokens_in = (routed_request
        .completion_request
        .messages
        .iter()
        .map(|msg| msg.content.len())
        .sum::<usize>()
        / 4) as u64;
    let tokens_out = (accumulated_answer.len() / 4) as u64;
    let total_call_cost = estimate_cost(
        tokens_in,
        tokens_out,
        provider_capability.cost_per_1k_in,
        provider_capability.cost_per_1k_out,
    );

    let _ = event_sender.send(AskEvent::Cost(total_call_cost));

    let trace_database_path = project_root.as_path().join(".yantra").join("traces.sqlite");
    let provider_id = provider.id().to_owned();
    tokio::task::block_in_place(|| {
        let trace_result: anyhow::Result<()> = (|| {
            if let Some(parent_directory) = trace_database_path.parent() {
                std::fs::create_dir_all(parent_directory)?;
            }
            let trace_connection = rusqlite::Connection::open(&trace_database_path)?;
            let span_record = Span {
                span_id: SpanId::new(),
                parent_id: None,
                session_id,
                task_id: Some(TaskId::new()),
                truth_token: None,
                agent: Some(AgentKind::Coder),
                model: yantra_core::ModelId::new(&provider_id).ok(),
                tokens_in,
                tokens_out,
                cost_usd: total_call_cost,
                duration_ms: elapsed_milliseconds,
                started_at: chrono::Utc::now(),
                outcome: Outcome::Success,
                error: None,
            };
            record_span(&trace_connection, &span_record)?;
            Ok(())
        })();
        if let Err(trace_error) = trace_result {
            tracing::warn!(error = %trace_error, "failed to record ask span");
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ask_messages_has_system_then_user_role() {
        let messages =
            build_ask_messages("some subgraph text", "what does X do?", "You are helpful.");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[1].role, MessageRole::User);
    }

    #[test]
    fn build_ask_messages_embeds_question_in_user_content() {
        let messages = build_ask_messages("context", "explain run_ask", "system prompt");
        assert!(messages[1].content.contains("explain run_ask"));
    }

    #[test]
    fn build_ask_messages_uses_no_index_notice_when_subgraph_empty() {
        let messages = build_ask_messages("", "my question", "system");
        assert!(messages[1].content.contains("No CRG index available"));
    }

    #[test]
    fn estimate_cost_basic_arithmetic() {
        let cost = estimate_cost(1000, 500, 0.002, 0.004);
        assert!((cost - 0.004).abs() < 1e-9, "expected 0.004, got {cost}");
    }

    #[test]
    fn estimate_cost_zero_tokens() {
        let cost = estimate_cost(0, 0, 0.002, 0.004);
        assert!(cost.abs() < f64::EPSILON);
    }
}
