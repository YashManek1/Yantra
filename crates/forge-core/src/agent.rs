//! # forge-core: Agent Types
//!
//! Defines the specialist agent kinds and capability declarations used by the
//! orchestrator, tool router, and audit layer. These types describe what an
//! agent is allowed to do, without implementing any agent behavior.
//!
//! ## Input
//! - Static agent policy from configuration files
//! - Tool capability names assigned by the orchestrator
//!
//! ## Output
//! - `AgentKind`, `AgentCapability`, and `AgentSpec`
//!
//! ## Related
//! - `forge-core::task` — assigns tasks to agents
//! - `forge-core::span` — records which agent produced a span

use serde::{Deserialize, Serialize};

/// Specialist agent role known to the Yantra runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    /// Runs the `STVP` questionnaire and intent extraction flow.
    Interrogator,
    /// Gathers external or repository context before implementation.
    Researcher,
    /// Produces code diffs.
    Coder,
    /// Produces and runs tests.
    Tester,
    /// Performs behavior-preserving code restructuring.
    Refactorer,
    /// Attempts adversarial review of generated changes.
    RedTeam,
    /// Finalizes accepted changes.
    Committer,
    /// Monitors autonomous Night Mode execution.
    NightWatchman,
    /// Validates source-truth artifacts.
    TruthValidator,
    /// Resolves conflicts between agent outputs.
    ConflictResolver,
    /// Checks runtime integrity and invariants.
    IntegrityChecker,
}

/// Capability granted to an agent by policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentCapability {
    /// Read files from the workspace.
    ReadFiles,
    /// Write files in the permitted workspace.
    WriteFiles,
    /// Run shell commands.
    RunShell,
    /// Access network resources.
    AccessNetwork,
    /// Write Git state.
    GitWrite,
    /// Write files guarded by sacred-file policy.
    SacredWrite,
    /// Mutate durable memory stores.
    MutateMemory,
}

/// Static agent policy used by routing and tool authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Agent role.
    pub kind: AgentKind,
    /// Capabilities granted to the agent.
    pub capabilities: Vec<AgentCapability>,
    /// Tool names the agent may invoke.
    pub permitted_tools: Vec<String>,
}
