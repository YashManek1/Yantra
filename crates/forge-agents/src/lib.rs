//! # forge-agents: Specialist Agent Implementations
//!
//! Contains the event loops for every specialist agent role: Coder, Tester,
//! Researcher, Refactorer, Committer, and Red Team. Each agent receives a
//! task assignment from the orchestrator, queries the CRG for context, calls
//! the model through the router, and routes its output through the verifier.
//!
//! ## Input
//! - `AgentTask` dispatched by `forge-orchestrator` (always carries a `TruthToken`)
//! - `RenderedSubgraph` from `forge-crg` as the primary code context
//! - Tool responses from `forge-tools` during the agent's action loop
//!
//! ## Output
//! - Proposed diffs forwarded to `forge-verifier`
//! - Memory writes committed via `forge-memory`
//! - `AgentEvent` messages published to the multi-agent event bus
//!
//! ## Related
//! - `forge-router` — all LLM calls route through the model router
//! - `forge-tools` — agents invoke MCP tools via this crate
//! - `forge-verifier` — every diff passes through the three gates
