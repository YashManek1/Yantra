//! # forge-night integration tests: Decision Rule Engine Accuracy
//!
//! Verifies that `RuleEngine::resolve` produces the correct `RuleAction` for
//! every (event, rule_set, expected_action) triple in a 15-entry scenario matrix.
//!
//! The decision rules engine is fully deterministic — given the same event and
//! rule slice it must always return the same action — so the acceptance criterion
//! is 100% correctness across all 15 scenarios.
//!
//! ## Input
//! - 15 labeled `(NightEvent, Vec<DecisionRule>, Option<RuleAction>)` triples
//!   covering all `RuleCondition` discriminants, priority ordering, threshold
//!   boundaries, and the no-match case
//!
//! ## Output
//! - Pass/fail assertions; any failure indicates a regression in rule resolution
//!
//! ## Related
//! - `forge-night::decision_rules` — `RuleEngine` under test

use std::time::Duration;

use yantra_night::decision_rules::{
    DecisionRule, NightEvent, RuleAction, RuleCondition, RuleEngine,
};

/// One labeled entry in the scenario matrix.
struct Scenario {
    /// Human-readable label used in assertion messages.
    label: &'static str,
    /// The event dispatched to the rule engine.
    event: NightEvent,
    /// The ordered slice of rules evaluated against `event`.
    rules: Vec<DecisionRule>,
    /// The action the engine must return, or `None` when no rule should match.
    expected_action: Option<RuleAction>,
}

fn build_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            label: "security issue maps to HaltAndDocument at priority 100",
            event: NightEvent::SecurityIssueDetected,
            rules: vec![DecisionRule {
                condition: RuleCondition::SecurityIssueFound,
                action: RuleAction::HaltAndDocument,
                priority: 100,
            }],
            expected_action: Some(RuleAction::HaltAndDocument),
        },
        Scenario {
            label: "cost exceeded above cap maps to HardStop(notify=true)",
            event: NightEvent::CostThresholdExceeded { total_usd: 25.1 },
            rules: vec![DecisionRule {
                condition: RuleCondition::CostExceeds(25.0),
                action: RuleAction::HardStop { notify: true },
                priority: 80,
            }],
            expected_action: Some(RuleAction::HardStop { notify: true }),
        },
        Scenario {
            label: "cost exactly at cap does NOT match (strict greater-than)",
            event: NightEvent::CostThresholdExceeded { total_usd: 25.0 },
            rules: vec![DecisionRule {
                condition: RuleCondition::CostExceeds(25.0),
                action: RuleAction::HardStop { notify: true },
                priority: 80,
            }],
            expected_action: None,
        },
        Scenario {
            label: "test failure at exact retry threshold maps to TagWipAndDefer",
            event: NightEvent::TestFailure { retry_count: 3 },
            rules: vec![DecisionRule {
                condition: RuleCondition::TestFailureAfterRetries(3),
                action: RuleAction::TagWipAndDefer,
                priority: 50,
            }],
            expected_action: Some(RuleAction::TagWipAndDefer),
        },
        Scenario {
            label: "test failure below retry threshold returns None",
            event: NightEvent::TestFailure { retry_count: 2 },
            rules: vec![DecisionRule {
                condition: RuleCondition::TestFailureAfterRetries(3),
                action: RuleAction::TagWipAndDefer,
                priority: 50,
            }],
            expected_action: None,
        },
        Scenario {
            label: "merge conflict maps to RebaseAttempt",
            event: NightEvent::MergeConflictDetected,
            rules: vec![DecisionRule {
                condition: RuleCondition::MergeConflict,
                action: RuleAction::RebaseAttempt {
                    on_fail: Box::new(RuleAction::HardStop { notify: false }),
                },
                priority: 40,
            }],
            expected_action: Some(RuleAction::RebaseAttempt {
                on_fail: Box::new(RuleAction::HardStop { notify: false }),
            }),
        },
        Scenario {
            label: "unknown file accessed maps to SkipTask",
            event: NightEvent::UnknownFileAccessed,
            rules: vec![DecisionRule {
                condition: RuleCondition::UnknownFileTouched,
                action: RuleAction::SkipTask,
                priority: 20,
            }],
            expected_action: Some(RuleAction::SkipTask),
        },
        Scenario {
            label: "low confidence below threshold maps to TagWipAndDefer",
            event: NightEvent::LowConfidenceScore { score: 0.69 },
            rules: vec![DecisionRule {
                condition: RuleCondition::ConfidenceBelow(0.70),
                action: RuleAction::TagWipAndDefer,
                priority: 30,
            }],
            expected_action: Some(RuleAction::TagWipAndDefer),
        },
        Scenario {
            label: "low confidence at threshold does NOT match (strict less-than)",
            event: NightEvent::LowConfidenceScore { score: 0.70 },
            rules: vec![DecisionRule {
                condition: RuleCondition::ConfidenceBelow(0.70),
                action: RuleAction::TagWipAndDefer,
                priority: 30,
            }],
            expected_action: None,
        },
        Scenario {
            label: "external API error matches exact status code 429",
            event: NightEvent::ExternalApiFailure { status_code: 429 },
            rules: vec![DecisionRule {
                condition: RuleCondition::ExternalApiError(429),
                action: RuleAction::Wait {
                    duration: Duration::from_secs(30),
                    then: Box::new(RuleAction::SkipTask),
                },
                priority: 15,
            }],
            expected_action: Some(RuleAction::Wait {
                duration: Duration::from_secs(30),
                then: Box::new(RuleAction::SkipTask),
            }),
        },
        Scenario {
            label: "external API error does NOT match a different status code",
            event: NightEvent::ExternalApiFailure { status_code: 500 },
            rules: vec![DecisionRule {
                condition: RuleCondition::ExternalApiError(429),
                action: RuleAction::SkipTask,
                priority: 15,
            }],
            expected_action: None,
        },
        Scenario {
            label: "empty rule set always returns None",
            event: NightEvent::SecurityIssueDetected,
            rules: vec![],
            expected_action: None,
        },
        Scenario {
            label: "highest-priority rule wins when multiple rules match",
            event: NightEvent::TestFailure { retry_count: 5 },
            rules: vec![
                DecisionRule {
                    condition: RuleCondition::TestFailureAfterRetries(1),
                    action: RuleAction::SkipTask,
                    priority: 10,
                },
                DecisionRule {
                    condition: RuleCondition::TestFailureAfterRetries(2),
                    action: RuleAction::HaltAndDocument,
                    priority: 90,
                },
                DecisionRule {
                    condition: RuleCondition::TestFailureAfterRetries(3),
                    action: RuleAction::TagWipAndDefer,
                    priority: 50,
                },
            ],
            expected_action: Some(RuleAction::HaltAndDocument),
        },
        Scenario {
            label: "unmatched event type in a populated rule set returns None",
            event: NightEvent::UnknownFileAccessed,
            rules: vec![
                DecisionRule {
                    condition: RuleCondition::SecurityIssueFound,
                    action: RuleAction::HaltAndDocument,
                    priority: 100,
                },
                DecisionRule {
                    condition: RuleCondition::MergeConflict,
                    action: RuleAction::TagWipAndDefer,
                    priority: 60,
                },
            ],
            expected_action: None,
        },
        Scenario {
            label: "security issue beats lower-priority cost rule when both present",
            event: NightEvent::SecurityIssueDetected,
            rules: vec![
                DecisionRule {
                    condition: RuleCondition::SecurityIssueFound,
                    action: RuleAction::HaltAndDocument,
                    priority: 100,
                },
                DecisionRule {
                    condition: RuleCondition::CostExceeds(0.0),
                    action: RuleAction::HardStop { notify: false },
                    priority: 5,
                },
            ],
            expected_action: Some(RuleAction::HaltAndDocument),
        },
    ]
}

