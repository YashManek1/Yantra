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

/// Short-lived, high-priority scratchpad for the current agent turn.
/// Written at the start of a task; cleared on task completion.
pub struct WorkingMemory {
    /// Entries stored in this working memory tier.
    pub entries: Vec<String>,
}

/// Mid-term episodic store: completed task summaries and code snippets
/// persisted across sessions for semantic recall queries.
pub struct RecallMemory {
    /// SQLite path backing this recall tier.
    pub database_path: String,
}

/// Long-term compressed store: distilled knowledge that never expires.
/// Written only by the Memory Consolidation agent during Night Mode.
pub struct ArchivalMemory {
    /// SQLite path backing this archival tier.
    pub database_path: String,
}

/// Temporal Knowledge Graph: entities, relationships, and their validity
/// windows. Used for reasoning about evolving codebase state.
pub struct TemporalKnowledgeGraph {
    /// SQLite path backing the TKG.
    pub database_path: String,
}

/// Vault for signed `SOURCE_TRUTH.yaml` artifacts issued by STVP.
/// Read by `forge-stvp` when verifying truth tokens for a task.
pub struct TruthVault {
    /// SQLite path backing the vault.
    pub database_path: String,
}

/// Failure record store: past agent mistakes converted to prevention rules
/// and injected into prompt preambles of future agent runs.
pub struct MistakeLibrary {
    /// SQLite path backing the mistake library.
    pub database_path: String,
}

/// Unified handle to all memory tiers.
/// All agents receive one `MemoryHandle` in their `AgentContext`.
pub struct MemoryHandle {
    /// Short-lived working scratchpad for the current turn.
    pub working: WorkingMemory,
    /// Medium-term episodic recall store.
    pub recall: RecallMemory,
    /// Long-term archival knowledge store.
    pub archival: ArchivalMemory,
    /// Temporal knowledge graph for evolving codebase state.
    pub temporal_knowledge_graph: TemporalKnowledgeGraph,
    /// Vault holding signed STVP truth artifacts.
    pub truth_vault: TruthVault,
    /// Library of past mistakes used to prevent future failures.
    pub mistake_library: MistakeLibrary,
}
