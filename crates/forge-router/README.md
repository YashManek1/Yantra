Defines the provider abstraction and tiered routing policy for Yantra model calls. It keeps the runtime model-agnostic by routing prompts through the `ModelProvider` trait and selecting local, free, low-cost, or frontier providers according to policy.

## Tier Policy

| Tier | Providers | When selected |
|------|-----------|---------------|
| Tier 0 | Ollama (local) | Default for all agent work; free, offline-capable |
| Tier 1 | GitHub Models (free API) | Tier 0 unavailable; non-critical tasks |
| Tier 2 | OpenRouter (low-cost) | Budget within soft limit; higher capability needed |
| Tier 3 | Frontier (Anthropic, GPT-4o) | Architecture, debate synthesis, sacred-file decisions only |

## Benchmarks & Accuracy

| Metric | Target | Achieved |
|--------|--------|----------|
| Routing policy selection p99 | ≤ 1 ms | < 0.1 ms (in-memory policy lookup) |
| Tier-selection accuracy (10 labeled fixtures) | ≥ 95% | **100%** (10/10) |

Benchmarks: `crates/forge-router/benches/router_bench.rs` — run with `cargo bench -p yantra-router`.
Accuracy tests: `crates/forge-router/tests/router_accuracy.rs` — 10 tier-selection golden labels covering all four tiers.
SLA test: `router_policy_p99_meets_1ms_sla`.
