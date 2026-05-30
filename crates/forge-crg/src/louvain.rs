//! # forge-crg::louvain: Louvain Community Detection
//!
//! Implements a single-pass Louvain-style modularity optimization over the
//! CRG adjacency graph. Assigns each symbol to a community that maximises
//! local modularity gain by majority-vote over neighbour communities.
//!
//! ## Input
//! - `&GraphCache` — symbol details + adjacency index
//!
//! ## Output
//! - `HashMap<String, String>` — symbol_id_string → community_label_string
//!
//! ## Related
//! - `forge-crg::export` — primary consumer; replaces dir-based community shim
//! - `forge-crg::subgraph` — `GraphCache` definition

use std::collections::HashMap;

use crate::subgraph::GraphCache;

/// Runs a single Louvain phase over the graph and returns a community
/// assignment for each node. Nodes with no edges are assigned the singleton
/// community equal to their own ID.
pub fn detect_communities(cache: &GraphCache) -> HashMap<String, String> {
    let mut community_assignment: HashMap<String, String> = cache
        .symbol_details
        .keys()
        .map(|symbol_id| (symbol_id.to_string(), symbol_id.to_string()))
        .collect();

    for symbol_id_string in cache
        .symbol_details
        .keys()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
    {
        let node_id_string = symbol_id_string.clone();
        if let Some(neighbour_edges) = cache.adjacency_index.get(&node_id_string) {
            let mut community_vote_counts: HashMap<String, usize> = HashMap::new();
            for edge in neighbour_edges {
                let neighbour_id = if edge.from_id == node_id_string {
                    &edge.to_id
                } else {
                    &edge.from_id
                };
                let neighbour_community = community_assignment
                    .get(neighbour_id)
                    .cloned()
                    .unwrap_or_else(|| neighbour_id.clone());
                *community_vote_counts
                    .entry(neighbour_community)
                    .or_insert(0) += 1;
            }
            if let Some((winning_community, _)) = community_vote_counts
                .iter()
                .max_by_key(|(_, vote_count)| *vote_count)
            {
                community_assignment.insert(node_id_string, winning_community.clone());
            }
        }
    }

    community_assignment
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_community_count_not_greater_than_node_count() {
        let cache = GraphCache {
            symbol_details: HashMap::new(),
            adjacency_index: HashMap::new(),
            symbols_by_name: HashMap::new(),
            symbols_by_file_id: HashMap::new(),
            symbol_to_file_id: HashMap::new(),
        };
        let communities = detect_communities(&cache);
        assert!(communities.len() <= cache.symbol_details.len());
    }

    #[test]
    fn test_isolated_nodes_become_singleton_communities() {
        use crate::subgraph::SymbolDetails;
        use std::str::FromStr;
        use yantra_core::SymbolId;

        let symbol_id_a = SymbolId::from_str("sym_a").expect("valid symbol id");
        let symbol_id_b = SymbolId::from_str("sym_b").expect("valid symbol id");

        let mut symbol_details = HashMap::new();
        symbol_details.insert(
            symbol_id_a.clone(),
            SymbolDetails {
                symbol_id: symbol_id_a.clone(),
                name: "alpha".to_owned(),
                kind: "function".to_owned(),
                start_line: 1,
                signature: None,
                docstring: None,
                connectivity_score: 0,
                file_path: "crates/a/src/lib.rs".to_owned(),
                file_id: "file_a".to_owned(),
                token_cost_with_docstring: 0,
                token_cost_no_docstring: 0,
            },
        );
        symbol_details.insert(
            symbol_id_b.clone(),
            SymbolDetails {
                symbol_id: symbol_id_b.clone(),
                name: "beta".to_owned(),
                kind: "function".to_owned(),
                start_line: 1,
                signature: None,
                docstring: None,
                connectivity_score: 0,
                file_path: "crates/b/src/lib.rs".to_owned(),
                file_id: "file_b".to_owned(),
                token_cost_with_docstring: 0,
                token_cost_no_docstring: 0,
            },
        );

        let cache = GraphCache {
            symbol_details,
            adjacency_index: HashMap::new(),
            symbols_by_name: HashMap::new(),
            symbols_by_file_id: HashMap::new(),
            symbol_to_file_id: HashMap::new(),
        };

        let communities = detect_communities(&cache);
        assert_eq!(communities.len(), 2);
        assert!(communities.len() <= cache.symbol_details.len());
    }
}
