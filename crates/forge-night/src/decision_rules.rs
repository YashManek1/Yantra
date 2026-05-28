//! # forge-night: Decision Rules Engine
//!
//! Defines the rule vocabulary (`DecisionRule`, `RuleCondition`, `RuleAction`)
//! and the `RuleEngine` resolver that maps a `NightEvent` to the highest-priority
//! `RuleAction` from the active rule set. When no rule matches, `resolve` returns
//! `None`; callers must treat an absent match as an implicit `TagWipAndDefer`.
//!
//! ## Input
//! - `Vec<DecisionRule>` collected during Twilight from the user (or defaults)
//! - `NightEvent` emitted by the Night Run executor when notable conditions arise
//!
//! ## Output
//! - `Option<RuleAction>` — the action of the highest-priority matching rule, or
//!   `None` to signal that the safe-default defer behaviour should apply
//!
//! ## Related
//! - `forge-night::twilight` — collects rules during the Twilight phase
//! - `forge-night::mode_policy` — `ModePolicy.test_retry_limit` feeds rule thresholds

use std::time::Duration;

/// An event emitted by the Night Run executor when a notable condition occurs.
///
/// Each variant maps to exactly one `RuleCondition` discriminant so the
/// match logic in `RuleCondition::matches` stays exhaustive and type-safe.
#[derive(Debug, Clone, PartialEq)]
pub enum NightEvent {
    /// A test suite failed after `retry_count` prior retry attempts.
    TestFailure {
        /// Number of retries already attempted for this test run.
        retry_count: u8,
    },
    /// Cumulative task cost has crossed a configured threshold.
    CostThresholdExceeded {
        /// Total USD spent on the task so far.
        total_usd: f32,
    },
    /// An external API returned an HTTP error status.
    ExternalApiFailure {
        /// HTTP status code returned by the external service.
        status_code: u16,
    },
    /// A merge conflict was detected while integrating changes.
    MergeConflictDetected,
    /// A static-analysis or verifier pass found a security issue.
    SecurityIssueDetected,
    /// The agent's self-reported confidence score fell below a policy threshold.
    LowConfidenceScore {
        /// Confidence score in the range `[0.0, 1.0]`.
        score: f32,
    },
    /// The agent attempted to read or write a file not present in the CRG.
    UnknownFileAccessed,
}

/// Predicate that determines whether a `DecisionRule` fires for a given `NightEvent`.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleCondition {
    /// Fires when a test fails and at least `n` retries have already been attempted.
    TestFailureAfterRetries(u8),
    /// Fires when the cumulative task cost strictly exceeds `cap` USD.
    CostExceeds(f32),
    /// Fires when an external API returns exactly `status_code`.
    ExternalApiError(u16),
    /// Fires on any merge conflict.
    MergeConflict,
    /// Fires when the verifier detects a security issue.
    SecurityIssueFound,
    /// Fires when the agent confidence score is strictly below `threshold`.
    ConfidenceBelow(f32),
    /// Fires when the agent accesses a file not present in the CRG.
    UnknownFileTouched,
}

impl RuleCondition {
    /// Returns `true` when this condition matches `event`.
    pub fn matches(&self, event: &NightEvent) -> bool {
        match (self, event) {
            (Self::TestFailureAfterRetries(threshold), NightEvent::TestFailure { retry_count }) => {
                retry_count >= threshold
            }
            (Self::CostExceeds(cap), NightEvent::CostThresholdExceeded { total_usd }) => {
                total_usd > cap
            }
            (
                Self::ExternalApiError(expected_code),
                NightEvent::ExternalApiFailure { status_code },
            ) => status_code == expected_code,
            (Self::MergeConflict, NightEvent::MergeConflictDetected) => true,
            (Self::SecurityIssueFound, NightEvent::SecurityIssueDetected) => true,
            (Self::ConfidenceBelow(threshold), NightEvent::LowConfidenceScore { score }) => {
                score < threshold
            }
            (Self::UnknownFileTouched, NightEvent::UnknownFileAccessed) => true,
            _ => false,
        }
    }
}

