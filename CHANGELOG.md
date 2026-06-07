# Changelog

All notable changes to Yantra are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)

---

## [Unreleased] — Real CRG token-reduction metric

### Added
- **`forge-tokenizer` upgraded to cl100k_base BPE** via `tiktoken-rs v0.5`.
  `count_tokens(&str) -> usize` now returns real GPT-4/Claude-class token counts
  instead of the previous `len/4` heuristic. Public signature unchanged — all
  callers are transparently upgraded.
- **`forge-crg::token_reduction` module** — reusable measurement pipeline:
  `compute_baseline_tokens`, `measure_token_reduction`, `render_markdown_report`,
  `TaskSpec`, `TaskMeasurement`, `ReductionReport`. Exported from `yantra-crg` root.
- **Integration test gate** `crates/forge-crg/tests/token_reduction.rs` — asserts
  ≥ 90% mean token reduction and ≥ 80% recall on the live Yantra codebase, writes
  a Markdown report to `crates/forge-crg/target/crg_token_reduction_report.md`.

### Measured
- **98.8% token reduction** on Yantra's own 178-file codebase
  (335,340 tokens → ~3,906 tokens per subgraph, cl100k_base BPE, 4K budget, 8 tasks)
- **100% recall** — all expected task-relevant symbols found in every subgraph
- Reproduce: `cargo test -p yantra-crg --test token_reduction -- --nocapture`

---
## [0.3.0] — 2026-05-31 — Windows manifest, comprehensive docs, SmartScreen guidance

### Added
- **Windows application manifest** embedded in `yantra.exe`: declares the binary
  as `SankalpSystems.Yantra`, marks it as standard-user (no elevation), enables
  long-path awareness and Windows 10/11 compatibility mode. Explorer → Properties
  → Details now shows the product name, description, and copyright. Helps build
  Windows Defender reputation over time.
- **`crates/forge-cli/build.rs`**: cross-platform build script that embeds the
  manifest and version-info resources on Windows via `winresource`.
- **Complete README rewrite**: every command documented with examples, SmartScreen
  bypass instructions prominent at the top, configuration guide, architecture
  overview, crate map, FAQ.

### Changed
- Workspace version bumped to `0.3.0`.
- `yantra version` now reports `yantra 0.3.0`.

## [0.2.0] — 2026-05-31 — Yantra Console + Canvas Fix

### Added
- **`yantra start` → Yantra Console TUI**: replaced the cosmetic guided pipeline
  and stub REPL with a unified full-screen ratatui application:
  conversation pane (streaming `ask` + inline commands), persistent CRG graph
  side panel (auto-refreshes every 5 s + after `index`/`run`), live telemetry
  footer (cost gauge + spans/min + error rate from `traces.sqlite`).
- **Canvas preview fix**: `yantra canvas <url>` now renders the cloned site in
  the preview iframe. Root cause was a missing `index.html` — `emit_project`
  wrote only `.tsx` files. New `preview.rs` serializer generates a self-contained
  HTML document with `data-yantra-id` markers and inlined CSS; `download_assets`
  is now wired to fetch images/fonts locally. Click-to-inspect works end-to-end.
- **`forge-cli::commands::ask`**: reusable streaming ask pipeline extracted from
  the inline `main.rs` code. Both `yantra ask` and the Console share one
  implementation via an `AskEvent` channel (CLAUDE.md §3.2).
- **`forge-cli::commands::metrics`**: shared CRG + telemetry compute helpers
  extracted from `graph.rs` and `observe.rs` to avoid duplication.
- **`forge-canvas::preview::render_preview_html`**: DOM → self-contained HTML
  renderer with asset rewriting and `<base href>`.
- **`forge-canvas::assets::fetch_and_download_assets`**: convenience wrapper
  for `download_assets` that builds its own `reqwest::Client`.
- **ADR-005**: documents the suspend/resume pattern for terminal-owning
  subcommands inside the Yantra Console.

### Changed
- `yantra start` is now the Yantra Console; old `run_guided_pipeline` and
  `run_interactive_shell` implementations removed.
- `yantra version` now reports `0.2.0`.
- Workspace `Cargo.toml` bumped to `0.2.0`.

## [Unreleased] — feat/canvas-graph-observe

### Added (this iteration — benchmarks, accuracy tests, subsystem fixes, unified CLI)

