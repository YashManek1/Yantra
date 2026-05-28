//! # forge-night: Mode Policies
//!
//! Defines `ModePolicy` — the bundle of runtime guardrails active during a
//! Night Mode session — and four built-in policy constants covering the most
//! common operating contexts: interactive day use, autonomous overnight,
//! trusted-developer shortcut, and maximum-security enforcement.
//!
//! ## Input
//! - `Strictness` and `ModelTier` from `forge-core`
//!
//! ## Output
//! - `ModePolicy` values consumed by `NightSession` and the Rule Engine
//!
//! ## Related
//! - `forge-night::twilight` — selects a policy when constructing `NightSession`
//! - `forge-night::decision_rules` — `RuleEngine` enforces `cost_per_task_cap_usd`

use yantra_core::{ModelTier, Strictness};

/// Controls the approval and interaction model during a Night Mode run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Every agent action must be approved before execution.
    EveryAction,
    /// Only actions that match a `DecisionRule` are approved; all others defer.
    RulesOnly,
    /// No approval gates; the agent runs fully autonomously.
    None,
}

/// Governs how the runtime handles writes to sacred files during a Night Mode run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SacredPolicy {
    /// Sacred-file writes are entirely forbidden; any attempt halts the task.
    Forbidden,
    /// Sacred-file writes require an STVP `sacred_authorization` in the token.
    RequireAuth,
    /// Sacred-file writes are allowed when the ensemble agrees (three-variant consensus).
    AllowWithEnsemble,
}

/// The complete bundle of runtime guardrails for a Night Mode session.
#[derive(Debug, Clone)]
pub struct ModePolicy {
    /// STVP strictness level applied to tasks run under this policy.
    pub stvp_strictness: Strictness,
    /// Approval gate that controls when tasks may proceed autonomously.
    pub approval_gate: ApprovalMode,
    /// Maximum model routing tier permitted for any agent call in this session.
    pub model_tier_ceiling: ModelTier,
    /// Maximum allowed cost in USD per individual task.
    pub cost_per_task_cap_usd: f32,
    /// Policy governing sacred-file write attempts.
    pub sacred_file_writes: SacredPolicy,
    /// Maximum number of times the runtime retries a failing test before deferring.
    pub test_retry_limit: u8,
    /// Whether tasks whose confidence score falls below `confidence_threshold` are deferred.
    pub defer_on_low_confidence: bool,
    /// Minimum confidence score `[0.0, 1.0]` required to proceed without deferring.
    pub confidence_threshold: f32,
}

/// Interactive daytime policy — the user is present and approves every action.
///
/// Designed for supervised sessions where the developer is at the keyboard.
/// Enforces Strict STVP, caps model spend at Tier 2, and requires explicit
/// approval before every agent action. Sacred-file writes require auth.
pub const DAY_POLICY: ModePolicy = ModePolicy {
    stvp_strictness: Strictness::Strict,
    approval_gate: ApprovalMode::EveryAction,
    model_tier_ceiling: ModelTier::Tier2,
    cost_per_task_cap_usd: 0.50,
    sacred_file_writes: SacredPolicy::RequireAuth,
    test_retry_limit: 3,
    defer_on_low_confidence: true,
    confidence_threshold: 0.80,
};

/// Autonomous overnight policy — no user present; the runtime operates alone.
///
/// Conservative Tier-1 ceiling and rules-only approval mean the runtime defers
/// anything ambiguous rather than guessing. Sacred-file writes are forbidden;
/// any touch of a sacred file halts the task and documents the reason.
pub const NIGHT_POLICY: ModePolicy = ModePolicy {
    stvp_strictness: Strictness::Light,
    approval_gate: ApprovalMode::RulesOnly,
    model_tier_ceiling: ModelTier::Tier1,
    cost_per_task_cap_usd: 5.00,
    sacred_file_writes: SacredPolicy::Forbidden,
    test_retry_limit: 2,
    defer_on_low_confidence: true,
    confidence_threshold: 0.90,
};

