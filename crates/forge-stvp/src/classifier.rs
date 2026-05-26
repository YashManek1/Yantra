//! # forge-stvp: Task Classifier
//!
//! Maps a natural-language task description to a `TaskClass` using keyword and
//! regex matching. Each class has a compiled pattern; all patterns are scored
//! against the description and the highest-scoring class wins. Ties are broken
//! by an explicit priority order so that specific classes (Migration, Integration)
//! outrank generic ones (NewFeature).
//!
//! ## Input
//! - `description: &str` — natural-language task description from the user
//!
//! ## Output
//! - `TaskClass` — the most likely class for the description
//!
//! ## Related
//! - `forge-core::task` — defines the `TaskClass` enum
//! - `forge-stvp::interrogator` — uses the classifier to select questionnaire templates

use std::sync::LazyLock;

use regex::Regex;
use yantra_core::TaskClass;

static PATTERN_BUG_FIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(fix(es|ed|ing)?|bugs?|broken|regressions?|crash(es|ed|ing)?|incorrect|fails?|failed|failing|wrong|errors?)\b",
    )
    .expect("BugFix pattern is valid regex")
});

static PATTERN_REFACTOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(refactor(s|ed|ing)?|clean(s|ed|ing)?(\s+up)?|rename(s|d)?|restructur(e|es|ed|ing)|reorgani[zs]e(s|d|ing)?)\b",
    )
    .expect("Refactor pattern is valid regex")
});

static PATTERN_MIGRATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(migrat(e|es|ed|ing)|upgrad(e|es|ed|ing)|switch\s+from|port(s|ed|ing)?\s+from|mov(e|es|ed|ing)\s+from)\b",
    )
    .expect("Migration pattern is valid regex")
});

static PATTERN_INTEGRATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(integrat(e|es|ed|ing)|connect\s+to|add\s+support\s+for)\b")
        .expect("Integration pattern is valid regex")
});

static PATTERN_EXPLORATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(explor(e|es|ed|ing)|investigat(e|es|ed|ing)|understand(s|ing)?|research(es|ed|ing)?)\b",
    )
    .expect("Exploration pattern is valid regex")
});

static PATTERN_DOCSTRING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(docstring(s)?|documentation|document(s|ed|ing)?|comment(s|ed|ing)?)\b")
        .expect("Docstring pattern is valid regex")
});

static PATTERN_STYLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(reformat(s|ted|ting)?|format(s|ted|ting)?|style(s|d|ing)?)\b")
        .expect("Style pattern is valid regex")
});

static PATTERN_NEW_FEATURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(add(s|ed|ing)?|implement(s|ed|ing)?|creat(e|es|ed|ing)|build(s|ing|t)?|writ(e|es|ing)|wrote|develop(s|ed|ing)?)\b",
    )
    .expect("NewFeature pattern is valid regex")
});

/// Priority-ordered list of `(TaskClass, pattern)` pairs.
///
/// For equal match counts, the class appearing earlier in this list wins.
/// Migration and Integration are most specific and therefore rank highest.
const CLASS_PRIORITY: &[TaskClass] = &[
    TaskClass::Migration,
    TaskClass::Integration,
    TaskClass::BugFix,
    TaskClass::Refactor,
    TaskClass::Exploration,
    TaskClass::Docstring,
    TaskClass::Style,
    TaskClass::NewFeature,
];

fn pattern_for_class(task_class: TaskClass) -> &'static Regex {
    match task_class {
        TaskClass::Migration => &PATTERN_MIGRATION,
        TaskClass::Integration => &PATTERN_INTEGRATION,
        TaskClass::BugFix => &PATTERN_BUG_FIX,
        TaskClass::Refactor => &PATTERN_REFACTOR,
        TaskClass::Exploration => &PATTERN_EXPLORATION,
        TaskClass::Docstring => &PATTERN_DOCSTRING,
        TaskClass::Style => &PATTERN_STYLE,
        TaskClass::NewFeature => &PATTERN_NEW_FEATURE,
        TaskClass::Chore => &PATTERN_STYLE,
    }
}

/// Counts the number of regex matches in `text` for a given class pattern.
fn match_count(task_class: TaskClass, text: &str) -> usize {
    pattern_for_class(task_class).find_iter(text).count()
}

/// Classifier that maps task descriptions to `TaskClass` values.
pub struct TaskClassifier;

