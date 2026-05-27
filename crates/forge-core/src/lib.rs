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

pub mod agent;
pub mod db;
pub mod error;
pub mod id;
pub mod model;
pub mod path;
pub mod span;
pub mod task;
pub mod truth;
pub mod workspace;

pub use agent::{AgentCapability, AgentKind, AgentSpec};
pub use db::{apply_migrations, connection_pool, DbPool, MIGRATION_VERSION_TABLE_SQL};
pub use error::{CoreError, CoreResult};
pub use id::{hex_encode, sha256_hex, DecisionId, SessionId, SpanId, SymbolId, TaskId};
pub use model::{ModelCapability, ModelId, ModelTier};
pub use path::{canonicalize_within, is_sacred, load_sacred_patterns, ProjectRoot};
pub use span::{ErrorRecord, Outcome, Span};
pub use task::{
    AugmentFuture, AugmentedQuestionTuple, AugmenterPort, TaskClass, TaskNode, TaskStatus,
};
pub use truth::{Strictness, TruthToken, VerifyingKey};
pub use workspace::WorkspaceMode;
