//! # forge-skills: SKILL.md Registry and Skill-Learning Loop
//!
//! Maintains the Git-backed registry of `SKILL.md` workflow documents and
//! runs the skill-learning loop that converts Postmortem Engine findings into
//! new or updated skills that future agent sessions can retrieve.
//!
//! ## Input
//! - `PostmortemSnippet` records from completed agent tasks
//! - Skill retrieval queries from agents preparing their prompt context
//!
//! ## Output
//! - New or updated `skills/*.md` files committed to Git
//! - Matched skill documents injected into agent context windows
//!
//! ## Related
//! - `forge-agents` — retrieves relevant skills before each task
//! - `forge-memory` — Mistake Library feeds the skill-learning loop
