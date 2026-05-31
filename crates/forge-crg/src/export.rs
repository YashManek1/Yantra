//! # forge-crg::export: vis.js-shaped Graph JSON
//!
//! Exports a `GraphCache` into the JSON shape expected by the browser CRG
//! viewer. Each node carries a community ID assigned by the Louvain community
//! detection algorithm (single-pass modularity optimisation) and a hub score
//! derived from the precomputed `connectivity_score`.
//!
//! ## Input
//! - `GraphCache` — full or focused symbol cache
//! - Optional focus `SymbolId` + hop count for BFS-bounded subgraph export
//!
//! ## Output
//! - `GraphJson { nodes, edges }` — directly serializable as vis.js data
//!
//! ## Related
//! - `forge-crg::louvain` — provides the community assignment map
//! - `forge-crg::subgraph::GraphCache` — input source
//! - `forge-canvas::graph_viz` — primary downstream consumer

use std::collections::{HashMap, HashSet, VecDeque};
use std::str::FromStr;

use serde::Serialize;
use yantra_core::SymbolId;

use crate::louvain::detect_communities;
use crate::subgraph::{EdgeDetails, GraphCache, SymbolDetails};

/// Serializable graph payload for vis.js / d3.
#[derive(Debug, Clone, Serialize)]
pub struct GraphJson {
    pub nodes: Vec<GraphNodeJson>,
    pub edges: Vec<GraphEdgeJson>,
}

/// One node in the exported graph.
#[derive(Debug, Clone, Serialize)]
pub struct GraphNodeJson {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub file: String,
    pub community: String,
    pub hub_score: i32,
    pub color: String,
}

/// One directed edge in the exported graph.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdgeJson {
    pub from: String,
    pub to: String,
    pub edge_type: String,
    pub weight: i32,
}

const PALETTE: &[&str] = &[
    "#f59e0b", "#3b82f6", "#10b981", "#ec4899", "#8b5cf6", "#ef4444", "#14b8a6", "#f97316",
    "#84cc16", "#06b6d4", "#a855f7", "#eab308", "#22c55e", "#0ea5e9",
];

/// Exports the full graph as JSON.
pub fn export_graph(cache: &GraphCache) -> GraphJson {
    let included_ids: HashSet<String> = cache
        .symbol_details
        .keys()
        .map(SymbolId::to_string)
        .collect();
    build_payload(cache, &included_ids)
}

/// Exports a BFS-bounded subgraph around `focus_id`, traversing up to `hops`.
///
/// # Errors
///
/// Returns `Err(String)` when `focus_id` cannot be parsed as a `SymbolId`
/// or is not present in the graph cache.
pub fn export_focused(
    cache: &GraphCache,
    focus_id: &str,
    hops: usize,
) -> Result<GraphJson, String> {
    let focus_symbol_id =
        SymbolId::from_str(focus_id).map_err(|err| format!("invalid focus symbol id: {err}"))?;

    if !cache.symbol_details.contains_key(&focus_symbol_id) {
        return Err(format!("focus symbol {focus_id} not in cache"));
    }

    let mut included: HashSet<String> = HashSet::new();
    included.insert(focus_id.to_owned());

    let mut frontier: VecDeque<(String, usize)> = VecDeque::new();
    frontier.push_back((focus_id.to_owned(), 0));

    while let Some((current_id, current_hop)) = frontier.pop_front() {
        if current_hop >= hops {
            continue;
        }
        if let Some(edges) = cache.adjacency_index.get(&current_id) {
            for edge in edges {
                let neighbor_id = if edge.from_id == current_id {
                    &edge.to_id
                } else {
                    &edge.from_id
                };
                if included.insert(neighbor_id.clone()) {
                    frontier.push_back((neighbor_id.clone(), current_hop + 1));
                }
            }
        }
    }

    Ok(build_payload(cache, &included))
}

fn build_payload(cache: &GraphCache, included_ids: &HashSet<String>) -> GraphJson {
    let louvain_assignment = detect_communities(cache);

    let community_for: HashMap<String, String> = cache
        .symbol_details
        .iter()
        .filter(|(symbol_id, _)| included_ids.contains(&symbol_id.to_string()))
        .map(|(symbol_id, details)| {
            let id_string = symbol_id.to_string();
            let community = louvain_assignment
                .get(&id_string)
                .cloned()
                .unwrap_or_else(|| community_label(&details.file_path));
            (id_string, community)
        })
        .collect();

    let mut nodes: Vec<GraphNodeJson> = cache
        .symbol_details
        .iter()
        .filter(|(symbol_id, _)| included_ids.contains(&symbol_id.to_string()))
        .map(|(symbol_id, details)| node_for(symbol_id, details, &community_for))
        .collect();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));

    let mut emitted_edge_keys: HashSet<(String, String, String)> = HashSet::new();
    let mut edges: Vec<GraphEdgeJson> = Vec::new();
    for edge_list in cache.adjacency_index.values() {
        for edge in edge_list {
            if !included_ids.contains(&edge.from_id) || !included_ids.contains(&edge.to_id) {
                continue;
            }
            let key = (
                edge.from_id.clone(),
                edge.to_id.clone(),
                edge.edge_type.clone(),
            );
            if !emitted_edge_keys.insert(key.clone()) {
                continue;
            }
            edges.push(edge_for(edge));
        }
    }
    edges.sort_by(|left, right| {
        (left.from.as_str(), left.to.as_str()).cmp(&(right.from.as_str(), right.to.as_str()))
    });

    GraphJson { nodes, edges }
}

fn node_for(
    symbol_id: &SymbolId,
    details: &SymbolDetails,
    community_for: &HashMap<String, String>,
) -> GraphNodeJson {
    let id_str = symbol_id.to_string();
    let community = community_for
        .get(&id_str)
        .cloned()
        .unwrap_or_else(|| "root".to_owned());
    let color = palette_color(&community);
    GraphNodeJson {
        id: id_str,
        label: details.name.clone(),
        kind: details.kind.clone(),
        file: details.file_path.clone(),
        community,
        hub_score: details.connectivity_score,
        color,
    }
}

fn edge_for(edge: &EdgeDetails) -> GraphEdgeJson {
    GraphEdgeJson {
        from: edge.from_id.clone(),
        to: edge.to_id.clone(),
        edge_type: edge.edge_type.clone(),
        weight: edge.weight,
    }
}

/// Derives a short community name from a file path by returning the first
/// path segment that is neither empty, `"src"`, nor a dotfile prefix.
/// Falls back to `"root"` when no meaningful segment exists.
pub fn community_label(file_path: &str) -> String {
    let normalized = file_path.replace('\\', "/");
    normalized
        .split('/')
        .find(|segment| !segment.is_empty() && *segment != "src" && !segment.starts_with('.'))
        .unwrap_or("root")
        .to_owned()
}

fn palette_color(community_label: &str) -> String {
    let mut hash: u32 = 0;
    for byte in community_label.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    let index = (hash as usize) % PALETTE.len();
    PALETTE[index].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn community_label_uses_first_meaningful_segment() {
        assert_eq!(community_label("crates/forge-core/src/lib.rs"), "crates");
        assert_eq!(community_label("src/main.rs"), "main.rs");
        assert_eq!(community_label("./hidden/foo.rs"), "hidden");
    }

    #[test]
    fn palette_color_deterministic() {
        let a = palette_color("crates");
        let b = palette_color("crates");
        assert_eq!(a, b);
    }
}
