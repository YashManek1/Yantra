//! # forge-orchestrator: Cognitive `DAG` Scheduler and Debate Engine
//!
//! Accepts a validated `TruthToken` and schedules a directed acyclic graph of
//! agent tasks, resolving cross-agent conflicts via the Debate Engine and
//! optimising task ordering with the `CSP` Planner. The Speculation Engine
//! pre-fetches likely subtasks to reduce latency.
//!
//! ## Input
//! - `TruthToken` from `forge-stvp` (task scheduling is gated on this)
//! - `AgentEvent` messages from the multi-agent event bus
//! - Heartbeat signals from `forge-obs` watchdog
//!
//! ## Output
//! - `AgentTask` structs dispatched to specialist agents in `forge-agents`
//! - `DecisionRecord` entries written to `decisions.sqlite`
//! - Live `DAG` state streamed to `forge-serve` for the Live Canvas
//!
//! ## Related
//! - `forge-stvp` — provides the `TruthToken`; no task runs without one
//! - `forge-agents` — receives and executes dispatched tasks
//! - `forge-obs` — every `DAG` node is an `OTel` span; watchdog monitors health
