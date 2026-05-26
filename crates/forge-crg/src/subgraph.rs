//! # Code-Review Graph: Subgraph Extraction and Adaptive Compression
//!
//! Implements the core subgraph extraction algorithm. Performs BFS traversal
//! up to 3 hops from seed symbols, tracks token costs using `forge-tokenizer`,
//! and applies adaptive compression (dropping low-degree non-forced symbols,
//! then dropping docstrings) to fit the final output within the token budget.
//!
//! ## Input
//! - `GraphCache` — pre-loaded in-memory symbol details and adjacency index
//! - Dense vector embedding store
//! - Natural language task description
//! - Token budget ceiling
//! - Pre-identified forced seed symbol identifiers
//!
//! ## Output
//! - A rendered text representation of the subgraph and its structural manifest
//!
//! ## Related
//! - `forge-crg::seed` — extracts the initial set of seed nodes
//! - `forge-crg::render` — defines the returned structure and manifest format

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::str::FromStr;

use rusqlite::Connection;
use yantra_core::SymbolId;
use yantra_tokenizer::count_tokens;

use crate::embedding::EmbeddingStore;
use serde::{Deserialize, Serialize};

use crate::render::{NodeProvenance, RenderedSubgraph, SubgraphManifest};
use crate::seed::{is_test_symbol, SeedExtractor, SeedSource};

/// One symbol row joined with its containing file path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolDetails {
    pub symbol_id: SymbolId,
    pub name: String,
    pub kind: String,
    pub start_line: i32,
    pub signature: Option<String>,
    pub docstring: Option<String>,
    pub connectivity_score: i32,
    pub file_path: String,
    pub file_id: String,
    pub token_cost_with_docstring: usize,
    pub token_cost_no_docstring: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeDetails {
    pub from_id: String,
    pub to_id: String,
    pub edge_type: String,
    pub weight: i32,
}

/// In-memory snapshot of the CRG symbol graph.
///
/// Build once after `GraphBuilder::build_from_repo`, then pass to every
/// `extract_subgraph` call. Invalidate and rebuild after `update_file`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphCache {
    /// All symbol details keyed by `SymbolId`.
    pub symbol_details: HashMap<SymbolId, SymbolDetails>,
    /// Bi-directional adjacency list: `symbol_id_string → Vec<EdgeDetails>`.
    pub adjacency_index: HashMap<String, Vec<EdgeDetails>>,
    /// Symbol name → list of matching `SymbolId`s (for name-match seed strategy).
    pub symbols_by_name: HashMap<String, Vec<SymbolId>>,
    /// `file_id → Vec<SymbolId>` (for forced-seed file expansion).
    pub symbols_by_file_id: HashMap<String, Vec<SymbolId>>,
    /// `SymbolId → file_id` (to resolve forced seeds to their containing file).
    pub symbol_to_file_id: HashMap<SymbolId, String>,
}

