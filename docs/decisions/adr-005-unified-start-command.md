# ADR-005: Unified `yantra start` Entry Point

**Date:** 2026-05-29
**Status:** Accepted
**Deciders:** Yash Manek, Claude Code (Sonnet 4.6)

## Context

Yantra previously exposed 11 separate subcommands (`index`, `ask`, `run`, `canvas`,
`graph`, `observe`, `status`, `context`, `doctor`, `version`, and no `night`).
New users needed to understand the correct invocation order: index → ask/run → canvas →
observe. The sidecar daemon's `main()` was an empty stub (`fn main() {}`), so no
coordinated boot sequence existed.

The user requirement: **one start command that makes a complete product** — no need
to memorize multiple subcommands. Two modes were explicitly requested: an interactive
shell and a guided pipeline.

## Decision

Add `yantra start [task]` as the **recommended primary entry point**:

- **`yantra start`** (no task) → interactive TUI shell. Boots the runtime in order
  (`.yantra/` init → doctor preflight → CRG index check → ready), then drops into
  an `inquire`-backed REPL where `index`, `ask`, `run`, `night`, `canvas`, `graph`,
  `observe`, `context`, `doctor` are available as in-process commands.

- **`yantra start "<task>"`** (with task) → guided 6-step pipeline: doctor → index
  (if missing) → STVP via `run_command` → verify (gates enforced inside `run`) →
  summary.

All 11 prior subcommands are preserved as power-user shortcuts (no breaking changes).
The `start` command lives in `crates/forge-cli/src/commands/start.rs` and reuses the
existing `commands::doctor`, `commands::run`, `commands::night`, etc. without
duplicating any orchestration logic.

## Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **Interactive TUI shell + guided pipeline (chosen)** | Single `start` serves both exploration and CI-like pipelines. Matches user's explicit request for both modes. | More complex to implement than a linear pipeline alone. |
| Guided pipeline only | Simpler code; clear step ordering. | Doesn't serve interactive exploration use case. |
| Sidecar supervisor | Long-running services managed centrally. | Requires filling in the empty sidecar stub first; higher complexity. |

## Consequences

**Positive:**
- Single `yantra start` replaces the need to know subcommand order.
- Both modes reuse existing command implementations — no logic duplication.
- Interactive shell allows in-process command history and contextual error recovery.
- Guided pipeline gives beginners a clear, numbered progress display.

**Negative:**
- The interactive shell is backed by `inquire::Text`, not a full readline/libedit
  implementation — no arrow-key history (acceptable for now; upgradeable later).
- `yantra start "<task>"` skips the interactive STVP questionnaire flow (delegates
  to `run_command` which includes it internally).

## Related

- `crates/forge-cli/src/commands/start.rs` — implementation
- `crates/forge-cli/src/main.rs` — `Commands::Start` enum variant
- ADR-003 (grounding as gate zero) — informed the 6-step pipeline ordering
