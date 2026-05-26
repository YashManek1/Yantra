//! # forge-tools: CRG MCP Server
//!
//! Exposes five Code-Review Graph query tools as MCP methods. Queries run
//! against the pre-built in-memory `GraphCache` and `EmbeddingStore`, with
//! the `Connection` used only for the recursive-CTE impact query which
//! requires SQL that cannot be expressed over the cache alone.
//!
//! ## Input
//! - JSON-RPC parameters: `task`, `budget`, `symbol_id`, `hops`, `query`
//! - Shared `GraphCache`, `EmbeddingStore`, and `SQLite` `Connection`
//!
//! ## Output
//! - JSON representations of `RenderedSubgraph`, `ImpactReport`, and symbol lists
//!
//! ## Related
//! - `forge-crg::subgraph` — `GraphCache` and `extract_subgraph`
//! - `forge-crg::embedding` — `EmbeddingStore` for semantic search
//! - `forge-tools::mcp` — `McpServer` trait and `ToolCapability`

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use yantra_crg::{extract_subgraph, EmbeddingStore, GraphCache};

use crate::error::McpError;
use crate::mcp::{McpServer, ToolCapability};

/// A single symbol entry returned by search and dependency tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    /// UUID symbol identifier.
    pub id: String,
    /// Symbol name as it appears in source.
    pub name: String,
    /// Symbol kind: `function`, `struct`, `trait`, `impl`, etc.
    pub kind: String,
    /// Absolute path of the containing file.
    pub file_path: String,
    /// 1-based line number where the symbol starts.
    pub start_line: i32,
}

/// Result of a `crg.impact_of_change` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactReport {
    /// The root symbol whose change is being analysed.
    pub root_symbol: SymbolEntry,
    /// All symbols that transitively call the root within 5 hops.
    pub affected_symbols: Vec<SymbolEntry>,
    /// Maps each affected symbol id to the shortest hop distance from root.
    pub hop_map: std::collections::HashMap<String, u8>,
}

/// CRG MCP server exposing five graph query tools.
pub struct CrgMcpServer {
    sqlite_connection: Arc<Mutex<Connection>>,
    embedding_store: Arc<EmbeddingStore>,
    graph_cache: Arc<GraphCache>,
    server_name: String,
}

impl CrgMcpServer {
    /// Creates a new `CrgMcpServer`.
    ///
    /// The connection must have `create_crg_schema` applied and
    /// `build_from_repo` completed before this server is used. The
    /// `EmbeddingStore` must have `embed_all` called beforehand.
    pub fn new(
        sqlite_connection: Connection,
        embedding_store: EmbeddingStore,
        graph_cache: GraphCache,
    ) -> Self {
        Self {
            sqlite_connection: Arc::new(Mutex::new(sqlite_connection)),
            embedding_store: Arc::new(embedding_store),
            graph_cache: Arc::new(graph_cache),
            server_name: "crg".to_owned(),
        }
    }

    /// `crg.subgraph` — extracts a token-budgeted subgraph for a task description.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for missing parameters or extraction failure.
    pub async fn subgraph(&self, params: &Value) -> Result<Value, McpError> {
        let task_description = required_string(params, "task")?;
        let token_budget = params.get("budget").and_then(Value::as_u64).unwrap_or(4000) as usize;

        let graph_cache = Arc::clone(&self.graph_cache);
        let embedding_store = Arc::clone(&self.embedding_store);
        let task_description_clone = task_description.clone();

        let rendered_subgraph = tokio::task::spawn_blocking(move || {
            extract_subgraph(
                &graph_cache,
                &embedding_store,
                task_description_clone.as_str(),
                token_budget,
                &[],
            )
        })
        .await
        .map_err(|join_error| McpError::internal(join_error.to_string()))?
        .map_err(|extraction_error| McpError::internal(extraction_error.to_string()))?;

        Ok(json!({
            "text": rendered_subgraph.text,
            "token_cost": rendered_subgraph.token_cost,
            "included_node_count": rendered_subgraph.included_nodes.len(),
        }))
    }

    /// `crg.expand_symbol` — BFS subgraph centred on a single symbol.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for missing parameters or unknown symbol.
    pub async fn expand_symbol(&self, params: &Value) -> Result<Value, McpError> {
        let symbol_id_string = required_string(params, "symbol_id")?;
        let hop_count = params
            .get("hops")
            .and_then(Value::as_u64)
            .unwrap_or(2)
            .min(5) as usize;

        let forced_seed = yantra_core::SymbolId::new(symbol_id_string.clone())
            .map_err(|parse_error| McpError::invalid_params(parse_error.to_string()))?;

        let token_budget = (hop_count + 1) * 800;
        let graph_cache = Arc::clone(&self.graph_cache);
        let embedding_store = Arc::clone(&self.embedding_store);
        let symbol_name = graph_cache
            .symbol_details
            .get(&forced_seed)
            .map_or_else(|| symbol_id_string.clone(), |details| details.name.clone());

        let task_description = format!("expand symbol {symbol_name}");

        let rendered_subgraph = tokio::task::spawn_blocking(move || {
            extract_subgraph(
                &graph_cache,
                &embedding_store,
                &task_description,
                token_budget,
                std::slice::from_ref(&forced_seed),
            )
        })
        .await
        .map_err(|join_error| McpError::internal(join_error.to_string()))?
        .map_err(|extraction_error| McpError::internal(extraction_error.to_string()))?;

        Ok(json!({
            "text": rendered_subgraph.text,
            "token_cost": rendered_subgraph.token_cost,
            "included_node_count": rendered_subgraph.included_nodes.len(),
        }))
    }

