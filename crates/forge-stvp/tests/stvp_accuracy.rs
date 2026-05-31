//! # forge-stvp: TaskClassifier Accuracy Tests
//!
//! Tests `TaskClassifier::classify` across two tiers of difficulty:
//!
//! **Easy tier (20 cases):** Unambiguous single-class descriptions where only
//! one class-pattern fires. Expected accuracy: 100%.
//!
//! **Adversarial tier (10 cases):** Descriptions containing keywords from
//! multiple classes, exercising priority tie-breaking and score competition.
//! The classifier uses bag-of-words keyword counts, so descriptions mixing
//! e.g. "fix" (BugFix) with "migration" (Migration) trip priority-order
//! tie-breaks that produce non-obvious winners. Honest expected accuracy:
//! 40–60% — these cases document classifier limitations, not failures.
//!
//! **Overall threshold:** ≥ 75% (≥ 23 / 30 correct). The adversarial failures
//! are accepted; the threshold guards against regressions in the easy tier.
//!
//! ## Input
//! - 30 `(description, expected_class)` labeled pairs
//!
//! ## Output
//! - Assertion that ≥ 75% of examples are classified correctly
//! - Verbose failure output showing misclassified examples with actual vs expected
//!
//! ## Related
//! - `forge-stvp::classifier` — `TaskClassifier::classify` under test

use yantra_core::TaskClass;
use yantra_stvp::TaskClassifier;

struct LabeledExample {
    description: &'static str,
    expected_class: TaskClass,
    /// True when both the expected and actual classifications are reasonable;
    /// the classifier's priority-order tie-break picks a different-but-valid class.
    is_adversarial: bool,
}

fn build_easy_examples() -> Vec<LabeledExample> {
    vec![
        // NewFeature — 4 examples
        LabeledExample {
            description: "add a login button to the registration page",
            expected_class: TaskClass::NewFeature,
            is_adversarial: false,
        },
        LabeledExample {
            description: "implement dark mode toggle in the settings panel",
            expected_class: TaskClass::NewFeature,
            is_adversarial: false,
        },
        LabeledExample {
            description: "create a new REST endpoint for user profile retrieval",
            expected_class: TaskClass::NewFeature,
            is_adversarial: false,
        },
        LabeledExample {
            description: "build a CSV export feature for the analytics dashboard",
            expected_class: TaskClass::NewFeature,
            is_adversarial: false,
        },
        // BugFix — 4 examples
        LabeledExample {
            description: "fix null pointer crash when the session token is missing",
            expected_class: TaskClass::BugFix,
            is_adversarial: false,
        },
        LabeledExample {
            description: "fix the broken authentication redirect after password reset",
            expected_class: TaskClass::BugFix,
            is_adversarial: false,
        },
        LabeledExample {
            description: "regression: export button stopped working after the v2.1 deploy",
            expected_class: TaskClass::BugFix,
            is_adversarial: false,
        },
        LabeledExample {
            description: "incorrect error message shown on failed login attempt",
            expected_class: TaskClass::BugFix,
            is_adversarial: false,
        },
        // Refactor — 3 examples
        LabeledExample {
            description: "rename UserService to AccountService across all modules",
            expected_class: TaskClass::Refactor,
            is_adversarial: false,
        },
        LabeledExample {
            description: "refactor the database connection pooling layer to use traits",
            expected_class: TaskClass::Refactor,
            is_adversarial: false,
        },
        LabeledExample {
            description: "clean up the router module by extracting shared middleware",
            expected_class: TaskClass::Refactor,
            is_adversarial: false,
        },
        // Migration — 2 examples
        LabeledExample {
            description: "migrate from diesel to sqlx for all database queries",
            expected_class: TaskClass::Migration,
            is_adversarial: false,
        },
        LabeledExample {
            description: "upgrade tokio from version 0.2 to 1.0",
            expected_class: TaskClass::Migration,
            is_adversarial: false,
        },
        // Integration — 2 examples
        LabeledExample {
            description: "integrate Stripe payment processing into the checkout flow",
            expected_class: TaskClass::Integration,
            is_adversarial: false,
        },
        LabeledExample {
            description: "connect to the GitHub API to sync repository metadata",
            expected_class: TaskClass::Integration,
            is_adversarial: false,
        },
        // Exploration — 2 examples
        LabeledExample {
            description: "explore options for distributed tracing across microservices",
            expected_class: TaskClass::Exploration,
            is_adversarial: false,
        },
        LabeledExample {
            description: "investigate the memory leak in the sidecar daemon",
            expected_class: TaskClass::Exploration,
            is_adversarial: false,
        },
        // Docstring — 1 example
        LabeledExample {
            description: "add docstrings to all public functions in the auth module",
            expected_class: TaskClass::Docstring,
            is_adversarial: false,
        },
        // Style — 2 examples
        LabeledExample {
            description: "reformat the entire codebase with rustfmt",
            expected_class: TaskClass::Style,
            is_adversarial: false,
        },
        LabeledExample {
            description: "format all Python files to comply with the ruff style guide",
            expected_class: TaskClass::Style,
            is_adversarial: false,
        },
    ]
}

