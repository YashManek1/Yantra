# ADR-002: Replace dir-based community shim with Louvain community detection in forge-crg

**Date:** 2026-05-29
**Status:** Accepted
**Deciders:** Yash Manek (Sankalp Systems)

## Context

The original `community_label` field in the CRG schema used the top-level directory name of a symbol's source file as a proxy for community membership. This approach has two problems:

1. It is semantically coarse: two symbols in `crates/forge-stvp/src/` and `crates/forge-stvp/tests/` get different community labels even though they belong to the same logical component.
2. The community labeling logic was duplicated across two files (`builder.rs` and `subgraph.rs`), violating the no-duplication rule in CLAUDE.md §3.2.

Louvain community detection is a standard algorithm for finding communities in undirected graphs by maximizing modularity. Applying it to the CRG's `CALLS`, `IMPORTS`, and `IMPLEMENTS` edges produces semantically meaningful communities that reflect actual coupling rather than directory structure.

## Decision

Implement a single-pass Louvain algorithm in `forge-crg/src/louvain.rs`. The `community_label` field is populated by Louvain at index-build time. All other code reads the single stored label; the duplicate directory-based logic is removed.

## Options Considered

| Option | Pros | Cons |
|--------|------|------|
| Dir-based shim (status quo) | Zero compute cost | Coarse, duplicated, misleading for cross-dir modules |
| Louvain (single-pass) | Semantically meaningful, single source of truth | O(n log n) compute at index time |
| Label propagation | Faster than Louvain | Less stable; different runs can produce different labels |
| Spectral clustering | High accuracy | Requires dense matrix; too slow for 100K-LoC graphs |

## Consequences

**Positive:**
- Community labels reflect actual coupling patterns, improving CRG subgraph quality.
- Single source of truth for `community_label` — no duplication.
- Enables `list_communities` MCP tool to return semantically meaningful groups.

**Negative:**
- Index build time increases by O(n log n) for the Louvain pass (expected <500ms for 100K-LoC projects).
- Community labels are non-deterministic across runs if graph has tie-breaking ambiguity; acceptable for Yantra's use case.

## Related

- `crates/forge-crg/src/louvain.rs` — implementation
- `crates/forge-crg/src/builder.rs` — calls Louvain post-parse
- ADR-001 for benchmark infrastructure used to measure the Louvain build-time cost
- `mcp__code-review-graph__list_communities_tool` — consumer of community labels
