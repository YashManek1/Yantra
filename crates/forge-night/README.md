Implements Night Mode across three phases: **Twilight** (front-load STVP for all planned tasks, collect decision rules), **Night Run** (execute the approved DAG without approval gates, checkpoint every 30 s), and **Dawn Digest** (generate `dawn_digest.md` and forward postmortem snippets to the Mistake Library).

## CLI

```sh
# Run Night Mode with one or more tasks
yantra night --tasks "fix auth bug,add retry logic"

# Dry-run (no LLM calls — smoke test only)
yantra night --dry-run

# Or use the unified entry point
yantra start  # interactive shell → night <tasks>
```

## Production executor

The `ProductionTaskExecutor` (in `forge-cli::commands::night`) dispatches each night task through the shared multi-agent `Scheduler` (Researcher → Coder → Verifier → RedTeam → Committer) via `forge-cli::commands::agent_runtime::build_scheduler`.

## Accuracy & Benchmarks

Decision-rule resolution benchmarked at p99 ≤ 10 ms (see `benches/night_bench.rs`). Decision-rule outcome correctness is 100% deterministic (see `tests/night_accuracy.rs`).