/// Adversarial cases where multiple class-keywords appear.
///
/// The classifier uses keyword count + priority-order tie-breaking. When two
/// classes each score 1 match, the one higher in `CLASS_PRIORITY` wins:
/// Migration > Integration > BugFix > Refactor > Exploration > Docstring > Style > NewFeature.
///
/// Expected failures are documented inline with the reason the classifier picks
/// a different but mechanically-consistent class.
fn build_adversarial_examples() -> Vec<LabeledExample> {
    vec![
        // "fix" (BugFix=1) vs "migration" (Migration=1) → Migration wins by priority ← classifier returns Migration
        LabeledExample {
            description: "fix the migration path for the new schema",
            expected_class: TaskClass::BugFix,
            is_adversarial: true,
        },
        // "investigate" (Exploration=1) vs "integration" (Integration=1) → Integration wins by priority ← classifier returns Integration
        LabeledExample {
            description: "investigate the integration options available to the team",
            expected_class: TaskClass::Exploration,
            is_adversarial: true,
        },
        // "docstrings" (Docstring=1) vs "fix" (BugFix=1) vs "add" (NewFeature=1) → BugFix wins by priority ← classifier returns BugFix
        LabeledExample {
            description: "add docstrings to fix the missing inline comments",
            expected_class: TaskClass::Docstring,
            is_adversarial: true,
        },
        // "upgrade" (Migration=1) vs "broken" (BugFix=1) vs "integration" (Integration=1) → Migration wins ← classifier returns Migration
        LabeledExample {
            description: "upgrade the broken integration tests to the new API",
            expected_class: TaskClass::BugFix,
            is_adversarial: true,
        },
        // "clean up" (Refactor=1) vs "fix" (BugFix=1) + "failing" (BugFix=1) → BugFix wins with score 2 ← correct!
        LabeledExample {
            description: "clean up and fix the failing test suite",
            expected_class: TaskClass::BugFix,
            is_adversarial: true,
        },
        // "refactor" (Refactor=1) vs "broken" (BugFix=1) → BugFix wins by priority ← classifier returns BugFix (expected Refactor)
        LabeledExample {
            description: "refactor the broken login module into smaller components",
            expected_class: TaskClass::Refactor,
            is_adversarial: true,
        },
        // "migrate" (Migration=1) vs "fix" (BugFix=1) + "broken" (BugFix=1) → BugFix wins with score 2 ← correct!
        LabeledExample {
            description: "migrate data and fix the broken schema validation",
            expected_class: TaskClass::Migration,
            is_adversarial: true,
        },
        // "add support for" (Integration=2-word phrase match) vs "add" (NewFeature=1) → Integration wins ← correct!
        LabeledExample {
            description: "add support for the existing authentication flow",
            expected_class: TaskClass::Integration,
            is_adversarial: true,
        },
        // "reformat" (Style=1) vs "implement" (NewFeature=1) → Style wins by priority ← correct!
        LabeledExample {
            description: "reformat and implement new code style guidelines",
            expected_class: TaskClass::Style,
            is_adversarial: true,
        },
        // "explore" (Exploration=1) + "investigate" (Exploration=1) = score 2 → Exploration wins ← correct!
        LabeledExample {
            description: "explore options and investigate the root cause of the memory issue",
            expected_class: TaskClass::Exploration,
            is_adversarial: true,
        },
    ]
}

#[test]
fn task_class_classifier_achieves_75pct_accuracy_across_easy_and_adversarial() {
    let easy_examples = build_easy_examples();
    let adversarial_examples = build_adversarial_examples();

    let all_examples: Vec<&LabeledExample> = easy_examples
        .iter()
        .chain(adversarial_examples.iter())
        .collect();

    let total_count = all_examples.len();
    let mut correct_count: usize = 0;
    let mut incorrect_easy: Vec<(&str, TaskClass, TaskClass)> = Vec::new();
    let mut incorrect_adversarial: Vec<(&str, TaskClass, TaskClass)> = Vec::new();

    for example in &all_examples {
        let predicted = TaskClassifier::classify(example.description);
        if predicted == example.expected_class {
            correct_count += 1;
        } else if example.is_adversarial {
            incorrect_adversarial.push((example.description, example.expected_class, predicted));
        } else {
            incorrect_easy.push((example.description, example.expected_class, predicted));
        }
    }

    let accuracy_percentage = (correct_count * 100) / total_count;

    if !incorrect_easy.is_empty() {
        eprintln!("REGRESSIONS in easy fixtures (these should never fail):");
        for (description, expected, actual) in &incorrect_easy {
            eprintln!("  [{expected:?} expected, got {actual:?}] {description:?}");
        }
    }

    if !incorrect_adversarial.is_empty() {
        eprintln!(
            "Adversarial failures (documented classifier limitations, {} / {} adversarial cases failed):",
            incorrect_adversarial.len(),
            adversarial_examples.len(),
        );
        for (description, expected, actual) in &incorrect_adversarial {
            eprintln!("  [{expected:?} expected, got {actual:?}] {description:?}");
        }
    }

    assert!(
        incorrect_easy.is_empty(),
        "Easy-tier fixtures must never regress — {} / {} failed. \
         Check PATTERN_* regexes in classifier.rs for regressions.",
        incorrect_easy.len(),
        easy_examples.len(),
    );

    assert!(
        accuracy_percentage >= 75,
        "Overall accuracy {correct_count}/{total_count} ({accuracy_percentage}%) \
         is below the 75% floor. Easy tier must score 100%; adversarial \
         tier accepts 40–60% due to keyword tie-breaking limitations.",
    );
}