impl GraphCache {
    /// Builds the full in-memory graph cache from the given `SQLite` connection.
    /// Runs two queries: one for symbols (joined with files), one for edges.
    ///
    /// # Errors
    ///
    /// Returns an error if any `SQLite` query or row deserialization fails.
    pub fn build(sqlite_connection: &Connection) -> anyhow::Result<Self> {
        let mut select_symbols = sqlite_connection.prepare(
            "SELECT s.id, s.name, s.kind, s.start_line, s.signature, s.docstring,
                    s.connectivity_score, f.path, s.file_id
             FROM symbols s
             JOIN files f ON s.file_id = f.id",
        )?;

        let symbol_rows = select_symbols.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i32>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;

        let mut symbol_details: HashMap<SymbolId, SymbolDetails> = HashMap::new();
        let mut symbols_by_name: HashMap<String, Vec<SymbolId>> = HashMap::new();
        let mut symbols_by_file_id: HashMap<String, Vec<SymbolId>> = HashMap::new();
        let mut symbol_to_file_id: HashMap<SymbolId, String> = HashMap::new();

        for row_result in symbol_rows {
            let (
                id_string,
                name,
                kind,
                start_line,
                signature,
                docstring,
                connectivity_score,
                file_path,
                file_id,
            ) = row_result?;
            if let Ok(symbol_id) = SymbolId::from_str(&id_string) {
                symbols_by_name
                    .entry(name.clone())
                    .or_default()
                    .push(symbol_id.clone());
                symbols_by_file_id
                    .entry(file_id.clone())
                    .or_default()
                    .push(symbol_id.clone());
                symbol_to_file_id.insert(symbol_id.clone(), file_id.clone());

                let text_with_docstring = format_symbol_text(
                    &file_path,
                    &kind,
                    signature.as_deref(),
                    &name,
                    docstring.as_deref(),
                    true,
                    "",
                );
                let text_no_docstring = format_symbol_text(
                    &file_path,
                    &kind,
                    signature.as_deref(),
                    &name,
                    docstring.as_deref(),
                    false,
                    "",
                );
                let token_cost_with_docstring = count_tokens(&text_with_docstring);
                let token_cost_no_docstring = count_tokens(&text_no_docstring);

                symbol_details.insert(
                    symbol_id.clone(),
                    SymbolDetails {
                        symbol_id,
                        name,
                        kind,
                        start_line,
                        signature,
                        docstring,
                        connectivity_score,
                        file_path,
                        file_id,
                        token_cost_with_docstring,
                        token_cost_no_docstring,
                    },
                );
            }
        }

        let mut select_edges =
            sqlite_connection.prepare("SELECT from_id, to_id, edge_type, weight FROM edges")?;

        let edge_rows = select_edges.query_map([], |row| {
            Ok(EdgeDetails {
                from_id: row.get(0)?,
                to_id: row.get(1)?,
                edge_type: row.get(2)?,
                weight: row.get(3)?,
            })
        })?;

        let mut adjacency_index: HashMap<String, Vec<EdgeDetails>> = HashMap::new();
        for row_result in edge_rows {
            let edge = row_result?;
            adjacency_index
                .entry(edge.from_id.clone())
                .or_default()
                .push(edge.clone());
            adjacency_index
                .entry(edge.to_id.clone())
                .or_default()
                .push(edge);
        }

        Ok(Self {
            symbol_details,
            adjacency_index,
            symbols_by_name,
            symbols_by_file_id,
            symbol_to_file_id,
        })
    }
}

/// Extracts a token-budgeted subgraph for `task_description` from the pre-built
/// `graph_cache`. All SQL I/O is performed during `GraphCache::build`; this
/// function executes entirely in memory.
///
/// # Errors
///
/// Returns an error if seed extraction or embedding lookup fails.
#[derive(Debug, Eq, PartialEq)]
struct QueueItem {
    symbol_id: SymbolId,
    hop_distance: usize,
    connectivity_score: i32,
}

impl Ord for QueueItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // We want smaller hop_distance first, so compare other with self
        other
            .hop_distance
            .cmp(&self.hop_distance)
            // If hop_distance is the same, compare connectivity_score (higher first)
            .then_with(|| self.connectivity_score.cmp(&other.connectivity_score))
    }
}

