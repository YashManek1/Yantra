//! # forge-router: Routing Tier Selection Accuracy Tests
//!
//! Verifies that `RoutingPolicy::classify` achieves at least 90% accuracy
//! across 10 labeled `(TaskDescription, expected_tier)` examples that exercise
//! every branch of the four-tier routing decision tree.
//!
//! ## Input
//! - 10 labeled `TaskDescription` examples with known correct tiers:
//!   - Tier3: Migration, Integration, and sacred-file NewFeature tasks
//!   - Tier2: multi-file Refactor tasks
//!   - Tier1: large token/tool-call tasks that exceed Tier0 limits
//!   - Tier0: small BugFix and Style tasks within token/tool-call limits
//!
//! ## Output
//! - Assertion that at least 9 of 10 examples are routed to the correct tier
//!   (≥ 90% accuracy)
//!
//! ## Related
//! - `forge-router::routing` — `RoutingPolicy::classify` under test
//! - `forge-core::task`      — defines `TaskClass` and `ModelTier`

use yantra_core::{ModelTier, TaskClass};
use yantra_router::routing::{RoutingPolicy, TaskDescription};

/// A single labeled example pairing a `TaskDescription` with the expected
/// `ModelTier` that the default `RoutingPolicy` should select.
struct LabeledRoutingExample {
    label: &'static str,
    task_description: TaskDescription,
    expected_tier: ModelTier,
}

fn build_labeled_routing_examples() -> Vec<LabeledRoutingExample> {
    vec![
        // --- Tier3 cases ---
        LabeledRoutingExample {
            label: "tier3: Migration task always routes to Tier3",
            task_description: TaskDescription {
                description: "migrate from diesel to sqlx across all database queries".to_owned(),
                class: TaskClass::Migration,
                tokens_estimated: 500,
                tool_calls_predicted: 2,
                touches_sacred_files: false,
                multi_file: false,
            },
            expected_tier: ModelTier::Tier3,
        },
        LabeledRoutingExample {
            label: "tier3: Integration task always routes to Tier3",
            task_description: TaskDescription {
                description: "integrate Stripe payment processing into the checkout flow"
                    .to_owned(),
                class: TaskClass::Integration,
                tokens_estimated: 400,
                tool_calls_predicted: 1,
                touches_sacred_files: false,
                multi_file: false,
            },
            expected_tier: ModelTier::Tier3,
        },
        LabeledRoutingExample {
            label: "tier3: NewFeature touching sacred files routes to Tier3",
            task_description: TaskDescription {
                description: "add JWT rotation to the auth service".to_owned(),
                class: TaskClass::NewFeature,
                tokens_estimated: 600,
                tool_calls_predicted: 2,
                touches_sacred_files: true,
                multi_file: false,
            },
            expected_tier: ModelTier::Tier3,
        },
        // --- Tier2 cases ---
        LabeledRoutingExample {
            label: "tier2: multi-file Refactor routes to Tier2",
            task_description: TaskDescription {
                description: "refactor the database connection pooling across all crates"
                    .to_owned(),
                class: TaskClass::Refactor,
                tokens_estimated: 3_000,
                tool_calls_predicted: 5,
                touches_sacred_files: false,
                multi_file: true,
            },
            expected_tier: ModelTier::Tier2,
        },
        LabeledRoutingExample {
            label: "tier2: large multi-file Refactor with many tool calls routes to Tier2",
            task_description: TaskDescription {
                description: "rename UserService to AccountService across all modules".to_owned(),
                class: TaskClass::Refactor,
                tokens_estimated: 8_000,
                tool_calls_predicted: 10,
                touches_sacred_files: false,
                multi_file: true,
            },
            expected_tier: ModelTier::Tier2,
        },
        // --- Tier1 cases ---
        LabeledRoutingExample {
            label: "tier1: BugFix exceeding token limit routes to Tier1",
            task_description: TaskDescription {
                description: "fix the broken authentication flow across multiple files".to_owned(),
                class: TaskClass::BugFix,
                tokens_estimated: 5_000,
                tool_calls_predicted: 2,
                touches_sacred_files: false,
                multi_file: false,
            },
            expected_tier: ModelTier::Tier1,
        },
        LabeledRoutingExample {
            label: "tier1: NewFeature exceeding tool-call limit routes to Tier1",
            task_description: TaskDescription {
                description: "add pagination to the search results page".to_owned(),
                class: TaskClass::NewFeature,
                tokens_estimated: 1_000,
                tool_calls_predicted: 5,
                touches_sacred_files: false,
                multi_file: false,
            },
            expected_tier: ModelTier::Tier1,
        },
        // --- Tier0 cases ---
        LabeledRoutingExample {
            label: "tier0: small BugFix within token and tool-call limits routes to Tier0",
            task_description: TaskDescription {
                description: "fix null pointer crash in session handler".to_owned(),
                class: TaskClass::BugFix,
                tokens_estimated: 800,
                tool_calls_predicted: 2,
                touches_sacred_files: false,
                multi_file: false,
            },
            expected_tier: ModelTier::Tier0,
        },
        LabeledRoutingExample {
            label: "tier0: Style task within token and tool-call limits routes to Tier0",
            task_description: TaskDescription {
                description: "reformat the codebase with rustfmt".to_owned(),
                class: TaskClass::Style,
                tokens_estimated: 500,
                tool_calls_predicted: 1,
                touches_sacred_files: false,
                multi_file: false,
            },
            expected_tier: ModelTier::Tier0,
        },
        LabeledRoutingExample {
            label: "tier0: Exploration task with tiny token budget routes to Tier0",
            task_description: TaskDescription {
                description: "explore options for caching the auth token".to_owned(),
                class: TaskClass::Exploration,
                tokens_estimated: 200,
                tool_calls_predicted: 0,
                touches_sacred_files: false,
                multi_file: false,
            },
            expected_tier: ModelTier::Tier0,
        },
    ]
}

#[test]
fn routing_tier_selection_meets_90pct_accuracy() {
    let routing_policy = RoutingPolicy::default();
    let labeled_examples = build_labeled_routing_examples();
    let total_example_count = labeled_examples.len();
    let mut correct_tier_count: usize = 0;
    let mut incorrect_examples: Vec<(&str, ModelTier, ModelTier)> = Vec::new();

    for labeled_example in &labeled_examples {
        let selected_tier = routing_policy.classify(&labeled_example.task_description);
        if selected_tier == labeled_example.expected_tier {
            correct_tier_count += 1;
        } else {
            incorrect_examples.push((
                labeled_example.label,
                labeled_example.expected_tier,
                selected_tier,
            ));
        }
    }

    let accuracy_percentage = (correct_tier_count * 100) / total_example_count;

    assert!(
        accuracy_percentage >= 90,
        "Routing tier accuracy {correct_tier_count}/{total_example_count} ({accuracy_percentage}%) \
         is below the 90% SLA target. Wrong routing decisions:\n{}",
        incorrect_examples
            .iter()
            .map(|(label, expected, actual)| format!(
                "  label={label:?} expected={expected:?} actual={actual:?}"
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
