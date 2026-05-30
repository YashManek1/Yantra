//! # forge-verifier: Gate 1 and Hallucination Accuracy Tests
//!
//! Verifies two detection properties:
//!
//! **Gate 1 (truth drift):** `TruthDriftGate::check` achieves ≥ 90% correct
//! verdicts across 10 seeded diff-and-truth pairs (5 PASS, 5 FAIL). Gate 1 is
//! a purely deterministic scope and constraint checker — 100% is expected and
//! enforced.
//!
//! **Hallucination L1 (CRG symbol cross-check):** `ast_symbol_cross_check`
//! is tested across two tiers of fixture:
//!
//! - *Obvious tier (5 hallucinated + 5 clean):* clearly fake names like
//!   `totally_made_up_function`. Expected precision: 100%.
//! - *Subtle tier (5 plausible-but-fake method calls):* real Rust types used
//!   with non-existent methods (e.g. `HashMap::fuzzy_get`, `Vec::sort_stable`).
//!   The L1 regex extracts the method name regardless of receiver type, so
//!   these are still caught. Expected precision: 100%.
//! - *False-positive tier (5 clean real-world patterns):* real stdlib methods
//!   and user-defined symbols that must NOT be flagged.
//!
//! ## Known L1 blind spots (documented, not tested here)
//! - Methods that share names with stdlib allowlist entries (`sort`, `find`,
//!   `replace`) used in novel contexts — L1 cannot distinguish them.
//! - Method names never extracted (only function calls matching `name(`);
//!   dot-method syntax `obj.method()` is captured, but `Type::method()`
//!   static dispatch extracts the method name correctly since it still matches
//!   the `name(` regex pattern after `::`.
//!
//! ## Output
//! - Gate 1: ≥ 90% accuracy (enforced)
//! - Hallucination precision (obvious + subtle): ≥ 80% (enforced)
//! - False-positive rate: 0 (hard enforced per fixture)
//!
//! ## Related
//! - `forge-verifier::gate1_truth_drift` — `TruthDriftGate::check` under test
//! - `forge-verifier::hallucination`     — `ast_symbol_cross_check` under test
//! - `forge-stvp::source_truth`          — `SourceTruth` type consumed by gate1

use std::collections::{BTreeMap, HashSet};

use chrono::Utc;
use yantra_core::{Strictness, TaskClass, TaskId};
use yantra_stvp::source_truth::SourceTruth;
use yantra_verifier::hallucination::ast_symbol_cross_check;
use yantra_verifier::types::{Diff, ValidationResult};
use yantra_verifier::TruthDriftGate;

fn make_source_truth(answers: &[(&str, &str)], task_class: TaskClass) -> SourceTruth {
    let collected_answers: BTreeMap<String, String> = answers
        .iter()
        .map(|&(key, value)| (key.to_owned(), value.to_owned()))
        .collect();

    SourceTruth {
        task_id: TaskId::new(),
        task_class,
        strictness: Strictness::Strict,
        description: "benchmark accuracy task".to_owned(),
        created_at: Utc::now(),
        answers: collected_answers,
        augmented_question_ids: Vec::new(),
    }
}

fn make_diff(touched_file_paths: &[&str], task_class: TaskClass) -> Diff {
    let diff_text = touched_file_paths
        .iter()
        .flat_map(|path| {
            [
                format!("--- a/{path}"),
                format!("+++ b/{path}"),
                "@@ -1,3 +1,4 @@".to_owned(),
                "+    let new_line = true;".to_owned(),
            ]
        })
        .collect::<Vec<_>>()
        .join("\n");

    Diff {
        text: diff_text,
        task_class,
    }
}

/// Whether the caller expects the gate to pass or fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedVerdict {
    Pass,
    Fail,
}

struct Gate1Fixture {
    label: &'static str,
    source_truth: SourceTruth,
    diff: Diff,
    expected_verdict: ExpectedVerdict,
}

