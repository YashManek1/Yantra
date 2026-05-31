Implements Yantra's durable memory service, including Working, Recall, Archival, and Temporal Knowledge Graph tiers. It also stores truth artifacts and failure records that inform future planning and agent behavior.

## Memory Tiers

| Tier | Storage | TTL | Purpose |
|------|---------|-----|---------|
| Working | In-process `HashMap` | Session | Active context for the current task |
| Recall | SQLite (`memory.sqlite`) | 30 days | Conversation turns, session summaries |
| Archival | SQLite (compressed) | Indefinite | Long-term patterns, resolved mistakes |
| TKG | SQLite graph tables | Indefinite | Temporal knowledge graph for causal chains |

## Heartbeat Tasks

The background heartbeat fires every 60 s and runs four tasks:
1. **Session summarization** — condenses the oldest unsummarised session turns.
2. **CRG staleness check** — warns if `crg.sqlite` has not been rebuilt in > 1 hour.
3. **Ollama liveness probe** — TCP connect to `127.0.0.1:11434` with 1 s timeout.
4. **KV-cache warm** — queries the three most recent session summaries to prime the context window.

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| `get_recent_session_ids(3)` p99 | ≤ 15 ms | < 5 ms (indexed SQLite query) |
| `summarize_session` + `get_session_summary` round-trip p99 | ≤ 15 ms | < 10 ms |
| Recall ordering correctness (3 scenarios) | 100% | **100%** |
| Summary round-trip fidelity | 100% | **100%** |

Benchmarks: `crates/forge-memory/benches/memory_bench.rs` — run with `cargo bench -p yantra-memory`.
Accuracy tests: `crates/forge-memory/tests/memory_accuracy.rs`.
SLA tests: `memory_recall_p99_meets_15ms_sla`, `memory_summary_roundtrip_p99_meets_15ms_sla`.
