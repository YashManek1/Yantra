# ADR-005: Yantra Console — Suspend/Resume for Terminal-Owning Subcommands

**Status:** Accepted  
**Date:** 2026-05-31

## Context

`yantra start` was rebuilt as the "Yantra Console" — a unified full-screen
ratatui alternate-screen TUI with a conversation pane, a live graph side panel,
and a telemetry footer. The goal is a single integrated product surface similar
to Claude Code.

Several existing subcommands (`run`, `night`, `canvas`, `observe`, `doctor`)
have deep coupling to the terminal:

- `run_command` builds its own `RunTui` (raw mode + alternate screen), uses
  `inquire` for STVP approval/diff prompts, calls `ApprovalGate::render_and_wait`
  and `DiffPreview::preview_and_approve`, and writes ANSI-coloured diffs directly
  to stdout.
- `canvas_command` opens a browser and blocks on `Ctrl-C`.
- `observe_command` and `night_command` each maintain their own ratatui loops.
- `doctor` uses `println!` / `eprintln!`.

Routing these through a "pane channel" would require rewriting `approval.rs`,
`diff_preview.rs`, `tui.rs`, the STVP `CliQuestionnaireUi`, and all the
interactive prompt logic in `run.rs` — a large, high-risk scope expansion that
would delay shipping.

## Decision

The Console uses **suspend/resume** for commands that own the terminal:

1. `terminal_guard.suspend()` — calls `disable_raw_mode()` and `LeaveAlternateScreen`,
   returns the terminal to cooked mode.
2. Prints a brief separator banner on the normal terminal.
3. Awaits the subcommand directly — it behaves exactly as `yantra <cmd>` today.
4. `terminal_guard.resume()` — calls `enable_raw_mode()`, `EnterAlternateScreen`,
   `terminal.clear()`, and triggers a full redraw.
5. Sends a graph-refresh nudge so the side panel reflects any CRG changes.

`ask` (streams into the pane via `AskEvent` channel) and `index`
(`block_in_place` + progress lines) stay in-pane and do **not** suspend.

## Consequences

**Positive:**
- Preserves all approval and diff-preview UX without any changes to the existing
  subcommand implementations.
- Reduces the blast radius of this change to `console/mod.rs` only.
- Suspension/resume is a well-understood pattern used by editors like `vim` and
  `helix` when spawning external processes.

**Negative:**
- A brief visual flash (screen clear → normal terminal → screen clear → TUI) is
  visible when a suspending command is invoked. This is accepted.
- Long-running suspended commands (`canvas`, blocking on Ctrl-C) hold the
  Console in `Suspended` mode throughout; the graph/telemetry background tasks
  continue buffering snapshots in the unbounded channels and drain on resume.

## Alternatives Considered

**Stream everything through channels:** Would require rewriting `run.rs`,
`approval.rs`, `diff_preview.rs`, `tui.rs`, and the STVP questionnaire UI to
push output through an `mpsc::Sender<String>` instead of writing to stdout/
alternate-screen. Significantly higher risk, deferred to a future milestone.

**Treat run/night as CLI-only:** Would mean `yantra start` couldn't trigger the
pipeline at all, losing the "all-in-one" goal.
