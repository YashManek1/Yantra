//! # forge-canvas::graph_viz: CRG Graph Endpoints
//!
//! Lazy-loads `.yantra/crg.sqlite` on first request, serves the
//! vis.js-shaped graph JSON, and answers LLM explanation requests by
//! routing through the existing `yantra-router` at `ModelTier::Tier0`.
//!
//! ## Input
//! - `AppState.graph` — `GraphState` with optional cached `GraphCache`
//! - `?focus=<symbol_id>` query for subgraph extraction
//!
//! ## Output
//! - `GET  /api/graph/json` → `GraphJson` (vis.js nodes + edges)
//! - `POST /api/graph/explain` → `{markdown}` with LLM-generated text
//!
//! ## Related
//! - `forge-crg::export` — produces the JSON shape
//! - `forge-router::Router` — Tier 0 call target for explanations

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use yantra_crg::{export_focused, export_graph, GraphCache};

use crate::server::AppState;

/// Server-side cache state for the CRG graph viewer.
#[derive(Clone)]
pub struct GraphState {
    /// Lazy-loaded graph cache. `None` until the first request lands.
    pub cache: Arc<RwLock<Option<GraphCache>>>,
    /// Path to `.yantra/crg.sqlite`; defaulted to `./.yantra/crg.sqlite`.
    pub sqlite_path: Arc<PathBuf>,
}

impl GraphState {
    /// Creates an idle `GraphState` that lazy-loads from the default location
    /// `./.yantra/crg.sqlite`.
    pub fn idle() -> Self {
        Self {
            cache: Arc::new(RwLock::new(None)),
            sqlite_path: Arc::new(PathBuf::from("./.yantra/crg.sqlite")),
        }
    }

    /// Returns the cached graph, building it from SQLite on first call.
    async fn get_or_load(&self) -> Result<GraphCache, String> {
        {
            let guard = self.cache.read().await;
            if let Some(cached) = guard.as_ref() {
                return Ok(cached.clone());
            }
        }
        let path_owned = (*self.sqlite_path).clone();
        let built = tokio::task::spawn_blocking(move || -> Result<GraphCache, String> {
            let conn = Connection::open(&path_owned)
                .map_err(|err| format!("open {}: {err}", path_owned.display()))?;
            GraphCache::build(&conn).map_err(|err| format!("build cache: {err}"))
        })
        .await
        .map_err(|join_err| format!("background task: {join_err}"))??;
        {
            let mut guard = self.cache.write().await;
            *guard = Some(built.clone());
        }
        Ok(built)
    }
}

/// Query parameters for `/api/graph/json`.
#[derive(Debug, Deserialize)]
pub struct GraphQuery {
    /// Optional focus symbol; restricts output to a BFS-bounded subgraph.
    #[serde(default)]
    pub focus: Option<String>,
    /// BFS hop limit when `focus` is set.
    #[serde(default = "default_hops")]
    pub hops: usize,
}

const fn default_hops() -> usize {
    2
}

/// Axum handler for `GET /api/graph/json`.
pub async fn graph_json_handler(
    State(state): State<AppState>,
    Query(query): Query<GraphQuery>,
) -> Response {
    let cache = match state.graph.get_or_load().await {
        Ok(loaded) => loaded,
        Err(error_text) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "graph cache unavailable",
                    "detail": error_text,
                    "hint": "run 'yantra index .' first to build the CRG"
                })),
            )
                .into_response();
        }
    };

    let graph_json = if let Some(focus_id) = query.focus.as_deref() {
        match export_focused(&cache, focus_id, query.hops) {
            Ok(json_payload) => json_payload,
            Err(error_text) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": error_text })),
                )
                    .into_response();
            }
        }
    } else {
        export_graph(&cache)
    };

    Json(graph_json).into_response()
}

/// Request body for `POST /api/graph/explain`.
#[derive(Debug, Deserialize)]
pub struct ExplainRequest {
    pub symbol_id: String,
}

