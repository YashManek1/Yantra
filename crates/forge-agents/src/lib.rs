//! # forge-agents: Specialist Agent Implementations
//!
//! Contains the event loops for every specialist agent role. Each agent receives
//! a task assignment from the orchestrator, queries the CRG for context via
//! MCP, calls the model through the router, and returns a `TaskResult` with an
//! optional unified diff.
//!
//! ## Input
//! - `TaskNode` dispatched by `forge-orchestrator` (always carries a `TruthToken`)
//! - `RenderedSubgraph` from `forge-crg` via the `crg.subgraph` MCP tool
//! - Tool responses from `forge-tools` during the agent action loop
//!
//! ## Output
//! - `TaskResult` containing proposed diffs forwarded to `forge-verifier`
//! - `AgentEvent` messages published to the multi-agent event bus
//!
//! ## Related
//! - `forge-router` — all LLM calls route through the model router
//! - `forge-tools` — agents invoke MCP tools via this crate
//! - `forge-verifier` — every diff passes through the three gates

pub mod agent;
pub mod coder;
pub mod committer;
pub mod error;
pub mod red_team;
pub mod refactorer;
pub mod researcher;
pub mod tester;
pub mod verifier_agent;

pub use agent::{Agent, AgentContext, Diff, FileDiff, TaskResult};
pub use coder::{parse_diffs_from_response, CoderAgent};
pub use committer::{CommitMessage, CommitSigningKey, CommitterAgent};
pub use error::AgentError;
pub use red_team::RedTeamAgent;
pub use refactorer::RefactorerAgent;
pub use researcher::{parse_research_memo, ResearchFinding, ResearchMemo, ResearcherAgent};
pub use tester::{run_and_report_test_failures, TesterAgent};
pub use verifier_agent::{
    extract_project_root_from_task, parse_verifier_verdict, VerifierAgent, VerifierVerdict,
};