/// The action the runtime takes when a matching `DecisionRule` fires.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleAction {
    /// Mark the task as a WIP branch and place it in the deferred queue.
    TagWipAndDefer,
    /// Stop the Night Run immediately, optionally waking the user.
    HardStop {
        /// Whether to wake the user with a push notification.
        notify: bool,
    },
    /// Pause for `duration`, then execute a follow-up action.
    Wait {
        /// How long to pause before taking the follow-up action.
        duration: Duration,
        /// Action to execute after the wait expires.
        then: Box<RuleAction>,
    },
    /// Attempt a rebase; execute the fallback action if the rebase fails.
    RebaseAttempt {
        /// Action to execute if the rebase fails.
        on_fail: Box<RuleAction>,
    },
    /// Halt the task and write a full incident document for post-mortem review.
    HaltAndDocument,
    /// Skip the current task and continue with the next one without failing.
    SkipTask,
}

/// A single policy rule evaluated by the `RuleEngine` against incoming events.
#[derive(Debug, Clone)]
pub struct DecisionRule {
    /// The event predicate that activates this rule.
    pub condition: RuleCondition,
    /// The action the runtime takes when `condition` matches.
    pub action: RuleAction,
    /// Resolution priority — higher values beat lower values when multiple rules
    /// match the same event. When two rules share the same priority, the one
    /// defined first in the slice wins.
    pub priority: u32,
}

/// Evaluates a set of `DecisionRule`s against a `NightEvent` and returns the
/// action of the highest-priority matching rule.
pub struct RuleEngine;

