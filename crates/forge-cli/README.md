`forge-cli` is the `yantra` binary entry point, built with `clap` for argument parsing and `ratatui` for interactive terminal UIs.

## Quick start

```sh
# Recommended: unified entry point (interactive shell)
yantra start

# Recommended: guided 6-step pipeline for a single task
yantra start "add retry logic to the HTTP client"
```

## All subcommands

| Command | Description |
|---|---|
| `start [task]` | **Recommended entry point.** Interactive shell (no task) or guided pipeline (with task). Boots all services in correct order. |
| `night [--tasks t1,t2] [--dry-run]` | Autonomous Night Mode — Twilight → Night Run → Dawn Digest. |
| `index [path]` | Build or refresh the CRG symbol index. |
| `ask <question>` | Query the codebase via CRG subgraph + routed LLM. |
| `run <task>` | Full STVP pipeline → multi-agent DAG (Researcher → Coder → Verifier → RedTeam → Committer). |
| `canvas [url]` | Open the browser-based visual editor; optionally clone a URL. |
| `graph [--focus id]` | Open the CRG graph viewer in-browser. |
| `observe` | Live OTel span TUI with cost gauge and anomaly detection. |
| `status [--json]` | Print current session cost and threshold state. |
| `context <task>` | Show the token ledger (Context Lens) for a task. |
| `doctor [--json]` | Preflight health checks (`.yantra/`, Ollama, CRG). |
| `version` | Print the current version. |

The CLI is intentionally thin: it loads `configs/routing.toml`, constructs the `Router`, and delegates all runtime logic to `forge-orchestrator`, `forge-night`, and `forge-canvas`. ratatui TUIs are used for diff previews, STVP approval gates, and the `observe` live dashboard.