impl PartialOrd for QueueItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub fn extract_subgraph(
    graph_cache: &GraphCache,
    embedding_store: &EmbeddingStore,
    task_description: &str,
    token_budget: usize,
    forced_seeds: &[SymbolId],
) -> anyhow::Result<RenderedSubgraph> {
    let seeds =
        SeedExtractor::extract_seeds(graph_cache, embedding_store, task_description, forced_seeds)?;

    let mut node_provenances: HashMap<SymbolId, (Option<SeedSource>, usize)> = HashMap::new();
    let mut priority_queue: BinaryHeap<QueueItem> = BinaryHeap::new();
    let mut visited_symbols: HashSet<SymbolId> = HashSet::new();

    for (symbol_identifier, seed_source) in &seeds {
        if let Some(details) = graph_cache.symbol_details.get(symbol_identifier) {
            priority_queue.push(QueueItem {
                symbol_id: symbol_identifier.clone(),
                hop_distance: 0,
                connectivity_score: details.connectivity_score,
            });
            visited_symbols.insert(symbol_identifier.clone());
            node_provenances.insert(symbol_identifier.clone(), (Some(*seed_source), 0));
        }
    }

    let mut candidate_symbols: HashSet<SymbolId> = HashSet::new();
    let mut candidate_edges: HashSet<EdgeDetails> = HashSet::new();

    while let Some(QueueItem {
        symbol_id: current_symbol_id,
        hop_distance: current_hop,
        ..
    }) = priority_queue.pop()
    {
        let symbol_details = match graph_cache.symbol_details.get(&current_symbol_id) {
            Some(details) => details,
            None => continue,
        };

        let symbol_token_cost = symbol_details.token_cost_with_docstring;

        let is_forced = node_provenances
            .get(&current_symbol_id)
            .is_some_and(|provenance| provenance.0 == Some(SeedSource::Forced));

        if !is_forced && symbol_token_cost > token_budget {
            continue;
        }

        candidate_symbols.insert(current_symbol_id.clone());

        if current_hop < 3 {
            let current_id_string = current_symbol_id.to_string();
            let empty_vec = Vec::new();
            let neighbor_edges = graph_cache
                .adjacency_index
                .get(&current_id_string)
                .unwrap_or(&empty_vec);

            let mut edges_list: Vec<EdgeDetails> = neighbor_edges.clone();
            edges_list.sort_by(|first, second| {
                let first_is_calls = first.edge_type == "CALLS";
                let second_is_calls = second.edge_type == "CALLS";
                match (first_is_calls, second_is_calls) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => second.weight.cmp(&first.weight),
                }
            });

            for edge in edges_list.into_iter().take(10) {
                let neighbor_id_string = if edge.from_id == current_id_string {
                    edge.to_id.clone()
                } else {
                    edge.from_id.clone()
                };

                if let Ok(neighbor_id) = SymbolId::from_str(&neighbor_id_string) {
                    if let Some(neighbor_details) = graph_cache.symbol_details.get(&neighbor_id) {
                        if is_test_symbol(&neighbor_id, graph_cache) {
                            continue;
                        }
                        candidate_edges.insert(edge);
                        if !visited_symbols.contains(&neighbor_id) {
                            visited_symbols.insert(neighbor_id.clone());
                            node_provenances.insert(neighbor_id.clone(), (None, current_hop + 1));
                            priority_queue.push(QueueItem {
                                symbol_id: neighbor_id,
                                hop_distance: current_hop + 1,
                                connectivity_score: neighbor_details.connectivity_score,
                            });
                        }
                    }
                }
            }
        }
    }

    let mut pruned_symbols: HashSet<SymbolId> = HashSet::new();
    let mut include_docstrings = true;

    let (final_rendered_text, final_token_cost) = loop {
        let active_symbols: HashSet<SymbolId> = candidate_symbols
            .iter()
            .filter(|symbol_id| !pruned_symbols.contains(*symbol_id))
            .cloned()
            .collect();

        let rendered_text = render_subgraph_text(
            &active_symbols,
            &candidate_edges,
            &graph_cache.symbol_details,
            &node_provenances,
            include_docstrings,
        );

        let token_cost = count_tokens(&rendered_text);

        if token_cost <= token_budget {
            break (rendered_text, token_cost);
        }

        let mut pruneable_symbols: Vec<&SymbolDetails> = candidate_symbols
            .iter()
            .filter(|symbol_id| !pruned_symbols.contains(*symbol_id))
            .filter(|symbol_id| {
                let is_seed = node_provenances
                    .get(*symbol_id)
                    .is_some_and(|provenance| provenance.0.is_some());
                !is_seed
            })
            .filter_map(|symbol_id| graph_cache.symbol_details.get(symbol_id))
            .collect();

        if !pruneable_symbols.is_empty() {
            pruneable_symbols.sort_by(|first_details, second_details| {
                let first_hop_distance = node_provenances
                    .get(&first_details.symbol_id)
                    .map_or(0, |(_, hop)| *hop);
                let second_hop_distance = node_provenances
                    .get(&second_details.symbol_id)
                    .map_or(0, |(_, hop)| *hop);

                second_hop_distance.cmp(&first_hop_distance).then_with(|| {
                    first_details
                        .connectivity_score
                        .cmp(&second_details.connectivity_score)
                })
            });
            let prune_count = (pruneable_symbols.len() / 10).max(1);
            for symbol_to_prune in pruneable_symbols.iter().take(prune_count) {
                pruned_symbols.insert(symbol_to_prune.symbol_id.clone());
            }
        } else if include_docstrings {
            include_docstrings = false;
        } else {
            break (rendered_text, token_cost);
        }
    };

    let mut final_included_nodes = Vec::new();
    let mut manifest_provenance = Vec::new();

    for (symbol_id, (seed_source, hop_distance)) in node_provenances {
        let retained =
            candidate_symbols.contains(&symbol_id) && !pruned_symbols.contains(&symbol_id);
        if retained {
            final_included_nodes.push(symbol_id.clone());
        }
        manifest_provenance.push(NodeProvenance {
            symbol_id,
            seed_source,
            hop_distance,
            retained,
        });
    }

    Ok(RenderedSubgraph {
        text: final_rendered_text,
        included_nodes: final_included_nodes,
        token_cost: final_token_cost,
        manifest: SubgraphManifest {
            nodes: manifest_provenance,
        },
    })
}