impl RuleEngine {
    /// Returns the action of the highest-priority rule that matches `event`.
    ///
    /// Returns `None` when no rule matches. Callers must treat `None` as an
    /// implicit `TagWipAndDefer` — the safe default for unhandled conditions.
    pub fn resolve(event: &NightEvent, rules: &[DecisionRule]) -> Option<RuleAction> {
        rules
            .iter()
            .filter(|rule| rule.condition.matches(event))
            .max_by_key(|rule| rule.priority)
            .map(|rule| rule.action.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn test_failure_defer_rule(priority: u32) -> DecisionRule {
        DecisionRule {
            condition: RuleCondition::TestFailureAfterRetries(2),
            action: RuleAction::TagWipAndDefer,
            priority,
        }
    }

    fn cost_hard_stop_rule(priority: u32) -> DecisionRule {
        DecisionRule {
            condition: RuleCondition::CostExceeds(10.0),
            action: RuleAction::HardStop { notify: true },
            priority,
        }
    }

    // --- no-match cases ---

    #[test]
    fn resolve_returns_none_when_no_rules_match() {
        let rules = vec![test_failure_defer_rule(10)];
        let unmatched_event = NightEvent::MergeConflictDetected;
        assert_eq!(RuleEngine::resolve(&unmatched_event, &rules), None);
    }

    #[test]
    fn resolve_empty_rules_slice_returns_none() {
        let event = NightEvent::SecurityIssueDetected;
        assert_eq!(RuleEngine::resolve(&event, &[]), None);
    }

    // --- single-match cases ---

    #[test]
    fn resolve_returns_action_for_matching_test_failure_rule() {
        let rules = vec![test_failure_defer_rule(5)];
        let event = NightEvent::TestFailure { retry_count: 3 };
        assert_eq!(
            RuleEngine::resolve(&event, &rules),
            Some(RuleAction::TagWipAndDefer)
        );
    }

    #[test]
    fn resolve_returns_hard_stop_for_cost_exceeded() {
        let rules = vec![cost_hard_stop_rule(10)];
        let event = NightEvent::CostThresholdExceeded { total_usd: 10.01 };
        assert_eq!(
            RuleEngine::resolve(&event, &rules),
            Some(RuleAction::HardStop { notify: true })
        );
    }

    #[test]
    fn resolve_returns_halt_for_security_issue() {
        let rules = vec![DecisionRule {
            condition: RuleCondition::SecurityIssueFound,
            action: RuleAction::HaltAndDocument,
            priority: 50,
        }];
        let event = NightEvent::SecurityIssueDetected;
        assert_eq!(
            RuleEngine::resolve(&event, &rules),
            Some(RuleAction::HaltAndDocument)
        );
    }

    #[test]
    fn resolve_returns_skip_for_unknown_file() {
        let rules = vec![DecisionRule {
            condition: RuleCondition::UnknownFileTouched,
            action: RuleAction::SkipTask,
            priority: 5,
        }];
        let event = NightEvent::UnknownFileAccessed;
        assert_eq!(
            RuleEngine::resolve(&event, &rules),
            Some(RuleAction::SkipTask)
        );
    }

    // --- boundary / threshold cases ---

    #[test]
    fn test_failure_condition_does_not_match_below_retry_threshold() {
        let rules = vec![test_failure_defer_rule(10)];
        let event = NightEvent::TestFailure { retry_count: 1 };
        assert_eq!(RuleEngine::resolve(&event, &rules), None);
    }

    #[test]
    fn test_failure_condition_matches_at_exact_retry_threshold() {
        let rules = vec![test_failure_defer_rule(10)];
        let event = NightEvent::TestFailure { retry_count: 2 };
        assert_eq!(
            RuleEngine::resolve(&event, &rules),
            Some(RuleAction::TagWipAndDefer)
        );
    }

    #[test]
    fn cost_exceeds_condition_does_not_match_exactly_at_cap() {
        let rules = vec![cost_hard_stop_rule(10)];
        let event = NightEvent::CostThresholdExceeded { total_usd: 10.0 };
        assert_eq!(RuleEngine::resolve(&event, &rules), None);
    }

    #[test]
    fn confidence_below_condition_matches_below_threshold() {
        let rules = vec![DecisionRule {
            condition: RuleCondition::ConfidenceBelow(0.7),
            action: RuleAction::TagWipAndDefer,
            priority: 10,
        }];
        let event = NightEvent::LowConfidenceScore { score: 0.65 };
        assert_eq!(
            RuleEngine::resolve(&event, &rules),
            Some(RuleAction::TagWipAndDefer)
        );
    }

    #[test]
    fn confidence_below_condition_does_not_match_at_or_above_threshold() {
        let rules = vec![DecisionRule {
            condition: RuleCondition::ConfidenceBelow(0.7),
            action: RuleAction::TagWipAndDefer,
            priority: 10,
        }];
        assert_eq!(
            RuleEngine::resolve(&NightEvent::LowConfidenceScore { score: 0.7 }, &rules),
            None
        );
        assert_eq!(
            RuleEngine::resolve(&NightEvent::LowConfidenceScore { score: 0.75 }, &rules),
            None
        );
    }

    #[test]
    fn external_api_error_matches_exact_status_code_only() {
        let rules = vec![DecisionRule {
            condition: RuleCondition::ExternalApiError(429),
            action: RuleAction::Wait {
                duration: Duration::from_secs(30),
                then: Box::new(RuleAction::SkipTask),
            },
            priority: 15,
        }];
        assert!(
            RuleEngine::resolve(&NightEvent::ExternalApiFailure { status_code: 429 }, &rules)
                .is_some()
        );
        assert_eq!(
            RuleEngine::resolve(&NightEvent::ExternalApiFailure { status_code: 500 }, &rules),
            None
        );
    }

    // --- priority ordering ---

    #[test]
    fn resolve_returns_highest_priority_action_when_multiple_rules_match() {
        let low_priority_rule = DecisionRule {
            condition: RuleCondition::TestFailureAfterRetries(1),
            action: RuleAction::SkipTask,
            priority: 1,
        };
        let high_priority_rule = DecisionRule {
            condition: RuleCondition::TestFailureAfterRetries(2),
            action: RuleAction::HaltAndDocument,
            priority: 100,
        };
        let event = NightEvent::TestFailure { retry_count: 3 };
        let rules = vec![low_priority_rule, high_priority_rule];
        assert_eq!(
            RuleEngine::resolve(&event, &rules),
            Some(RuleAction::HaltAndDocument)
        );
    }

    #[test]
    fn priority_order_is_descending_highest_wins_across_three_rules() {
        let rules = vec![
            DecisionRule {
                condition: RuleCondition::UnknownFileTouched,
                action: RuleAction::SkipTask,
                priority: 10,
            },
            DecisionRule {
                condition: RuleCondition::UnknownFileTouched,
                action: RuleAction::HardStop { notify: false },
                priority: 90,
            },
            DecisionRule {
                condition: RuleCondition::UnknownFileTouched,
                action: RuleAction::TagWipAndDefer,
                priority: 50,
            },
        ];
        assert_eq!(
            RuleEngine::resolve(&NightEvent::UnknownFileAccessed, &rules),
            Some(RuleAction::HardStop { notify: false })
        );
    }

    // --- composite action round-trips ---

    #[test]
    fn wait_action_round_trips_through_rule_engine() {
        let wait_then_defer = RuleAction::Wait {
            duration: Duration::from_secs(60),
            then: Box::new(RuleAction::TagWipAndDefer),
        };
        let rules = vec![DecisionRule {
            condition: RuleCondition::MergeConflict,
            action: wait_then_defer.clone(),
            priority: 20,
        }];
        assert_eq!(
            RuleEngine::resolve(&NightEvent::MergeConflictDetected, &rules),
            Some(wait_then_defer)
        );
    }

    #[test]
    fn rebase_attempt_action_round_trips_through_rule_engine() {
        let rebase_action = RuleAction::RebaseAttempt {
            on_fail: Box::new(RuleAction::HardStop { notify: true }),
        };
        let rules = vec![DecisionRule {
            condition: RuleCondition::MergeConflict,
            action: rebase_action.clone(),
            priority: 30,
        }];
        assert_eq!(
            RuleEngine::resolve(&NightEvent::MergeConflictDetected, &rules),
            Some(rebase_action)
        );
    }

    // --- integration: synthetic event stream ---

    #[test]
    fn event_stream_produces_correct_dispositions() {
        let rules = vec![
            DecisionRule {
                condition: RuleCondition::SecurityIssueFound,
                action: RuleAction::HaltAndDocument,
                priority: 100,
            },
            DecisionRule {
                condition: RuleCondition::CostExceeds(20.0),
                action: RuleAction::HardStop { notify: true },
                priority: 80,
            },
            DecisionRule {
                condition: RuleCondition::TestFailureAfterRetries(3),
                action: RuleAction::TagWipAndDefer,
                priority: 50,
            },
            DecisionRule {
                condition: RuleCondition::MergeConflict,
                action: RuleAction::RebaseAttempt {
                    on_fail: Box::new(RuleAction::HardStop { notify: false }),
                },
                priority: 40,
            },
        ];

        let event_dispositions: &[(&NightEvent, Option<RuleAction>)] = &[
            (&NightEvent::TestFailure { retry_count: 2 }, None),
            (
                &NightEvent::TestFailure { retry_count: 3 },
                Some(RuleAction::TagWipAndDefer),
            ),
            (
                &NightEvent::CostThresholdExceeded { total_usd: 25.0 },
                Some(RuleAction::HardStop { notify: true }),
            ),
            (
                &NightEvent::SecurityIssueDetected,
                Some(RuleAction::HaltAndDocument),
            ),
            (
                &NightEvent::MergeConflictDetected,
                Some(RuleAction::RebaseAttempt {
                    on_fail: Box::new(RuleAction::HardStop { notify: false }),
                }),
            ),
            (&NightEvent::UnknownFileAccessed, None),
        ];

        for (event, expected_action) in event_dispositions {
            let actual_action = RuleEngine::resolve(event, &rules);
            assert_eq!(
                &actual_action, expected_action,
                "unexpected disposition for event {event:?}"
            );
        }
    }
}
