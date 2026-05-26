//! # Code-Review Graph: Seed Symbol Extractor
//!
//! Extracts seed nodes to initiate the subgraph BFS traversal. Utilizes three
//! sequential strategies: semantic cosine similarity matching over symbol names
//! and docstrings, forced file-level symbols defined by STVP, and regular
//! expression name matching over the task description. All lookups are performed
//! against the pre-loaded `GraphCache` — no SQL is executed at search time.
//!
//! ## Input
//! - `GraphCache` — pre-loaded in-memory symbol graph
//! - Task description text
//! - Existing forced symbols from STVP
//!
//! ## Output
//! - A list of deduplicated symbol identifiers and their extraction provenance
//!
//! ## Related
//! - `forge-crg::embedding` — performs semantic vector embedding searches
//! - `forge-crg::subgraph` — consumes the extracted seeds to start BFS traversal

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use regex::Regex;
use yantra_core::SymbolId;
use crate::embedding::EmbeddingStore;
use crate::subgraph::GraphCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SeedSource {
    Semantic,
    Forced,
    Name,
}

impl SeedSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SeedSource::Semantic => "semantic",
            SeedSource::Forced => "forced",
            SeedSource::Name => "name",
        }
    }
}

pub struct SeedExtractor;

impl SeedExtractor {
    /// Extracts seed symbols using three sequential strategies against the
    /// pre-loaded `GraphCache`. No OS threads are spawned; all lookups are
    /// pure in-memory HashMap operations after the initial `embed_query` call.
    ///
    /// Priority (highest wins on collision): Forced > Semantic > Name.
    pub fn extract_seeds(
        graph_cache: &GraphCache,
        embedding_store: &EmbeddingStore,
        task_description: &str,
        forced_seeds: &[SymbolId],
    ) -> anyhow::Result<Vec<(SymbolId, SeedSource)>> {
        let mut deduplicated_seeds: HashMap<SymbolId, SeedSource> = HashMap::new();

        let identifier_regex = Regex::new(r"\b([A-Z][a-zA-Z]+|[a-z]+(_[a-z]+)+)\b").ok();
        if let Some(regex_matcher) = identifier_regex {
            for captures in regex_matcher.captures_iter(task_description) {
                if let Some(matched_group) = captures.get(1) {
                    let matched_name = matched_group.as_str();
                    if let Some(symbol_ids) = graph_cache.symbols_by_name.get(matched_name) {
                        for symbol_id in symbol_ids {
                            deduplicated_seeds.insert(symbol_id.clone(), SeedSource::Name);
                        }
                    }
                }
            }
        }

        let query_vector = embedding_store.embed_query(task_description)?;
        let semantic_matches = embedding_store.search_with_embedding(&query_vector, 10)?;
        for (symbol_identifier, _) in semantic_matches {
            deduplicated_seeds.insert(symbol_identifier, SeedSource::Semantic);
        }

        for seed in forced_seeds {
            if let Some(file_id) = graph_cache.symbol_to_file_id.get(seed) {
                if let Some(file_symbols) = graph_cache.symbols_by_file_id.get(file_id) {
                    for symbol_id in file_symbols {
                        deduplicated_seeds.insert(symbol_id.clone(), SeedSource::Forced);
                    }
                }
            } else {
                let seed_string = seed.to_string();
                if let Some(file_symbols) = graph_cache.symbols_by_file_id.get(&seed_string) {
                    for symbol_id in file_symbols {
                        deduplicated_seeds.insert(symbol_id.clone(), SeedSource::Forced);
                    }
                }
            }
        }

        Ok(deduplicated_seeds.into_iter().collect())
    }
}