#[test]
fn decision_rule_outcome_is_100pct_correct() {
    let scenarios = build_scenarios();

    assert_eq!(
        scenarios.len(),
        15,
        "scenario matrix must contain exactly 15 entries"
    );

    let mut passed_count = 0_usize;

    for scenario in &scenarios {
        let actual_action = RuleEngine::resolve(&scenario.event, &scenario.rules);

        assert_eq!(
            actual_action, scenario.expected_action,
            "FAIL scenario '{}': expected {:#?} but got {:#?}",
            scenario.label, scenario.expected_action, actual_action,
        );

        passed_count += 1;
    }

    assert_eq!(
        passed_count,
        scenarios.len(),
        "all {total} scenarios must pass; only {passed_count} passed",
        total = scenarios.len(),
    );
}

#[test]
fn night_decision_rule_p99_meets_10ms_sla() {
    use std::time::{Duration, Instant};

    let rule_set: Vec<DecisionRule> = vec![
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
        DecisionRule {
            condition: RuleCondition::UnknownFileTouched,
            action: RuleAction::SkipTask,
            priority: 20,
        },
    ];

    let event_sequence: Vec<NightEvent> = vec![
        NightEvent::TestFailure { retry_count: 0 },
        NightEvent::TestFailure { retry_count: 1 },
        NightEvent::TestFailure { retry_count: 3 },
        NightEvent::CostThresholdExceeded { total_usd: 5.0 },
        NightEvent::CostThresholdExceeded { total_usd: 25.0 },
        NightEvent::ExternalApiFailure { status_code: 429 },
        NightEvent::ExternalApiFailure { status_code: 500 },
        NightEvent::MergeConflictDetected,
        NightEvent::SecurityIssueDetected,
        NightEvent::LowConfidenceScore { score: 0.95 },
        NightEvent::LowConfidenceScore { score: 0.30 },
        NightEvent::UnknownFileAccessed,
        NightEvent::TestFailure { retry_count: 4 },
        NightEvent::CostThresholdExceeded { total_usd: 19.99 },
        NightEvent::MergeConflictDetected,
        NightEvent::SecurityIssueDetected,
        NightEvent::TestFailure { retry_count: 2 },
        NightEvent::ExternalApiFailure { status_code: 503 },
        NightEvent::UnknownFileAccessed,
        NightEvent::LowConfidenceScore { score: 0.50 },
    ];

    for _warmup_index in 0..20 {
        for event in &event_sequence {
            let _resolved_action = RuleEngine::resolve(event, &rule_set);
        }
    }

    let mut iteration_durations: Vec<Duration> = Vec::new();

    for _ in 0..100 {
        let iteration_start_time = Instant::now();
        for event in &event_sequence {
            let _resolved_action = RuleEngine::resolve(event, &rule_set);
        }
        iteration_durations.push(iteration_start_time.elapsed());
    }

    iteration_durations.sort();

    let p99_duration = iteration_durations
        .get(98)
        .copied()
        .expect("100 iterations must produce 100 duration samples");

    assert!(
        p99_duration.as_millis() < 10,
        "decision rule resolution p99 latency exceeded 10ms target: {p99_duration:?}",
    );
}
