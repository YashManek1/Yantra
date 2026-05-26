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
//! - `AgentEvent` messages published to the multi-agent event bus (Day 3+)
//!
//! ## Related
//! - `forge-router` — all LLM calls route through the model router
//! - `forge-tools` — agents invoke MCP tools via this crate
//! - `forge-verifier` — every diff passes through the three gates (Day 4+)

pub mod agent;
pub mod coder;
pub mod error;

pub use agent::{Agent, AgentContext, Diff, FileDiff, TaskResult};
pub use coder::CoderAgent;
pub use error::AgentError;
