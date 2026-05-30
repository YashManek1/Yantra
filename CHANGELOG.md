# Changelog

All notable changes to Yantra are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)

---

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