/// Trusted-environment policy — senior developer, low-risk tasks, no gates.
///
/// Removes all approval friction: Trust-mode STVP skips the questionnaire,
/// no approval gates block execution, and the frontier tier is available.
/// Intended for solo developers running on pre-validated work items where
/// ensemble-approved sacred-file writes are acceptable.
pub const TRUST_POLICY: ModePolicy = ModePolicy {
    stvp_strictness: Strictness::Trust,
    approval_gate: ApprovalMode::None,
    model_tier_ceiling: ModelTier::Tier3,
    cost_per_task_cap_usd: 50.00,
    sacred_file_writes: SacredPolicy::AllowWithEnsemble,
    test_retry_limit: 5,
    defer_on_low_confidence: false,
    confidence_threshold: 0.50,
};

/// Maximum-security policy — every change is treated as potentially breaking.
///
/// Applies the highest STVP strictness, requires approval for every action,
/// and forbids all sacred-file writes. A single test failure triggers an
/// immediate halt with no retries. Intended for security-critical refactors
/// and compliance-gated migrations.
pub const STRICT_POLICY: ModePolicy = ModePolicy {
    stvp_strictness: Strictness::Strict,
    approval_gate: ApprovalMode::EveryAction,
    model_tier_ceiling: ModelTier::Tier3,
    cost_per_task_cap_usd: 1.00,
    sacred_file_writes: SacredPolicy::Forbidden,
    test_retry_limit: 1,
    defer_on_low_confidence: true,
    confidence_threshold: 0.95,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_policy_requires_approval_for_every_action() {
        assert_eq!(DAY_POLICY.approval_gate, ApprovalMode::EveryAction);
    }

    #[test]
    fn day_policy_uses_strict_stvp() {
        assert_eq!(DAY_POLICY.stvp_strictness, Strictness::Strict);
    }

    #[test]
    fn night_policy_forbids_sacred_file_writes() {
        assert_eq!(NIGHT_POLICY.sacred_file_writes, SacredPolicy::Forbidden);
    }

    #[test]
    fn night_policy_uses_rules_only_approval() {
        assert_eq!(NIGHT_POLICY.approval_gate, ApprovalMode::RulesOnly);
    }

    #[test]
    fn night_policy_model_tier_ceiling_is_tier1() {
        assert_eq!(NIGHT_POLICY.model_tier_ceiling, ModelTier::Tier1);
    }

    #[test]
    fn trust_policy_has_no_approval_gates() {
        assert_eq!(TRUST_POLICY.approval_gate, ApprovalMode::None);
    }

    #[test]
    fn trust_policy_does_not_defer_on_low_confidence() {
        let defer = TRUST_POLICY.defer_on_low_confidence;
        assert!(!defer);
    }

    #[test]
    fn trust_policy_allows_ensemble_sacred_writes() {
        assert_eq!(
            TRUST_POLICY.sacred_file_writes,
            SacredPolicy::AllowWithEnsemble
        );
    }

    #[test]
    fn strict_policy_has_lowest_test_retry_limit() {
        assert_eq!(STRICT_POLICY.test_retry_limit, 1);
    }

    #[test]
    fn strict_policy_forbids_sacred_file_writes() {
        assert_eq!(STRICT_POLICY.sacred_file_writes, SacredPolicy::Forbidden);
    }

    #[test]
    fn policy_confidence_thresholds_are_strictly_ordered() {
        let strict = STRICT_POLICY.confidence_threshold;
        let night = NIGHT_POLICY.confidence_threshold;
        let day = DAY_POLICY.confidence_threshold;
        let trust = TRUST_POLICY.confidence_threshold;
        assert!(strict > night, "strict ({strict}) must exceed night ({night})");
        assert!(strict > day, "strict ({strict}) must exceed day ({day})");
        assert!(strict > trust, "strict ({strict}) must exceed trust ({trust})");
        assert!(night > day, "night ({night}) must exceed day ({day})");
    }

    #[test]
    fn trust_policy_has_highest_cost_cap() {
        let trust_cap = TRUST_POLICY.cost_per_task_cap_usd;
        let day_cap = DAY_POLICY.cost_per_task_cap_usd;
        let night_cap = NIGHT_POLICY.cost_per_task_cap_usd;
        let strict_cap = STRICT_POLICY.cost_per_task_cap_usd;
        assert!(trust_cap > day_cap);
        assert!(trust_cap > night_cap);
        assert!(trust_cap > strict_cap);
    }
}
