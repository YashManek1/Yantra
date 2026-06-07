`forge-crg` is the Code-Review Graph: Yantra's structural code-intelligence layer. It drives Tree-sitter (via `forge-ast`) over every source file to extract typed symbols and infers four edge kinds (`CALLS`, `IMPORTS`, `IMPLEMENTS`, `TESTS`) which are persisted in `.yantra/crg.sqlite`. Subgraph extraction runs a weighted BFS from seed symbols, bounded by a caller-supplied token budget, and returns a `RenderedSubgraph` that agents receive instead of naive whole-repo context — compressing a whole-repo context by **98.8%** (~335K tokens → ~3.9K tokens per subgraph, measured). Community detection uses a Louvain algorithm (`louvain.rs`) so `community_label` reflects actual structural coupling rather than directory structure. The crate also exports the full graph (or a focused subgraph) as a vis.js-compatible `GraphJson` via `export_graph` and `export_focused`, served by `forge-canvas` at `/api/graph/json` for interactive browser visualization.

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| **Token reduction vs whole-repo naive baseline** | ≥ 90% | **98.8%** (measured) |
| **Mean recall of task-relevant symbols** | ≥ 80% | **100%** (measured) |
| Subgraph extraction recall (golden corpus, 10 hand-labeled fixtures) | ≥ 95% | ≥ **95%** |
| Community quality after Louvain wiring (modularity > 0) | > 0.0 | Verified per-test |

**Token reduction detail (Yantra's own 178-file codebase, 8 tasks, 4K budget):**

| Approach | Tokens |
|---|---|
| Naive whole-repo (cl100k_base BPE) | 335,340 |
| CRG subgraph per task | ~3,906 |
| Reduction | **98.8%** |

Benchmarks: `crates/forge-crg/benches/crg_bench.rs` — run with `cargo bench -p yantra-crg`.
Accuracy tests: `crates/forge-crg/tests/crg_tests.rs` — golden-corpus recall test + Louvain community-quality assertion.
Token reduction gate: `crates/forge-crg/tests/token_reduction.rs` — run with `cargo test -p yantra-crg --test token_reduction -- --nocapture`.