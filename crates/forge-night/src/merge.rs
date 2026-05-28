//! # forge-night: Semantic Merge Lite
//!
//! Re-exports the unified diff merge utilities from `forge-core::diff` for
//! use within Night Mode. When two Night tasks complete out of dependency order
//! and both add symbols to the same `impl` block, this module's
//! `merge_additive_patches` union-merges them alphabetically so that git sees
//! a single clean diff rather than two competing branches.
//!
//! ## Input
//! - Two unified diff strings from Night task outputs
//!
//! ## Output
//! - `MergeResult` — merged patches ready for application, plus deferred paths
//!
//! ## Related
//! - `forge-core::diff` — contains the full algorithm and unit tests
//! - `forge-tools::tools::merge` — exposes this via the MCP tool layer

pub use yantra_core::diff::{
    merge_additive_patches, parse_unified_diff, DiffHunk, FilePatch, HunkLine, HunkLineKind,
    MergeResult,
};