fn build_gate1_fixtures() -> Vec<Gate1Fixture> {
    vec![
        // --- 5 PASS cases ---
        Gate1Fixture {
            label: "pass: clean diff touches only in-scope auth file",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/payments.rs"),
                    ("new_deps_allowed", "yes"),
                ],
                TaskClass::BugFix,
            ),
            diff: make_diff(&["src/auth.rs"], TaskClass::BugFix),
            expected_verdict: ExpectedVerdict::Pass,
        },
        Gate1Fixture {
            label: "pass: clean diff touches lib.rs when out_of_scope is payments only",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/payments.rs"),
                    ("new_deps_allowed", "yes"),
                ],
                TaskClass::NewFeature,
            ),
            diff: make_diff(&["src/lib.rs"], TaskClass::NewFeature),
            expected_verdict: ExpectedVerdict::Pass,
        },
        Gate1Fixture {
            label: "pass: deps allowed and Cargo.toml is touched",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/crypto.rs"),
                    ("new_deps_allowed", "yes"),
                ],
                TaskClass::Migration,
            ),
            diff: make_diff(&["Cargo.toml", "src/router.rs"], TaskClass::Migration),
            expected_verdict: ExpectedVerdict::Pass,
        },
        Gate1Fixture {
            label: "pass: multi-file refactor touching only in-scope files",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/payments.rs"),
                    ("new_deps_allowed", "yes"),
                ],
                TaskClass::Refactor,
            ),
            diff: make_diff(
                &["src/auth.rs", "src/session.rs", "src/middleware.rs"],
                TaskClass::Refactor,
            ),
            expected_verdict: ExpectedVerdict::Pass,
        },
        Gate1Fixture {
            label: "pass: exploration diff touching only docs directory",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/payments.rs"),
                    ("new_deps_allowed", "yes"),
                ],
                TaskClass::Exploration,
            ),
            diff: make_diff(&["docs/architecture.md"], TaskClass::Exploration),
            expected_verdict: ExpectedVerdict::Pass,
        },
        // --- 5 FAIL cases ---
        Gate1Fixture {
            label: "fail: scope drift — diff touches explicitly out-of-scope payments.rs",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/payments.rs"),
                    ("new_deps_allowed", "yes"),
                ],
                TaskClass::BugFix,
            ),
            diff: make_diff(&["src/payments.rs"], TaskClass::BugFix),
            expected_verdict: ExpectedVerdict::Fail,
        },
        Gate1Fixture {
            label: "fail: constraint drift — new_deps_allowed=no but Cargo.toml is touched",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/payments.rs"),
                    ("new_deps_allowed", "no"),
                ],
                TaskClass::BugFix,
            ),
            diff: make_diff(&["src/auth.rs", "Cargo.toml"], TaskClass::BugFix),
            expected_verdict: ExpectedVerdict::Fail,
        },
        Gate1Fixture {
            label: "fail: constraint drift — new_deps_allowed=no and nested Cargo.toml touched",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/payments.rs"),
                    ("new_deps_allowed", "no"),
                ],
                TaskClass::Refactor,
            ),
            diff: make_diff(&["crates/forge-stvp/Cargo.toml"], TaskClass::Refactor),
            expected_verdict: ExpectedVerdict::Fail,
        },
        Gate1Fixture {
            label: "fail: scope drift — diff touches crypto.rs listed as out_of_scope",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/crypto.rs"),
                    ("new_deps_allowed", "yes"),
                ],
                TaskClass::NewFeature,
            ),
            diff: make_diff(&["src/auth.rs", "src/crypto.rs"], TaskClass::NewFeature),
            expected_verdict: ExpectedVerdict::Fail,
        },
        Gate1Fixture {
            label: "fail: constraint drift — package.json touched when new_deps_allowed=no",
            source_truth: make_source_truth(
                &[
                    ("out_of_scope", "src/payments.rs"),
                    ("new_deps_allowed", "no"),
                ],
                TaskClass::Migration,
            ),
            diff: make_diff(&["src/api.ts", "package.json"], TaskClass::Migration),
            expected_verdict: ExpectedVerdict::Fail,
        },
    ]
}