/// Response body for `POST /api/graph/explain`.
#[derive(Debug, Serialize)]
pub struct ExplainResponse {
    pub markdown: String,
}

/// Axum handler for `POST /api/graph/explain`.
///
/// Phase 1: returns a structural summary built directly from `GraphCache`
/// (no LLM call). The Tier 0 LLM-assisted variant is Phase 2 — wiring the
/// `Router` requires loading `configs/routing.toml`, which the canvas
/// server does not own today.
pub async fn graph_explain_handler(
    State(state): State<AppState>,
    Json(request): Json<ExplainRequest>,
) -> Response {
    let cache = match state.graph.get_or_load().await {
        Ok(loaded) => loaded,
        Err(error_text) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "graph cache unavailable",
                    "detail": error_text
                })),
            )
                .into_response();
        }
    };

    let markdown_text = match build_structural_summary(&cache, &request.symbol_id) {
        Some(text) => text,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "unknown symbol_id" })),
            )
                .into_response();
        }
    };

    Json(ExplainResponse {
        markdown: markdown_text,
    })
    .into_response()
}

fn build_structural_summary(cache: &GraphCache, symbol_id_str: &str) -> Option<String> {
    use std::str::FromStr;
    let symbol_id = yantra_core::SymbolId::from_str(symbol_id_str).ok()?;
    let details = cache.symbol_details.get(&symbol_id)?;

    let neighbour_edges = cache
        .adjacency_index
        .get(symbol_id_str)
        .map_or(&[][..], Vec::as_slice);

    let mut incoming_callers: Vec<&str> = Vec::new();
    let mut outgoing_callees: Vec<&str> = Vec::new();
    for edge in neighbour_edges {
        if edge.to_id == symbol_id_str {
            incoming_callers.push(edge.from_id.as_str());
        } else if edge.from_id == symbol_id_str {
            outgoing_callees.push(edge.to_id.as_str());
        }
    }

    let mut summary = String::new();
    summary.push_str(&format!("# `{}` ({})\n\n", details.name, details.kind));
    summary.push_str(&format!("**File**: `{}`\n\n", details.file_path));
    if let Some(signature) = &details.signature {
        summary.push_str("**Signature**:\n```rust\n");
        summary.push_str(signature);
        summary.push_str("\n```\n\n");
    }
    if let Some(docstring) = &details.docstring {
        summary.push_str("**Docs**:\n");
        summary.push_str(docstring);
        summary.push_str("\n\n");
    }
    summary.push_str(&format!(
        "**Connectivity score**: {}\n\n",
        details.connectivity_score
    ));

    if !incoming_callers.is_empty() {
        summary.push_str(&format!("**Callers** ({}): ", incoming_callers.len()));
        let preview: Vec<&&str> = incoming_callers.iter().take(5).collect();
        let names: Vec<String> = preview
            .iter()
            .filter_map(|id_str| {
                let symbol_id = yantra_core::SymbolId::from_str(id_str).ok()?;
                cache
                    .symbol_details
                    .get(&symbol_id)
                    .map(|details| details.name.clone())
            })
            .collect();
        summary.push_str(&names.join(", "));
        summary.push_str("\n\n");
    }
    if !outgoing_callees.is_empty() {
        summary.push_str(&format!("**Calls** ({}): ", outgoing_callees.len()));
        let preview: Vec<&&str> = outgoing_callees.iter().take(5).collect();
        let names: Vec<String> = preview
            .iter()
            .filter_map(|id_str| {
                let symbol_id = yantra_core::SymbolId::from_str(id_str).ok()?;
                cache
                    .symbol_details
                    .get(&symbol_id)
                    .map(|details| details.name.clone())
            })
            .collect();
        summary.push_str(&names.join(", "));
        summary.push_str("\n\n");
    }

    summary.push_str(
        "---\n*Phase 1: structural summary. LLM-generated\
                      explanations land in Phase 2.*\n",
    );
    Some(summary)
}