fn format_symbol_text(
    file_path: &str,
    kind: &str,
    signature: Option<&str>,
    name: &str,
    docstring: Option<&str>,
    include_docstring: bool,
    provenance_prefix: &str,
) -> String {
    let cleaned_kind = kind.trim_matches('"');
    let mut rendered = format!("{provenance_prefix}{file_path} :: {cleaned_kind}");
    if let Some(sig) = signature {
        rendered.push_str(&format!(" {name}({sig})"));
    } else {
        rendered.push_str(&format!(" {name}"));
    }
    if include_docstring {
        if let Some(doc) = docstring {
            let first_line = doc.lines().next().unwrap_or("").trim();
            if !first_line.is_empty() {
                rendered.push_str(&format!("  // {first_line}"));
            }
        }
    }
    rendered
}

fn format_single_symbol(
    symbol: &SymbolDetails,
    include_docstring: bool,
    provenance_prefix: &str,
) -> String {
    format_symbol_text(
        &symbol.file_path,
        &symbol.kind,
        symbol.signature.as_deref(),
        &symbol.name,
        symbol.docstring.as_deref(),
        include_docstring,
        provenance_prefix,
    )
}

fn render_subgraph_text(
    symbols: &HashSet<SymbolId>,
    edges: &HashSet<EdgeDetails>,
    details_map: &HashMap<SymbolId, SymbolDetails>,
    node_provenances: &HashMap<SymbolId, (Option<SeedSource>, usize)>,
    include_docstrings: bool,
) -> String {
    let mut file_groups: HashMap<String, Vec<&SymbolDetails>> = HashMap::new();
    for symbol_id in symbols {
        if let Some(details) = details_map.get(symbol_id) {
            file_groups
                .entry(details.file_path.clone())
                .or_default()
                .push(details);
        }
    }

    let mut rendered_output = String::new();

    let mut sorted_files: Vec<String> = file_groups.keys().cloned().collect();
    sorted_files.sort();

    for file_path in sorted_files {
        if let Some(mut file_symbols) = file_groups.remove(&file_path) {
            file_symbols.sort_by_key(|symbol| symbol.start_line);
            for symbol in file_symbols {
                let (seed_source, hop_distance) = node_provenances
                    .get(&symbol.symbol_id)
                    .copied()
                    .unwrap_or((None, 0));

                let provenance_prefix = match seed_source {
                    Some(source) => format!(
                        "[SEED:{}, score:{}] ",
                        source.as_str(),
                        symbol.connectivity_score
                    ),
                    None => format!(
                        "[hop:{}, score:{}] ",
                        hop_distance, symbol.connectivity_score
                    ),
                };

                let formatted =
                    format_single_symbol(symbol, include_docstrings, &provenance_prefix);
                rendered_output.push_str(&formatted);
                rendered_output.push('\n');
            }
        }
    }

    let mut rendered_edges = Vec::new();
    for edge in edges {
        if edge.edge_type == "CALLS" {
            let from_symbol_id = SymbolId::from_str(&edge.from_id).ok();
            let to_symbol_id = SymbolId::from_str(&edge.to_id).ok();

            if let (Some(from_id), Some(to_id)) = (from_symbol_id, to_symbol_id) {
                if symbols.contains(&from_id) && symbols.contains(&to_id) {
                    if let (Some(from_details), Some(to_details)) =
                        (details_map.get(&from_id), details_map.get(&to_id))
                    {
                        rendered_edges
                            .push(format!("{} -calls→ {}", from_details.name, to_details.name));
                    }
                }
            }
        }
    }

    rendered_edges.sort();
    for edge_string in rendered_edges {
        rendered_output.push_str(&edge_string);
        rendered_output.push('\n');
    }

    rendered_output
}
