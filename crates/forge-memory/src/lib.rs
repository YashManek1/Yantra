//! # forge-memory: Four-Tier Memory Service
//!
//! Implements Working, Recall, Archival, and Temporal Knowledge Graph (TKG)
//! memory tiers backed by `memory.sqlite`, plus the Truth Vault (stores
//! signed `SOURCE_TRUTH.yaml` artifacts) and the Mistake Library (failure
//! records injected into future prompts).
//!
//! ## Input
//! - Memory write events from any agent or the orchestrator
//! - Recall queries with an optional recency or semantic filter
//! - Heartbeat intervals from `configs/memory.toml`
//!
//! ## Output
//! - Recalled memory snippets injected into agent context windows
//! - Truth artifacts retrieved by `forge-stvp` during token verification
//! - Mistake-prevention rules forwarded to the prompt assembler
//!
//! ## Related
//! - `forge-obs` — provides the `SQLite` helpers and span substrate
//! - `forge-stvp` — reads/writes Truth Vault entries
//! - `forge-agents` — every agent loop reads from and writes to memory