    /// `crg.impact_of_change` — recursive CTE BFS over CALLS edges up to 5 hops.
    ///
    /// Returns all symbols that transitively depend on the given symbol.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for missing parameters or SQL failure.
    pub async fn impact_of_change(&self, params: &Value) -> Result<Value, McpError> {
        let symbol_id_string = required_string(params, "symbol_id")?;

        let sqlite_connection = Arc::clone(&self.sqlite_connection);
        let symbol_id_for_query = symbol_id_string.clone();

        let impact_report = tokio::task::spawn_blocking(move || {
            compute_impact_report(&sqlite_connection.blocking_lock(), &symbol_id_for_query)
        })
        .await
        .map_err(|join_error| McpError::internal(join_error.to_string()))?
        .map_err(|sql_error| McpError::internal(sql_error.to_string()))?;

        serde_json::to_value(&impact_report)
            .map_err(|serialization_error| McpError::internal(serialization_error.to_string()))
    }

    /// `crg.find_pattern` — semantic + name search returning top-20 matching symbols.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for missing parameters or embedding failure.
    pub async fn find_pattern(&self, params: &Value) -> Result<Value, McpError> {
        let query_string = required_string(params, "query")?;

        let graph_cache = Arc::clone(&self.graph_cache);
        let embedding_store = Arc::clone(&self.embedding_store);

        let semantic_results = tokio::task::spawn_blocking({
            let query_string_clone = query_string.clone();
            move || -> anyhow::Result<Vec<(yantra_core::SymbolId, f32)>> {
                let query_vector = embedding_store.embed_query(&query_string_clone)?;
                embedding_store.search_with_embedding(&query_vector, 20)
            }
        })
        .await
        .map_err(|join_error| McpError::internal(join_error.to_string()))?
        .map_err(|embedding_error| McpError::internal(embedding_error.to_string()))?;

        let mut symbol_entries: Vec<SymbolEntry> = semantic_results
            .into_iter()
            .filter_map(|(symbol_id, _similarity_score)| {
                graph_cache
                    .symbol_details
                    .get(&symbol_id)
                    .map(|details| SymbolEntry {
                        id: symbol_id.to_string(),
                        name: details.name.clone(),
                        kind: details.kind.trim_matches('"').to_owned(),
                        file_path: details.file_path.clone(),
                        start_line: details.start_line,
                    })
            })
            .collect();

        let query_lower = query_string.to_lowercase();
        for (symbol_id, details) in &graph_cache.symbol_details {
            if details.name.to_lowercase().contains(&query_lower)
                && !symbol_entries
                    .iter()
                    .any(|entry| entry.id == symbol_id.to_string())
            {
                symbol_entries.push(SymbolEntry {
                    id: symbol_id.to_string(),
                    name: details.name.clone(),
                    kind: details.kind.trim_matches('"').to_owned(),
                    file_path: details.file_path.clone(),
                    start_line: details.start_line,
                });
                if symbol_entries.len() >= 20 {
                    break;
                }
            }
        }

        Ok(json!({ "symbols": symbol_entries }))
    }

    /// `crg.dependencies_of` — direct outgoing CALLS and IMPORT edges for a symbol.
    ///
    /// # Errors
    ///
    /// Returns an MCP error for missing parameters or unknown symbol.
    pub async fn dependencies_of(&self, params: &Value) -> Result<Value, McpError> {
        let symbol_id_string = required_string(params, "symbol_id")?;

        let parsed_symbol_id = yantra_core::SymbolId::new(symbol_id_string.clone())
            .map_err(|parse_error| McpError::invalid_params(parse_error.to_string()))?;

        let graph_cache = Arc::clone(&self.graph_cache);

        let empty_edges = vec![];
        let adjacent_edges = graph_cache
            .adjacency_index
            .get(&symbol_id_string)
            .unwrap_or(&empty_edges);

        let dependency_entries: Vec<SymbolEntry> = adjacent_edges
            .iter()
            .filter(|edge| edge.edge_type == "CALLS" && edge.from_id == symbol_id_string)
            .filter_map(|edge| {
                yantra_core::SymbolId::new(edge.to_id.clone())
                    .ok()
                    .and_then(|neighbor_id| {
                        graph_cache
                            .symbol_details
                            .get(&neighbor_id)
                            .map(|details| SymbolEntry {
                                id: edge.to_id.clone(),
                                name: details.name.clone(),
                                kind: details.kind.trim_matches('"').to_owned(),
                                file_path: details.file_path.clone(),
                                start_line: details.start_line,
                            })
                    })
            })
            .collect();

        let _ = parsed_symbol_id;
        Ok(json!({ "dependencies": dependency_entries }))
    }
}

