//! # Memory Service
//!
//! Unified facade over all five memory tiers. Phase 4 agents receive a
//! single `MemoryService` handle and do not manage individual store handles.
//!
//! ## Input
//! - `db_path: &Path` — path to memory.sqlite in the .yantra directory
//!
//! ## Output
//! - `MemoryService` — facade over Recall, Archival, TKG, TruthVault, MistakeLibrary
//!
//! ## Related
//! - `forge-memory::recall` — short-term session memory
//! - `forge-memory::archival` — compressed long-term memory
//! - `forge-memory::mistake_library` — prevention rules for future tasks

use std::path::Path;

use uuid::Uuid;
use yantra_core::TaskClass;

use crate::{
    archival::ArchivalStore,
    mistake_library::{MistakeLibraryStore, MistakeRecord},
    recall::RecallStore,
    tkg::TemporalKnowledgeGraph,
    truth_vault::TruthVault,
    MemoryError,
};

/// Unified facade over all five memory tiers.
pub struct MemoryService {
    recall_store: RecallStore,
    archival_store: ArchivalStore,
    knowledge_graph: TemporalKnowledgeGraph,
    truth_vault_store: TruthVault,
    mistake_library_store: MistakeLibraryStore,
}

impl MemoryService {
    /// Creates a new `MemoryService`, opening all five stores at `db_path`.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` if any of the five stores cannot be opened or
    /// their migrations cannot be applied.
    pub fn new(db_path: &Path) -> Result<Self, MemoryError> {
        Ok(Self {
            recall_store: RecallStore::new(db_path)?,
            archival_store: ArchivalStore::new(db_path)?,
            knowledge_graph: TemporalKnowledgeGraph::new(db_path)?,
            truth_vault_store: TruthVault::new(db_path)?,
            mistake_library_store: MistakeLibraryStore::new(db_path)?,
        })
    }

    /// Returns a reference to the short-term recall store.
    pub fn recall(&self) -> &RecallStore {
        &self.recall_store
    }

    /// Returns a reference to the compressed archival store.
    pub fn archival(&self) -> &ArchivalStore {
        &self.archival_store
    }

    /// Returns a reference to the temporal knowledge graph.
    pub fn knowledge_graph(&self) -> &TemporalKnowledgeGraph {
        &self.knowledge_graph
    }

    /// Returns a reference to the STVP truth vault.
    pub fn truth_vault(&self) -> &TruthVault {
        &self.truth_vault_store
    }

    /// Returns a reference to the mistake library.
    pub fn mistake_library(&self) -> &MistakeLibraryStore {
        &self.mistake_library_store
    }

    /// Assembles a context prompt for an agent working on a task of the given class.
    ///
    /// Combines recent prevention rules from the mistake library into a prompt string.
    /// Phase 4 will enrich this with recall and archival shards.
    ///
    /// # Errors
    ///
    /// Returns `MemoryError` if the mistake library query fails.
    pub fn assemble_agent_context(
        &self,
        _session_id: Uuid,
        task_class: TaskClass,
        _task_description: &str,
        _token_budget: usize,
    ) -> Result<String, MemoryError> {
        let prevention_rules: Vec<MistakeRecord> = self
            .mistake_library_store
            .query_for_task_class(task_class)?;

        if prevention_rules.is_empty() {
            return Ok(String::new());
        }

        let rules_text = prevention_rules
            .iter()
            .map(|mistake_record| format!("- {}", mistake_record.prevention_rule))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(format!(
            "## Prevention Rules (from Mistake Library)\n{rules_text}\n\n"
        ))
    }
}