impl TaskClassifier {
    /// Classifies `description` using keyword and regex matching.
    ///
    /// Returns the highest-scoring `TaskClass`. When multiple classes tie, the
    /// one with higher priority in `CLASS_PRIORITY` wins. Defaults to
    /// `TaskClass::NewFeature` when no patterns match.
    pub fn classify(description: &str) -> TaskClass {
        let mut best_class = TaskClass::NewFeature;
        let mut best_score: usize = 0;

        for &candidate_class in CLASS_PRIORITY {
            let candidate_score = match_count(candidate_class, description);
            if candidate_score > best_score {
                best_score = candidate_score;
                best_class = candidate_class;
            }
        }

        best_class
    }

    /// Stub for a future model-assisted fallback when rule confidence is low.
    ///
    /// Not implemented for the hackathon; delegates to `classify`.
    pub fn classify_with_model_fallback(description: &str) -> TaskClass {
        Self::classify(description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_maps_30_fixtures() {
        let fixtures: &[(&str, TaskClass)] = &[
            // NewFeature (8)
            ("add JWT rotation to auth service", TaskClass::NewFeature),
            ("implement dark mode toggle", TaskClass::NewFeature),
            (
                "create a new endpoint for user profiles",
                TaskClass::NewFeature,
            ),
            ("build a CSV export feature", TaskClass::NewFeature),
            (
                "add pagination to the search results",
                TaskClass::NewFeature,
            ),
            ("implement rate limiting on the API", TaskClass::NewFeature),
            (
                "write a caching layer for expensive queries",
                TaskClass::NewFeature,
            ),
            ("develop a notification system", TaskClass::NewFeature),
            // BugFix (8)
            ("fix the null pointer exception in login", TaskClass::BugFix),
            ("bug: users can't reset their password", TaskClass::BugFix),
            ("the registration form is broken", TaskClass::BugFix),
            (
                "regression: export button stopped working after v2.1",
                TaskClass::BugFix,
            ),
            ("fix crash when opening settings", TaskClass::BugFix),
            (
                "incorrect error message shown on failed login",
                TaskClass::BugFix,
            ),
            (
                "the search fails when query contains special chars",
                TaskClass::BugFix,
            ),
            ("fix the broken authentication flow", TaskClass::BugFix),
            // Refactor (4)
            (
                "refactor the database connection pooling",
                TaskClass::Refactor,
            ),
            ("clean up the router module", TaskClass::Refactor),
            ("rename UserService to AccountService", TaskClass::Refactor),
            (
                "refactor auth middleware to use traits",
                TaskClass::Refactor,
            ),
            // Migration (3)
            ("migrate from diesel to sqlx", TaskClass::Migration),
            ("upgrade tokio from 0.2 to 1.0", TaskClass::Migration),
            (
                "switch from redis to postgres for session storage",
                TaskClass::Migration,
            ),
            // Integration (3)
            (
                "integrate Stripe payment processing",
                TaskClass::Integration,
            ),
            (
                "connect to the GitHub API for repository sync",
                TaskClass::Integration,
            ),
            ("add support for Google OAuth", TaskClass::Integration),
            // Exploration (2)
            (
                "explore options for distributed tracing",
                TaskClass::Exploration,
            ),
            (
                "investigate the memory leak in the sidecar",
                TaskClass::Exploration,
            ),
            // Docstring (1) + Style (1)
            (
                "add docstrings to all public functions",
                TaskClass::Docstring,
            ),
            ("format the codebase with rustfmt", TaskClass::Style),
        ];

        for &(description, expected_class) in fixtures {
            let actual_class = TaskClassifier::classify(description);
            assert_eq!(
                actual_class, expected_class,
                "wrong class for: {description:?}"
            );
        }
    }

    #[test]
    fn classifier_defaults_to_new_feature_when_no_keywords_match() {
        let unknown_description = "something entirely unprecedented with no recognized verbs";
        assert_eq!(
            TaskClassifier::classify(unknown_description),
            TaskClass::NewFeature
        );
    }

    #[test]
    fn classifier_is_case_insensitive() {
        assert_eq!(TaskClassifier::classify("FIX the login"), TaskClass::BugFix);
        assert_eq!(
            TaskClassifier::classify("MIGRATE data schema"),
            TaskClass::Migration
        );
    }

    #[test]
    fn model_fallback_stub_delegates_to_rule_classifier() {
        assert_eq!(
            TaskClassifier::classify_with_model_fallback("fix the crash"),
            TaskClass::BugFix
        );
    }
}
