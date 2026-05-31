`forge-crg` is the Code-Review Graph: Yantra's structural code-intelligence layer. It drives Tree-sitter (via `forge-ast`) over every source file to extract typed symbols and infers four edge kinds (`CALLS`, `IMPORTS`, `IMPLEMENTS`, `TESTS`) which are persisted in `.yantra/crg.sqlite`. Subgraph extraction runs a weighted BFS from seed symbols, bounded by a caller-supplied token budget, and returns a `RenderedSubgraph` that agents receive instead of naive whole-repo context — compressing a 200K-token codebase to a 3–4K-token slice. Community detection uses a Louvain algorithm (`louvain.rs`) so `community_label` reflects actual structural coupling rather than directory structure. The crate also exports the full graph (or a focused subgraph) as a vis.js-compatible `GraphJson` via `export_graph` and `export_focused`, served by `forge-canvas` at `/api/graph/json` for interactive browser visualization.

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| Subgraph extraction recall (golden corpus, 10 hand-labeled fixtures) | ≥ 95% | ≥ **95%** |
| Community quality after Louvain wiring (modularity > 0) | > 0.0 | Verified per-test |

Benchmarks: `crates/forge-crg/benches/crg_bench.rs` — run with `cargo bench -p yantra-crg`.
Accuracy tests: `crates/forge-crg/tests/crg_tests.rs` — golden-corpus recall test + Louvain community-quality assertion.