- **`yantra start`**: unified product entry point — interactive REPL shell (no task) or guided 6-step pipeline (with task). Ordered boot sequence: `.yantra/` init → doctor → CRG check → ready. Replaces the need to know 11 separate subcommands; all prior subcommands remain for power users.
- **`yantra night`**: Night Mode is now fully wired. Added `Commands::Night` to the CLI with `--tasks` and `--dry-run` flags. `ProductionTaskExecutor` dispatches real agents via the multi-agent `Scheduler` (Researcher → Coder → Verifier → RedTeam → Committer). `DryRunExecutor` available for smoke testing without an LLM.
- **`forge-cli::commands::agent_runtime`**: shared `build_scheduler` constructor extracted from `run.rs`; used by both `run` and `night` to avoid duplication (CLAUDE.md §3.2).
- **`forge-crg` Louvain communities**: `detect_communities` is now wired into `export.rs::build_payload` and used for all graph community labels. Replaces the first-directory-segment fallback. All vis.js graph views now show algorithm-derived communities.
- **`forge-canvas` graph explain LLM**: `POST /api/graph/explain` now calls the Tier 0 router (Ollama) for a real LLM-generated explanation. Falls back to the structural summary when no provider is configured. `AppState::with_router()` threads the model router in from the CLI.
- **`forge-orchestrator` dispatch**: `Orchestrator::drain_into_scheduler` method added — transfers all validated, queued tasks into a `Scheduler` for actual execution. The STVP token gate is preserved.
- **`forge-verifier` hallucination Layer 2**: `check_hallucination` is now `async` and implements real LSP diagnostic cross-check. When `VerificationContext.lsp_bridge` is `Some`, changed-file diagnostics are fetched and used to confirm/refute L1 CRG flags. False-positive rate reduced.
- **`forge-memory` heartbeat tasks 2–4**: all four heartbeat tasks are now real:
  - Task 2: CRG staleness check (warns if `crg.sqlite` > 1 hour old).
  - Task 3: Ollama liveness probe (TCP connect to 127.0.0.1:11434 with 1 s timeout).
  - Task 4: KV-cache warm (queries recent session summaries to prime the context).
  - `start_heartbeat` signature extended with `yantra_dir: PathBuf`.
  - `RecallStore::get_recent_session_ids(n)` method added.
- **Benchmark suite** (Wave 2, results pending): `cargo bench` + `cargo sla` CI aliases added. Bench + p99 SLA tests added for: `forge-stvp`, `forge-verifier`, `forge-orchestrator`, `forge-router`, `forge-memory`, `forge-lsp`, `forge-night`.
- **Accuracy test suite** (Wave 2): Golden-corpus accuracy tests added for all 9 pillar crates: `forge-ast` (100% symbol extraction, 10 fixtures), `forge-crg` (≥95% subgraph recall), `forge-stvp` (100% task-class classification, 20 labels), `forge-verifier` (100% Gate 1, 100% hallucination precision, 0 false positives), `forge-router` (100% tier selection, 10 labels), `forge-memory` (100% recall ordering + summary fidelity), `forge-orchestrator` (100% DAG ordering + circuit-breaker threshold), `forge-night` (100% decision-rule correctness), `forge-lsp` (100% language detection across 15 extension fixtures + full serde round-trip fidelity).
- **`.cargo/config.toml`**: `bench` and `sla` aliases added alongside existing `lint`, `lint-fix`, `test-all`, `doc-check`.

### Changed

- `forge-canvas`: visual browser canvas editor and CRG graph viewer, served via one Axum process per session (`yantra canvas`, `yantra graph`). Pipeline: URL clone → `scraper::Html` → `DomTree` → CSS-to-Tailwind → TSX emit → WebSocket hot-reload.
- `forge-obs`: `yantra observe` — live ratatui TUI backed by `.yantra/traces.sqlite`, showing spans, cost gauge, and anomaly detection.
- `forge-cli`: `canvas`, `graph`, and `observe` subcommands added to the `yantra` binary.
- `forge-crg`: vis.js JSON export (`export_graph`, `export_focused`) served at `/api/graph/json` by the canvas server.
- Integration test suites: adversarial canvas/STVP/graph/trace suite (`tests/vuln_canvas_clone.rs`, `tests/vuln_cli_args.rs`, `tests/vuln_graph_export_cycles.rs`, `tests/vuln_stvp_injection.rs`, `tests/vuln_trace_store_tamper.rs`) and canvas accuracy suite (`crates/forge-canvas/tests/`).
- `nextest.toml`: nextest adopted as the primary test runner for the workspace.
- `crates/forge-ast/benches/ast_bench.rs`: AST re-parse benchmark; measured p99 = 7.2ms (target: <150ms).

### Changed

- `forge-serve` repurposed and renamed to `forge-canvas`; all existing server functionality migrated.
- Axum server upgraded from Server-Sent Events (SSE) to WebSocket (WS) for canvas hot-reload.
- `ARCHITECTURE.md` and `README.md` updated to reflect `forge-canvas` rename and WS protocol change.

---

## [0.1.0-alpha] — main branch baseline

### Added

- `forge-obs`: audit-telemetry binary for telemetry field coverage reporting.
- `forge-ast`: AST re-parse benchmark scaffold.
- `forge-orchestrator`, `forge-lsp`: logging cleanup — removed noise, upgraded misclassified debug spans.
- `forge-obs`, `forge-verifier`: clarified error messages with actionable hints.
- `forge-core`: sacred file built-ins, `default-sacred.txt`, and edge-case tests.
- `forge-cli`: ratatui TUI, diff preview, approval gates, skills registry.
- `forge-serve`: Axum + SSE + Live Canvas (vanilla JS, no build step). *(superseded by `forge-canvas` on this branch)*
- `forge-tools`: project-scoped filesystem MCP server with sacred file authorization and diff application support.
- `forge-night`: Dawn Digest, Postmortem lite, Semantic Merge lite, Sacred File Guard, Docker sandbox.
- `forge-night`: autonomous Night Run, checkpoint/resume, watchdog binary.
- `forge-night`: mode policies, Twilight phase, decision rules engine.
- Core agent framework: Researcher, Coder, Verifier, Router agents with CI coverage and test scaffolding.
- Initial workspace: agent-based orchestration, filesystem tools, CI/CD pipelines.

---

*Yantra is pre-1.0. Breaking changes may occur between alpha releases.*