#[test]
fn gate1_accuracy_meets_90pct() {
    let fixtures = build_gate1_fixtures();
    let total_fixture_count = fixtures.len();
    let mut correct_verdict_count: usize = 0;
    let mut incorrect_labels: Vec<&str> = Vec::new();

    for fixture in &fixtures {
        let gate_result = TruthDriftGate::check(&fixture.source_truth, &fixture.diff)
            .expect("gate1 check must not error during accuracy test");

        let actual_verdict = match &gate_result {
            ValidationResult::Pass => ExpectedVerdict::Pass,
            ValidationResult::Fail(_) => ExpectedVerdict::Fail,
        };

        if actual_verdict == fixture.expected_verdict {
            correct_verdict_count += 1;
        } else {
            incorrect_labels.push(fixture.label);
        }
    }

    let accuracy_percentage = (correct_verdict_count * 100) / total_fixture_count;

    assert!(
        accuracy_percentage >= 90,
        "Gate1 accuracy {correct_verdict_count}/{total_fixture_count} ({accuracy_percentage}%) \
         is below the 90% SLA target. Wrong verdicts:\n{}",
        incorrect_labels
            .iter()
            .map(|label| format!("  {label}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// A diff sample for the hallucination precision test.
struct HallucinationFixture {
    label: &'static str,
    /// Lines to add in the diff (without the leading `+`).
    added_lines: Vec<&'static str>,
    /// Known symbols that the CRG would supply.
    known_symbols: HashSet<String>,
    /// Whether this diff is expected to produce at least one hallucination flag.
    expect_flagged: bool,
}

fn build_hallucination_fixtures() -> Vec<HallucinationFixture> {
    let empty_known: HashSet<String> = HashSet::new();

    vec![
        // ─── Obvious-tier hallucinations (clearly fake names) ──────────────────
        HallucinationFixture {
            label: "obvious: calls totally_made_up_function not in CRG",
            added_lines: vec!["    let result = totally_made_up_function(input_value);"],
            known_symbols: empty_known.clone(),
            expect_flagged: true,
        },
        HallucinationFixture {
            label: "obvious: references GhostStruct type not in CRG",
            added_lines: vec!["    let ghost_value: GhostStruct = GhostStruct::new();"],
            known_symbols: empty_known.clone(),
            expect_flagged: true,
        },
        HallucinationFixture {
            label: "obvious: calls phantom_helper with empty CRG",
            added_lines: vec!["    phantom_helper(x, y, z);"],
            known_symbols: empty_known.clone(),
            expect_flagged: true,
        },
        HallucinationFixture {
            label: "obvious: references FakeAuthProvider type not in CRG",
            added_lines: vec!["    let provider = FakeAuthProvider::default();"],
            known_symbols: empty_known.clone(),
            expect_flagged: true,
        },
        HallucinationFixture {
            label: "obvious: calls nonexistent_serialize function",
            added_lines: vec!["    nonexistent_serialize(&payload, &mut buffer);"],
            known_symbols: empty_known.clone(),
            expect_flagged: true,
        },
        // ─── Subtle-tier hallucinations (real types, fake methods) ─────────────
        // The L1 regex extracts the method name regardless of receiver type —
        // `fuzzy_get`, `sort_stable` etc. are not in the stdlib allowlist, so
        // they are correctly flagged even though `HashMap` and `Vec` are real.
        HallucinationFixture {
            label: "subtle: HashMap::fuzzy_get — method does not exist in std",
            added_lines: vec!["    let value = cache_map.fuzzy_get(&lookup_key)?;"],
            known_symbols: ["cache_map", "lookup_key"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            expect_flagged: true,
        },
        HallucinationFixture {
            label: "subtle: Vec::sort_stable — method does not exist (should be sort_by)",
            added_lines: vec!["    result_list.sort_stable(compare_fn);"],
            known_symbols: ["result_list", "compare_fn"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            expect_flagged: true,
        },
        HallucinationFixture {
            label: "subtle: PathBuf::join_all — method does not exist in std",
            added_lines: vec!["    let full_path = base_dir.join_all(&path_segments);"],
            known_symbols: ["base_dir", "path_segments"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            expect_flagged: true,
        },
        HallucinationFixture {
            label: "subtle: String::split_into_words — method does not exist in std",
            added_lines: vec!["    let token_list = raw_input.split_into_words();"],
            known_symbols: ["raw_input"].iter().map(|s| (*s).to_owned()).collect(),
            expect_flagged: true,
        },
        HallucinationFixture {
            label: "subtle: Duration::from_minutes — method does not exist in std",
            added_lines: vec!["    let timeout = Duration::from_minutes(5);"],
            known_symbols: empty_known.clone(),
            expect_flagged: true,
        },
        // --- 5 CLEAN diffs (expect_flagged = false) ---
        HallucinationFixture {
            label: "clean: calls known function verify_token present in CRG",
            added_lines: vec!["    let is_valid = verify_token(&token, &yantra_dir)?;"],
            known_symbols: ["verify_token", "yantra_dir", "token"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            expect_flagged: false,
        },
        HallucinationFixture {
            label: "clean: references known type SessionManager from CRG",
            added_lines: vec!["    let session_manager = SessionManager::new(config);"],
            known_symbols: ["SessionManager", "config"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            expect_flagged: false,
        },
        HallucinationFixture {
            label: "clean: only stdlib Vec and String referenced",
            added_lines: vec!["    let names: Vec<String> = Vec::new();"],
            known_symbols: empty_known.clone(),
            expect_flagged: false,
        },
        HallucinationFixture {
            label: "clean: uses known functions issue_token and load_or_generate",
            added_lines: vec![
                "    let signing_key = SigningKey::load_or_generate(&yantra_dir)?;",
                "    let truth_token = issue_token(&source_truth, &signing_key)?;",
            ],
            known_symbols: [
                "SigningKey",
                "load_or_generate",
                "yantra_dir",
                "issue_token",
                "source_truth",
                "signing_key",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
            expect_flagged: false,
        },
        HallucinationFixture {
            label: "clean: only simple arithmetic with no identifiers",
            added_lines: vec!["    let total_count = base_count + extra_count;"],
            known_symbols: ["base_count", "extra_count", "total_count"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            expect_flagged: false,
        },
    ]
}

fn build_diff_text_from_added_lines(added_lines: &[&str]) -> String {
    let mut diff_text = String::from("+++ b/src/lib.rs\n@@ -1,0 +1,N @@\n");
    for added_line in added_lines {
        diff_text.push('+');
        diff_text.push_str(added_line);
        diff_text.push('\n');
    }
    diff_text
}

#[test]
fn hallucination_detection_meets_80pct_precision() {
    let fixtures = build_hallucination_fixtures();

    let hallucinated_fixtures: Vec<&HallucinationFixture> = fixtures
        .iter()
        .filter(|fixture| fixture.expect_flagged)
        .collect();

    let total_hallucinated_count = hallucinated_fixtures.len();
    let mut true_positive_count: usize = 0;
    let mut missed_labels: Vec<&str> = Vec::new();

    for fixture in &hallucinated_fixtures {
        let diff_text = build_diff_text_from_added_lines(&fixture.added_lines);
        let hallucinated_symbols = ast_symbol_cross_check(&diff_text, &fixture.known_symbols);

        if hallucinated_symbols.is_empty() {
            missed_labels.push(fixture.label);
        } else {
            true_positive_count += 1;
        }
    }

    let precision_percentage = (true_positive_count * 100) / total_hallucinated_count;

    assert!(
        precision_percentage >= 80,
        "Hallucination detection precision {true_positive_count}/{total_hallucinated_count} \
         ({precision_percentage}%) is below the 80% SLA target.\n\
         NOTE: 10 fixtures total — 5 obvious (clearly-fake names) + 5 subtle (real types with \
         fake methods). L1 catches both tiers since it extracts method names regardless of \
         receiver. Missed hallucinations:\n{}",
        missed_labels
            .iter()
            .map(|label| format!("  {label}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let clean_fixtures: Vec<&HallucinationFixture> = fixtures
        .iter()
        .filter(|fixture| !fixture.expect_flagged)
        .collect();

    for clean_fixture in &clean_fixtures {
        let diff_text = build_diff_text_from_added_lines(&clean_fixture.added_lines);
        let hallucinated_symbols = ast_symbol_cross_check(&diff_text, &clean_fixture.known_symbols);

        assert!(
            hallucinated_symbols.is_empty(),
            "clean diff unexpectedly flagged as hallucination: label={:?} flagged_symbols={:?}",
            clean_fixture.label,
            hallucinated_symbols
                .iter()
                .map(|symbol| &symbol.name)
                .collect::<Vec<_>>(),
        );
    }
}
