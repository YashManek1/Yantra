//! # forge-stvp: Source-Truth Validation Protocol
//!
//! Refuses to let any task proceed without a cryptographically signed
//! `TruthToken`. The Interrogator classifies the incoming task, runs a
//! tailored questionnaire, builds `SOURCE_TRUTH.yaml`, passes it through
//! three Validators, and issues an `ed25519`-signed token to the orchestrator.
//!
//! ## Input
//! - A natural-language task description from the user or Night Mode planner
//! - User answers to the dynamic questionnaire (Strict: 5-7 q, Light: 3 q)
//!
//! ## Output
//! - `TruthToken` — `ed25519`-signed proof of validated intent
//! - `SOURCE_TRUTH.yaml` stored immutably in `.yantra/source_truth/`
//! - Optional `spec_{task_id}.rs` proptest file (Spec-as-Tests, v2)
//!
//! ## Related
//! - `forge-crg` — provides `existing_truth_refs` for Validator 2
//! - `forge-memory` — Truth Vault stores and retrieves signed artifacts
//! - `forge-orchestrator` — verifies the token before scheduling any task
//! - `forge-verifier` — Truth Drift Detector checks diffs against the token