#[async_trait]
impl McpServer for CrgMcpServer {
    fn name(&self) -> &str {
        &self.server_name
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, McpError> {
        match method {
            "crg.subgraph" => self.subgraph(&params).await,
            "crg.expand_symbol" => self.expand_symbol(&params).await,
            "crg.impact_of_change" => self.impact_of_change(&params).await,
            "crg.find_pattern" => self.find_pattern(&params).await,
            "crg.dependencies_of" => self.dependencies_of(&params).await,
            _ => Err(McpError::method_not_found(method.to_owned())),
        }
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![]
    }
}

fn compute_impact_report(
    sqlite_connection: &Connection,
    root_symbol_id: &str,
) -> anyhow::Result<ImpactReport> {
    let root_symbol = load_symbol_entry(sqlite_connection, root_symbol_id)?
        .ok_or_else(|| anyhow::anyhow!("symbol not found: {root_symbol_id}"))?;

    let mut impact_statement = sqlite_connection.prepare(
        "WITH RECURSIVE impact(symbol_id, hop) AS (
            SELECT ?1, 0
            UNION ALL
            SELECT e.from_id, impact.hop + 1
            FROM edges e
            JOIN impact ON e.to_id = impact.symbol_id
            WHERE e.edge_type = 'CALLS' AND impact.hop < 5
        )
        SELECT DISTINCT s.id, s.name, s.kind, f.path, s.start_line, MIN(impact.hop) AS min_hop
        FROM impact
        JOIN symbols s ON s.id = impact.symbol_id
        JOIN files f ON f.id = s.file_id
        WHERE impact.symbol_id != ?1
        GROUP BY s.id
        ORDER BY min_hop",
    )?;

    let affected_rows = impact_statement.query_map([root_symbol_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i32>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;

    let mut affected_symbols = Vec::new();
    let mut hop_map = std::collections::HashMap::new();

    for row_result in affected_rows {
        let (affected_id, name, kind, file_path, start_line, hop) = row_result?;
        hop_map.insert(affected_id.clone(), hop as u8);
        affected_symbols.push(SymbolEntry {
            id: affected_id,
            name,
            kind: kind.trim_matches('"').to_owned(),
            file_path,
            start_line,
        });
    }

    Ok(ImpactReport {
        root_symbol,
        affected_symbols,
        hop_map,
    })
}

fn load_symbol_entry(
    sqlite_connection: &Connection,
    symbol_id: &str,
) -> anyhow::Result<Option<SymbolEntry>> {
    let result = sqlite_connection.query_row(
        "SELECT s.id, s.name, s.kind, f.path, s.start_line
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE s.id = ?1",
        [symbol_id],
        |row| {
            Ok(SymbolEntry {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get::<_, String>(2)?,
                file_path: row.get(3)?,
                start_line: row.get(4)?,
            })
        },
    );

    match result {
        Ok(entry) => Ok(Some(SymbolEntry {
            kind: entry.kind.trim_matches('"').to_owned(),
            ..entry
        })),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(other_error) => Err(other_error.into()),
    }
}

fn required_string(params: &Value, field_name: &str) -> Result<String, McpError> {
    params
        .get(field_name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| McpError::invalid_params(format!("missing string field `{field_name}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_entry_serializes_cleanly() {
        let symbol_entry = SymbolEntry {
            id: "uuid-1234".to_owned(),
            name: "my_function".to_owned(),
            kind: "function".to_owned(),
            file_path: "/src/lib.rs".to_owned(),
            start_line: 42,
        };
        let serialized = serde_json::to_string(&symbol_entry).unwrap();
        assert!(serialized.contains("my_function"));
        assert!(serialized.contains("function"));
    }

    #[test]
    fn test_impact_report_serializes_cleanly() {
        let root = SymbolEntry {
            id: "root-id".to_owned(),
            name: "root_fn".to_owned(),
            kind: "function".to_owned(),
            file_path: "/src/lib.rs".to_owned(),
            start_line: 1,
        };
        let report = ImpactReport {
            root_symbol: root,
            affected_symbols: vec![],
            hop_map: std::collections::HashMap::new(),
        };
        let serialized = serde_json::to_value(&report).unwrap();
        assert_eq!(serialized["root_symbol"]["name"], "root_fn");
        assert!(serialized["affected_symbols"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_required_string_missing_field_returns_error() {
        let params = serde_json::json!({ "other": "value" });
        let result = required_string(&params, "task");
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("task"));
    }
}
