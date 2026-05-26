//! # forge-memory: Four-Tier Memory Service
//!
//! Implements Working, Recall (episodic), Archival (long-term), and Temporal
//! Knowledge Graph (TKG) memory tiers, plus the Truth Vault (stores YAML
//! SOURCE_TRUTH artifacts) and the Mistake Library (failure → prevention rules).
//! A background heartbeat task keeps the tiers compact and coherent.
//!
//! ## Input
//! - Memory write events from agents, the orchestrator, and Night Mode
//! - Recall queries with recency or semantic filters
//! - Heartbeat interval from `configs/memory.toml`
//!
//! ## Output
//! - `AssembledPrompt` injected into each agent's context window
//! - Truth YAML retrieved by `forge-stvp` during token verification
//! - `MistakeRecord` prevention rules forwarded to the prompt assembler
//!
//! ## Related
//! - `forge-stvp` — reads/writes Truth Vault entries
//! - `forge-agents` — every agent loop reads from and writes to memory
//! - `forge-obs` — observability spans wrap all memory operations

pub mod archival;
pub mod error;
pub mod heartbeat;
pub mod mistake_library;
pub mod recall;
pub mod tkg;
pub mod truth_vault;
pub mod working;

pub use archival::{ArchivalStore, MemoryShard, ShardType};
pub use error::{MemoryError, MemoryResult};
pub use heartbeat::{start_heartbeat, HeartbeatHandle};
pub use mistake_library::{MistakeLibraryStore, MistakeRecord};
pub use recall::{ConversationTurn, RecallStore, SessionSummary};
pub use tkg::{Fact, TemporalKnowledgeGraph};
pub use truth_vault::{TruthHash, TruthVault};
pub use working::{assemble_prompt, AssembledPrompt};
