# Yantra (यन्त्र)

> **Engine. Instrument. Runtime.**  
> A Rust-native agentic coding runtime by **Sankalp Systems**.

[![Version](https://img.shields.io/badge/version-0.3.0-blue)](https://github.com/YashManek1/Yantra/releases)
[![License](https://img.shields.io/badge/license-Apache--2.0-green)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange)](https://www.rust-lang.org/)

---

## What is Yantra?

Yantra is **not a wrapper around an LLM**. It is a fully autonomous coding *runtime* — a substrate that turns commodity language models (including free, local ones like Ollama) into trustworthy, observable, autonomous engineers.

The core thesis:
> Models commoditize. Context retrieval, memory, planning, and verification do not. **The runtime is the moat.**

Yantra runs tasks end-to-end: verifying your intent → multi-agent code generation → five independent verification gates → signed git commit — all with full observability and zero hallucinated symbols.

---

## ⚠️ Windows SmartScreen Warning

When you first run `yantra.exe`, Windows Defender SmartScreen will show:

> **"Windows protected your PC"**  
> *Microsoft Defender SmartScreen prevented an unrecognized app from starting.*

**This is expected and safe.** SmartScreen blocks any unsigned executable it hasn't seen before — it is not a virus warning, just a "we haven't seen this binary before" notice common for all open-source software distributed without an expensive EV code-signing certificate.

### How to bypass it (one-time)

1. Click **"More info"** (the blue link in the dialog)
2. Click **"Run anyway"**

You will only need to do this once. After that, Windows remembers and won't ask again.

> **For the technically inclined:** this warning appears because `yantra.exe` is not code-signed with a Microsoft-trusted EV certificate (costs $200–$500/year). The source code is fully open at [github.com/YashManek1/Yantra](https://github.com/YashManek1/Yantra) — you can verify every line and build from source yourself (`cargo build --release`).

---

## Installation

### Option A — Download the binary (Windows x64)

1. Go to [Releases](https://github.com/YashManek1/Yantra/releases/latest)
2. Download `yantra.exe`
3. Place it somewhere on your `PATH` (e.g. `C:\Tools\` and add that to PATH)
4. Open a terminal and run: `yantra --help`

### Option B — Build from source (all platforms)

**Prerequisites:**
- [Rust stable](https://rustup.rs/) (≥ 1.86, pinned in `rust-toolchain.toml`)
- [Ollama](https://ollama.com/) (for local LLM, free)
- `cargo-nextest` for tests: `cargo install cargo-nextest`

```sh
git clone https://github.com/YashManek1/Yantra.git
cd Yantra
cargo build --release
# Binary at: target/release/yantra (or yantra.exe on Windows)
```

### Ollama setup (required for free local LLM)

```sh
# Install Ollama from https://ollama.com/
# Pull the default model Yantra uses:
ollama pull qwen2.5-coder:7b
```

---

## Quick Start (5 minutes)

```sh
# 1. Index your codebase (builds the Code-Review Graph)
yantra index .

# 2. Launch the Yantra Console — the unified product surface
yantra start

# 3. In the Console, type any question:
#    > what does the authentication flow do?
#    > run: add unit tests for the parser
```

---

## The Yantra Console — `yantra start`

The recommended entry point. Opens a unified full-screen terminal app with three persistent panels:

```
┌─────────────────────────────────────┬──────────────────────┐
│  Conversation                       │  Graph               │
│  ◉ Yantra Console                  │  S:2218 E:17752      │
│                                     │  Communities         │
│  > what does run_ask do?           │   312  forge-cli     │
│                                     │   198  forge-crg     │
│  ── CRG context extracted ──       │  Hubs                │
│  The run_ask function in           │    42  AppState      │
│  commands/ask.rs implements...     │    38  GraphCache    │
│                                     │    31  DomTree       │
│  Cost: $0.000012                   │                      │
│                                     │                      │
├─────────────────────────────────────┴──────────────────────┤
│  › _                                                        │
├─────────────────────────────────────────────────────────────┤
│  $0.0001  │  3/min  │  err 0.0%  │  Ok  │  PageUp/Dn scroll│
└─────────────────────────────────────────────────────────────┘
```

**Left panel — Conversation:** Type anything. Bare prose asks the codebase (CRG-grounded streaming answer). Explicit commands drive the full pipeline.

**Right panel — Graph:** CRG stats + community breakdown + top hub symbols. Auto-refreshes every 5 seconds and immediately after `index`/`run`. Shows the living structure of your codebase.

**Bottom bar — Telemetry:** Live cost gauge, spans/min, error rate — pulled from `.yantra/traces.sqlite` every second.

### Console commands

| Command | What it does |
|---|---|
| `<any question>` | Ask the codebase — CRG-grounded, Tier-0 LLM, streaming answer |
| `ask <question>` | Same as above, explicit form |
| `run <task>` | Full STVP + multi-agent DAG pipeline for a task |
| `index [path]` | Build or rebuild the CRG symbol index |
| `night [t1,t2,...]` | Start autonomous Night Mode (Dawn Digest on completion) |
| `canvas [url]` | Open the visual canvas editor (optionally clones a URL) |
| `observe` | Open the live telemetry TUI |
| `doctor` | Run preflight health checks |
| `help` / `?` | Show all commands |
| `quit` / `exit` / `q` | Exit |

**Keyboard shortcuts:**
- `PageUp` / `PageDown` — scroll conversation history
- `←` / `→` — move cursor in the input box
- `Esc` — cancel a running ask / exit

---

## All Commands

### `yantra index [path]`

Builds the Code-Review Graph (CRG) for the given path (defaults to `.`).

- Parses every source file with Tree-sitter (Rust, Python, TypeScript, JavaScript)
- Extracts all symbols (functions, structs, traits, classes, methods)
- Builds a typed structural graph with call/import/test edges
- Generates semantic embeddings for each symbol
- Persists everything to `.yantra/crg.sqlite`

```sh
yantra index .                  # index the current directory
yantra index ./src              # index a subdirectory
```

**Run this first.** All other commands that use CRG context require an index.

---

### `yantra ask "<question>"`

Ask a natural-language question about your codebase. Yantra:
1. Extracts a token-budgeted CRG subgraph relevant to the question
2. Sends it to a Tier-0 model (local Ollama by default — free)
3. Streams the answer token-by-token
4. Runs a grounding verifier to flag any hallucinated symbol names

```sh
yantra ask "What does the VerifierAgent do?"
yantra ask "Which functions call extract_subgraph?"
yantra ask "How does Night Mode decide when to stop?"
```

> Note: Answers are grounded against the CRG — if the model invents a function name that doesn't exist in the index, Yantra flags it as `⚠ Unverified symbol`.

---

### `yantra run "<task>"`

Run the full STVP + multi-agent pipeline for a task. This is Yantra's core autonomous coding workflow:

1. **STVP Interrogation** — Yantra asks 3–5 clarifying questions to pin down your intent
2. **Truth Token** — ed25519-signed token issued once intent is verified
3. **CRG Subgraph** — the relevant slice of your codebase is extracted
4. **Multi-Agent DAG** — Researcher → Coder → Tester → Verifier → RedTeam → Committer agents run in sequence
5. **Verification Gates** — diff is checked against truth drift, static analysis, and test passage
6. **Commit** — a signed git commit is created with the changes

```sh
yantra run "add unit tests for the parser module"
yantra run "refactor the auth middleware to use JWT"
yantra run "fix the off-by-one error in the pagination logic"
```

> **Tip:** Be specific. "add tests" is less effective than "add property tests for GraphCache::build covering empty graphs and cycles."

---

### `yantra canvas [url]`

Opens the browser-based visual canvas editor.

**With a URL:** Clones the target website, parses its DOM, downloads assets locally (images, fonts), generates a self-contained preview, and opens the editor in your browser.

```sh
yantra canvas https://example.com
yantra canvas https://tailwindui.com
```

**Without a URL:** Starts the canvas server on its landing page (blank canvas mode).

```sh
yantra canvas
```

**In the browser editor:**
- Click any element in the preview to inspect it
- Change colors, padding, font size in the right panel
- Click "Apply Changes" to update the underlying React component
- Changes hot-reload in the preview via WebSocket

**Also served at:**
- `http://127.0.0.1:8088/graph` — vis.js CRG graph viewer (click nodes to explain them)

---

### `yantra graph [--focus <symbol>]`

Opens the CRG graph dashboard TUI (terminal). Shows:
- Symbol count, edge count, community count, file count
- Community breakdown (which files/modules have the most symbols)
- Top hub symbols by connectivity score (most important entry-points)

**Keys:**
- `↑` / `↓` — navigate the hub list
- `g` — open the browser vis.js graph viewer focused on the selected hub
- `q` — quit

```sh
yantra graph                          # full graph overview
yantra graph --focus AppState         # open browser graph focused on AppState
```

---

### `yantra observe`

Live observability TUI. Opens a real-time dashboard over `.yantra/traces.sqlite` showing:
- Cumulative cost gauge (green / yellow / red by budget thresholds)
- Spans-per-minute throughput
- Error rate (last 5 minutes)
- Top 5 most expensive calls
- Scrollable recent span list

```sh
yantra observe
```

**Keys:** `↑`/`↓` scroll · `q` quit

---

### `yantra night [--tasks "task1,task2"] [--dry-run]`

Autonomous Night Mode. Front-loads all decisions (Twilight phase), runs the full pipeline overnight, and generates a **Dawn Digest** markdown report summarising everything that happened.

```sh
yantra night --tasks "add retry logic,fix auth bug,add integration tests"
yantra night --dry-run    # smoke test — no LLM calls
```

**Three phases:**
1. **Twilight** — Yantra asks all clarifying questions upfront (before you sleep)
2. **Night Run** — runs all tasks autonomously in sequence
3. **Dawn Digest** — writes `dawn_digest.md` with a summary of changes, costs, and any failures

---

### `yantra doctor`

Runs preflight health checks and reports on the runtime environment:

```sh
yantra doctor
yantra doctor --json    # machine-readable output
```

Checks: Ollama reachability · `.yantra/` directory structure · CRG index freshness · Rust toolchain · disk space

---

### `yantra context "<task>"`

The Context Lens. Shows how many tokens a given task would consume from the CRG subgraph, without actually running anything. Useful for budget planning.

```sh
yantra context "refactor the auth module"
yantra context --json "add JWT rotation"
```

---

### `yantra status`

Prints the current session's cost snapshot: total spans, cumulative cost, and status (Ok / Warn / Pause / Kill).

```sh
yantra status
yantra status --json
```

---

## Configuration

### `configs/routing.toml`

Controls which LLM providers are used and the cost budget. This file is read at startup.

```toml
[providers]
tier0 = [{ kind = "ollama", default_model = "qwen2.5-coder:7b", base_url = "http://localhost:11434" }]
tier1 = [{ kind = "openrouter", default_model = "mistralai/mistral-7b-instruct:free" }]
tier2 = []
tier3 = []

[budget]
soft_usd  = 1.0    # yellow warning
hard_usd  = 5.0    # pause new tasks
kill_usd  = 20.0   # hard stop
```

**Tier 0** — local Ollama (free, no internet required)  
**Tier 1** — free cloud APIs (OpenRouter free tier, GitHub Models)  
**Tier 2** — low-cost cloud (OpenRouter paid)  
**Tier 3** — frontier models (GPT-4, Claude Opus) — only for STVP debate synthesis

Yantra always tries Tier 0 first. It only escalates if the lower tier fails or is unavailable.

### Environment variables

| Variable | Purpose |
|---|---|
| `OPENROUTER_API_KEY` | API key for OpenRouter (Tier 1/2) |
| `GITHUB_TOKEN` | Token for GitHub Models (Tier 1) |
| `RUST_LOG` | Log level: `info`, `debug`, `warn` |

---

## System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        User Surfaces                            │
│  yantra start (Console TUI)  ·  canvas (Browser)  ·  IDE (WIP) │
└────────────────────────┬────────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────────┐
│                   ★ STVP (Truth Gate)                           │
│   Interrogator  →  3 Validators  →  ed25519 Truth Token        │
│   No task runs without a valid signed token.                   │
└────────────────────────┬────────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────────┐
│              Orchestrator (DAG Scheduler)                       │
│   Researcher → Coder → Tester → Verifier → RedTeam → Committer │
└────────┬──────────┬───────────────┬────────────────────────────┘
         │          │               │
    ★ CRG       Memory          Router
  subgraph    4-tier store    Tier 0→1→2→3
  extraction  + Truth Vault   (Ollama first)
         │                         │
    ★ Verification            Observability
  Truth Drift · Lint · Tests   OTel spans
  All 3 gates must pass        Cost gauge
```

---

## The Three Pillars

### ★ STVP — Source-Truth Validation Protocol

Before any code is written, Yantra interrogates the user with 3–5 targeted questions to produce a `SOURCE_TRUTH.yaml`. This file is then ed25519-signed into a `TruthToken`. The orchestrator refuses to schedule any task without a valid token.

This prevents:
- Scope creep ("while you're here, also refactor X")
- Ambiguous tasks producing wrong code
- Tasks that contradict existing constraints

### ★ CRG — Code-Review Graph

Rather than dumping entire files into context (wasteful, expensive, inaccurate), Yantra builds a typed structural graph of your codebase:
- **Nodes:** functions, structs, traits, classes, modules
- **Edges:** calls, imports, implements, tests-for
- **Subgraph extraction:** given a task, Yantra extracts the minimum token-budgeted slice needed

Measured on Yantra's own 178-file codebase, the CRG compresses **335,340 tokens**
(whole-repo naive baseline) to **~3,906 tokens** per subgraph — a **98.8% token reduction**
with **100% recall** of task-relevant symbols, across 8 representative tasks.

| Approach | Tokens | Notes |
|---|---|---|
| Naive whole-repo (Claude Code / Cursor baseline) | ~335,340 | Every source file in context |
| **Yantra CRG subgraph** | **~3,906** | Task-relevant slice, 4K budget |
| **Reduction** | **98.8%** | Measured, real BPE, 100% recall |

> Reproduce: `cargo test -p yantra-crg --test token_reduction -- --nocapture`

### ★ Night Mode

Designed for running overnight without supervision:
1. **Twilight:** All decisions front-loaded. Yantra asks everything it might need before you sleep.
2. **Night Run:** Tasks execute autonomously. Each task uses STVP + CRG + full verification.
3. **Dawn Digest:** A markdown report summarising every change, cost, time, and any failures.

---

## The Crate Map

| Crate | Purpose |
|---|---|
| `forge-core` | Shared types: `TaskId`, `TruthToken`, `AgentKind`, `SessionId` |
| `forge-tokenizer` | BPE tokenizer for token counting and budget enforcement |
| `forge-obs` | OpenTelemetry, SQLite trace store, cost gauge, watchdog |
| `forge-router` | `ModelProvider` trait + routing policy + prompt translation |
| `forge-ast` | Tree-sitter parser, symbol extraction, DB persistence |
| `forge-crg` ★ | Code-Review Graph: build, query, subgraph extraction, vis.js export |
| `forge-lsp` | tower-lsp client, LSP-MCP bridge |
| `forge-memory` | Working / Recall / Archival / TKG + Truth Vault + Mistake Library |
| `forge-tools` | All MCP server implementations (fs, git, CRG, LSP, browser) |
| `forge-stvp` ★ | Source-Truth Validation Protocol |
| `forge-verifier` | Boolean exit gate, static analysis, Truth Drift, hallucination check |
| `forge-agents` | Specialist agents: Coder, Tester, Researcher, Refactorer, RedTeam |
| `forge-orchestrator` | DAG scheduler, Debate Engine, CSP Planner |
| `forge-night` ★ | Night Mode: Twilight, Night Run, Dawn Digest |
| `forge-skills` | SKILL.md registry, skill-learning loop |
| `forge-sidecar` | Always-on background daemon (fs-watch, drift detection) |
| `forge-canvas` | Axum + WebSocket visual canvas editor + CRG graph viewer |
| `forge-cli` | clap + ratatui CLI entry point (the `yantra` binary) |
| `forge-eval` | SWE-bench Verified subset runner |

---

## Development

### Running checks

```sh
cargo fmt                  # format all code
cargo lint                 # clippy (warnings-as-errors)
cargo lint-fix             # auto-fix clippy lints
cargo test-all             # full test suite via nextest (576 tests)
cargo doc-check            # docs compile cleanly
cargo build --release      # release binary → target/release/yantra.exe
```

### Required tools

```sh
cargo install cargo-nextest    # faster test runner
rustup component add rustfmt   # formatter
rustup component add clippy    # linter
```

### Project layout

```
yantra/
├── Cargo.toml              ← workspace manifest (version bumped here)
├── CLAUDE.md               ← coding rules for AI assistants
├── CHANGELOG.md
├── crates/                 ← all library + binary crates
├── configs/                ← routing.toml, sacred.txt
├── docs/decisions/         ← Architecture Decision Records
├── tests/                  ← cross-crate integration + security tests
└── .yantra/                ← runtime data (gitignored): crg.sqlite, traces.sqlite
```

---

## Frequently Asked Questions

**Q: Does Yantra need internet?**  
A: No. With Ollama running locally, Yantra is fully offline-capable. Cloud providers (OpenRouter, GitHub Models) are additive.

**Q: Which languages does CRG support?**  
A: Rust, Python, TypeScript, JavaScript. More via Tree-sitter grammars.

**Q: Why is the binary 44 MB?**  
A: It includes the Ollama client, tree-sitter parsers for 4 languages, a BPE tokenizer, fastembed ONNX model for semantic search, an Axum HTTP server, and the full ratatui TUI stack — all statically linked.

**Q: Can I use GPT-4 / Claude?**  
A: Yes. Add your API key to `configs/routing.toml` as a Tier 3 provider. Yantra still uses local Ollama for most calls and only escalates to frontier models for STVP debate synthesis.

**Q: What is a "sacred file"?**  
A: Files matching patterns in `.yantra/sacred.txt` (e.g. `src/auth/**`, `Cargo.toml`) require explicit STVP authorization before Yantra will modify them. No exceptions.

**Q: SmartScreen keeps blocking the exe — is there a permanent fix?**  
A: Click "More info" → "Run anyway" once. After that Windows won't ask again for that binary. The long-term fix (code signing cert) is on the roadmap. You can always build from source to avoid SmartScreen entirely.

---

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for full release history.

---

## License

Apache-2.0 © 2026 Sankalp Systems

---

*The runtime is the moat. Build it like one.*
