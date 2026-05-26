//! # forge-tokenizer: `BPE` Token Counter and Budget Enforcer
//!
//! Counts tokens using a `BPE` vocabulary and enforces hard budget limits
//! before any text is forwarded to a model. All context-assembly paths
//! in Yantra pass through this crate to prevent overruns.
//!
//! ## Input
//! - Raw text strings or byte slices
//! - `token_budget: usize` — hard ceiling on output token count
//!
//! ## Output
//! - Token counts and (optionally) truncated or split buffers that fit
//!   within the supplied budget
//!
//! ## Related
//! - `forge-crg` — uses token counts to bound subgraph rendering
//! - `forge-router` — checks budget before every LLM call
