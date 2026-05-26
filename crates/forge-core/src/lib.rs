//! # forge-core: Shared Types and Traits
//!
//! Foundational vocabulary types shared across every Yantra crate. Nothing
//! in this crate has runtime side-effects; it exists purely to give all
//! other crates a common type language.
//!
//! ## Input
//! - None (foundation crate — has no internal dependencies)
//!
//! ## Output
//! - `TaskId`, `TruthToken`, `AgentKind`, `ModelId`, `Span`, `Outcome`
//!   and supporting error types
//!
//! ## Related
//! - Every other `yantra-*` crate depends on this one
