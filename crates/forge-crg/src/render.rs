//! # Code-Review Graph: Subgraph Rendering Structures
//!
//! Defines the output containers for the extracted subgraph, including the raw
//! text payload, token costs, and node provenance manifest.
//!
//! ## Input
//! - None
//!
//! ## Output
//! - Serialized or structured subgraph data representations
//!
//! ## Related
//! - `forge-crg::subgraph` — populates and generates these structures

use serde::{Deserialize, Serialize};
use yantra_core::SymbolId;

use crate::seed::SeedSource;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeProvenance {
    pub symbol_id: SymbolId,
    pub seed_source: Option<SeedSource>,
    pub hop_distance: usize,
    pub retained: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubgraphManifest {
    pub nodes: Vec<NodeProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedSubgraph {
    pub text: String,
    pub included_nodes: Vec<SymbolId>,
    pub token_cost: usize,
    pub manifest: SubgraphManifest,
}
