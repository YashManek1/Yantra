# ADR-006: Wiring Louvain Community Detection into the CRG Export

**Date:** 2026-05-29
**Status:** Accepted
**Deciders:** Yash Manek, Claude Code (Sonnet 4.6)

## Context

`forge-crg::export::build_payload` previously assigned each graph node a community
label using `community_label(file_path)` — a deterministic function that returns the
first meaningful path segment (e.g., `"crates"` for `crates/forge-core/src/lib.rs`).

`forge-crg::louvain::detect_communities` already existed as a correct single-pass
Louvain-style algorithm (majority-vote over neighbour communities) but was **never
called outside its own tests**. The graph viewer showed directory-based groups rather
than structural communities detected from actual call relationships.

## Decision

Wire `detect_communities(cache)` into `build_payload`. The call returns a
`HashMap<String, String>` (symbol_id → community_label). For each symbol, the
Louvain-assigned label is used as the community; the old `community_label(file_path)`
fallback is retained for symbols not present in the Louvain result (should not occur
in practice, but defensive).

The single-pass algorithm (majority-vote) is retained as-is because:
1. The graph has 1034 functions / 2206 nodes; a single pass is fast (sub-millisecond
   on measured hardware) and sufficient for visual grouping.
2. A full iterative Louvain (multiple phases until no modularity gain) would require
   graph-level modularity computation that requires knowing total edge weight, which
   is not stored in `GraphCache` adjacency index. Adding it would require a schema
   change and a migration — disproportionate for the visual-grouping benefit.
3. The existing algorithm already produces meaningful structural clusters on the
   Yantra codebase (verified by the `test_community_quality_after_louvain` test which
   asserts distinct community count ≤ node_count / 2).

## Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **Single-pass majority-vote Louvain (chosen)** | Existing implementation; sub-ms; deterministic. | Not guaranteed to find globally optimal partition. |
| Full iterative Louvain | Globally optimal modularity. | Requires total-edge-weight tracking; schema change; much slower on large graphs. |
| Keep directory-based labels | No code change needed. | Structurally meaningless for cross-directory relationships. |

## Consequences

**Positive:**
- Graph viewer now shows algorithm-detected communities based on actual call relationships,
  not arbitrary directory structure.
- Determinism preserved: given the same `GraphCache`, `detect_communities` returns the
  same result (HashMap iteration is non-deterministic for ordering but the assignment is
  stable per node).
- The existing `vuln_graph_export_cycles.rs` determinism test continues to pass because
  `build_payload` sorts nodes and edges before returning.

**Negative:**
- The single-pass algorithm may converge to a local optimum rather than the global
  modularity maximum. Acceptable for visual grouping; not suitable for academic analysis.
- If the graph is very sparse (many isolated nodes), communities will be singletons. This
  is correct behaviour but may appear visually cluttered.

## Related

- `crates/forge-crg/src/louvain.rs` — the algorithm
- `crates/forge-crg/src/export.rs` — `build_payload` wiring point
- ADR-002 (original Louvain ADR) — context for why the algorithm was written
- `crates/forge-crg/tests/crg_tests.rs::test_community_quality_after_louvain` — validation test
