//! # forge-crg: Code-Review Graph
//!
//! Builds and queries the typed structural graph that compresses a 200K-token
//! repository into a 3-4K token subgraph for any given task. The graph is
//! stored in `crg.sqlite` and updated incrementally by the sidecar.
//!
//! ## Input
//! - `Symbol` records from `forge-ast`
//! - A task description and `token_budget` for subgraph extraction
//! - Optional `forced_seeds: Vec<SymbolId>` from `forge-stvp`
//!
//! ## Output
//! - `RenderedSubgraph` — flat textual rendering plus a manifest of
//!   included nodes, ready for injection into an agent prompt
//! - `ImpactReport` — blast-radius analysis for a changed symbol
//!
//! ## Related
//! - `forge-ast` — provides symbol and edge input
//! - `forge-stvp` — supplies forced seeds via `existing_truth_refs`
//! - `forge-verifier` — queries the graph for hallucination checks
pub mod builder;
pub mod connectivity;
pub mod embedding;
pub mod error;
pub mod export;
pub mod render;
pub mod schema;
pub mod seed;
pub mod subgraph;

pub use builder::GraphBuilder;
pub use connectivity::compute_connectivity;
pub use embedding::EmbeddingStore;
pub use error::{CrgError, CrgResult};
pub use export::{export_focused, export_graph, GraphEdgeJson, GraphJson, GraphNodeJson};
pub use render::{NodeProvenance, RenderedSubgraph, SubgraphManifest};
pub use schema::{create_crg_schema, CRG_SCHEMA_SQL};
pub use seed::{SeedExtractor, SeedSource};
pub use subgraph::{extract_subgraph, EdgeDetails, GraphCache, SymbolDetails};
