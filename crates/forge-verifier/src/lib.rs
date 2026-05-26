//! # forge-verifier: Three-Gate Diff Verification
//!
//! Every diff produced by a Coder agent passes three sequential gates before
//! it can be handed to the Committer: Truth Drift Detection (did the diff
//! stay within STVP scope?), Static Analysis (clippy/mypy/tsc), and the
//! Boolean Exit Gate (all tests pass, no hallucinated symbols).
//!
//! ## Input
//! - A proposed diff from a Coder or Refactorer agent
//! - The `TruthToken` that governs the current task
//! - LSP diagnostics and AST symbol tables for hallucination checking
//!
//! ## Output
//! - `VerificationOutcome::Approved` — diff is forwarded to the Committer
//! - `VerificationOutcome::Rejected { reason }` — diff is returned to the
//!   originating agent for revision, with a structured failure reason
//!
//! ## Related
//! - `forge-stvp` — provides the `TruthToken` for drift boundary checking
//! - `forge-crg` — graph queries power the hallucination cross-check
//! - `forge-lsp` — LSP diagnostics feed the static-analysis gate
