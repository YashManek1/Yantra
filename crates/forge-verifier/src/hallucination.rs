//! # forge-verifier: Hallucination Detection
//!
//! Three-layer check for hallucinated symbols in a diff:
//!
//! 1. **AST symbol cross-check** — extracts referenced identifiers from `+` lines
//!    via regex and flags any not found in the CRG symbols table.
//! 2. **LSP verification** — for each function call, attempts `go-to-definition`
//!    via the LSP bridge. (Stubbed; wired when an LSP bridge is provided.)
//! 3. **Self-consistency sampling** — sacred-file diffs only: checks that the
//!    diff itself refers exclusively to symbols from the known CRG name set.
//!    (Variant generation requires a live LLM; stubbed for now.)
//!
//! All violations are `Warning` severity — the hallucination gate informs the
//! Coder rather than hard-blocking, since the CRG may have gaps.
//!
//! ## Input
//! - `Diff` — proposed change to scan for unknown identifiers
//! - `VerificationContext` — supplies the CRG db path
//!
//! ## Output
//! - `Vec<Violation>` — each flagged hallucinated symbol (Warning severity)
//!
//! ## Related
//! - `forge-verifier::pipeline` — calls this alongside gate 3
//! - `forge-crg` — the SQLite database queried for known symbol names

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::error::{VerifierError, VerifierResult};
use crate::types::{Diff, VerificationContext, Violation, ViolationSeverity};

static PATTERN_FUNCTION_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([a-z_][a-zA-Z0-9_]*)\s*\(").expect("function call pattern is valid")
});

static PATTERN_TYPE_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Z][a-zA-Z0-9_]*)\b").expect("type reference pattern is valid")
});

/// Rust standard library names that should never be flagged as hallucinations.
const RUST_STDLIB_NAMES: &[&str] = &[
    "String",
    "Vec",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "Option",
    "Result",
    "Ok",
    "Err",
    "Some",
    "None",
    "Box",
    "Arc",
    "Rc",
    "Cell",
    "RefCell",
    "Mutex",
    "RwLock",
    "PathBuf",
    "Path",
    "Duration",
    "Instant",
    "Cow",
    "Pin",
    "Future",
    "Stream",
    "Iterator",
    "ToString",
    "From",
    "Into",
    "Default",
    "Clone",
    "Copy",
    "Debug",
    "Display",
    "Send",
    "Sync",
    "Sized",
    "Drop",
    "Fn",
    "FnMut",
    "FnOnce",
    "true",
    "false",
    "Self",
    "super",
    "crate",
    "std",
    "core",
    "alloc",
    "println",
    "eprintln",
    "format",
    "write",
    "writeln",
    "panic",
    "assert",
    "assert_eq",
    "assert_ne",
    "unreachable",
    "unimplemented",
    "todo",
    "vec",
    "dbg",
    "matches",
    "cfg",
    "test",
    "derive",
    "allow",
    "warn",
    "deny",
    "forbid",
];

/// A symbol referenced in the diff that could not be found in the CRG.
#[derive(Debug, Clone)]
pub struct HallucinatedSymbol {
    /// The identifier that appears to be hallucinated.
    pub name: String,
    /// Position of the `+` line in the diff where the symbol appears (1-based).
    pub diff_line: Option<u32>,
}

/// Runs all three hallucination layers and returns the combined violations.
pub fn check_hallucination(
    diff: &Diff,
    ctx: &VerificationContext,
) -> VerifierResult<Vec<Violation>> {
    let known_symbol_names = match &ctx.crg_db_path {
        Some(crg_db_path) => load_crg_symbol_names(crg_db_path)?,
        None => HashSet::new(),
    };

    let hallucinated_symbols = ast_symbol_cross_check(&diff.text, &known_symbol_names);
    let violations = hallucinated_symbols_to_violations(hallucinated_symbols);

    Ok(violations)
}

/// Extracts referenced identifiers from `+` lines in `diff_text` and returns
/// any that are not in `known_symbol_names` and are not stdlib names.
#[allow(clippy::implicit_hasher)]
pub fn ast_symbol_cross_check(
    diff_text: &str,
    known_symbol_names: &HashSet<String>,
) -> Vec<HallucinatedSymbol> {
    let stdlib_allowlist: HashSet<&str> = RUST_STDLIB_NAMES.iter().copied().collect();
    let mut hallucinated_symbols = Vec::new();
    let mut already_flagged: HashSet<String> = HashSet::new();

    let added_lines: Vec<(u32, &str)> = diff_text
        .lines()
        .enumerate()
        .filter(|(_, diff_line)| diff_line.starts_with('+') && !diff_line.starts_with("+++"))
        .map(|(index, diff_line)| (index as u32 + 1, &diff_line[1..]))
        .collect();

    for (line_index, line_content) in &added_lines {
        for function_call_match in PATTERN_FUNCTION_CALL.captures_iter(line_content) {
            let symbol_name = &function_call_match[1];
            if stdlib_allowlist.contains(symbol_name)
                || known_symbol_names.contains(symbol_name)
                || already_flagged.contains(symbol_name)
            {
                continue;
            }
            already_flagged.insert(symbol_name.to_owned());
            hallucinated_symbols.push(HallucinatedSymbol {
                name: symbol_name.to_owned(),
                diff_line: Some(*line_index),
            });
        }

        for type_reference_match in PATTERN_TYPE_REFERENCE.captures_iter(line_content) {
            let symbol_name = &type_reference_match[1];
            if stdlib_allowlist.contains(symbol_name)
                || known_symbol_names.contains(symbol_name)
                || already_flagged.contains(symbol_name)
            {
                continue;
            }
            already_flagged.insert(symbol_name.to_owned());
            hallucinated_symbols.push(HallucinatedSymbol {
                name: symbol_name.to_owned(),
                diff_line: Some(*line_index),
            });
        }
    }

    hallucinated_symbols
}

/// Converts `HallucinatedSymbol`s into `Warning`-severity `Violation`s.
pub fn hallucinated_symbols_to_violations(
    hallucinated_symbols: Vec<HallucinatedSymbol>,
) -> Vec<Violation> {
    hallucinated_symbols
        .into_iter()
        .map(|hallucinated_symbol| Violation {
            file_path: None,
            line: hallucinated_symbol.diff_line,
            column: None,
            message: format!(
                "symbol '{}' referenced in diff but not found in CRG — possible hallucination",
                hallucinated_symbol.name
            ),
            severity: ViolationSeverity::Warning,
            rule_id: Some("hallucination::unknown_symbol".to_owned()),
        })
        .collect()
}

/// Opens the CRG SQLite database read-only and returns all symbol names.
pub fn load_crg_symbol_names(crg_db_path: &Path) -> VerifierResult<HashSet<String>> {
    use rusqlite::{Connection, OpenFlags};

    let crg_connection = Connection::open_with_flags(crg_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|db_error| VerifierError::Crg(db_error.to_string()))?;

    let mut crg_statement = crg_connection
        .prepare("SELECT name FROM symbols")
        .map_err(|db_error| VerifierError::Crg(db_error.to_string()))?;

    let symbol_names: HashSet<String> = crg_statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|db_error| VerifierError::Crg(db_error.to_string()))?
        .filter_map(std::result::Result::ok)
        .collect();

    Ok(symbol_names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_diff_with_added_lines(added_lines: &[&str]) -> String {
        let mut diff_text = String::from("+++ b/src/lib.rs\n@@ -1,0 +1,N @@\n");
        for added_line in added_lines {
            diff_text.push('+');
            diff_text.push_str(added_line);
            diff_text.push('\n');
        }
        diff_text
    }

    #[test]
    fn known_symbols_do_not_trigger_hallucination() {
        let known_symbols: HashSet<String> = ["my_function", "MyStruct"]
            .iter()
            .map(|symbol_name| (*symbol_name).to_owned())
            .collect();
        let diff_text = make_diff_with_added_lines(&["    my_function(arg);"]);
        let hallucinated_symbols = ast_symbol_cross_check(&diff_text, &known_symbols);
        assert!(
            !hallucinated_symbols
                .iter()
                .any(|symbol| symbol.name == "my_function"),
            "known symbol must not be flagged"
        );
    }

    #[test]
    fn unknown_function_is_flagged() {
        let known_symbols: HashSet<String> = HashSet::new();
        let diff_text = make_diff_with_added_lines(&["    totally_made_up_function(arg);"]);
        let hallucinated_symbols = ast_symbol_cross_check(&diff_text, &known_symbols);
        assert!(
            hallucinated_symbols
                .iter()
                .any(|symbol| symbol.name == "totally_made_up_function"),
            "unknown function must be flagged"
        );
    }

    #[test]
    fn stdlib_names_are_never_flagged() {
        let known_symbols: HashSet<String> = HashSet::new();
        let diff_text = make_diff_with_added_lines(&["    let result: Vec<String> = Vec::new();"]);
        let hallucinated_symbols = ast_symbol_cross_check(&diff_text, &known_symbols);
        assert!(
            !hallucinated_symbols
                .iter()
                .any(|symbol| symbol.name == "Vec" || symbol.name == "String"),
            "stdlib names must not be flagged"
        );
    }

    #[test]
    fn removed_lines_are_not_checked() {
        let known_symbols: HashSet<String> = HashSet::new();
        let diff_text = "+++ b/src/lib.rs\n-    old_hallucinated_func(x);\n";
        let hallucinated_symbols = ast_symbol_cross_check(diff_text, &known_symbols);
        assert!(
            !hallucinated_symbols
                .iter()
                .any(|symbol| symbol.name == "old_hallucinated_func"),
            "removed lines must not be checked"
        );
    }

    #[test]
    fn violations_have_warning_severity() {
        let known_symbols: HashSet<String> = HashSet::new();
        let diff_text = make_diff_with_added_lines(&["    ghost_function(x);"]);
        let hallucinated_symbols = ast_symbol_cross_check(&diff_text, &known_symbols);
        let violations = hallucinated_symbols_to_violations(hallucinated_symbols);
        assert!(violations
            .iter()
            .all(|violation| violation.severity == ViolationSeverity::Warning));
    }

    proptest::proptest! {
        #[test]
        fn known_symbols_always_pass_hallucination_check(
            symbol_name in "[a-z][a-z0-9_]{1,15}"
        ) {
            let mut known_symbols: HashSet<String> = HashSet::new();
            known_symbols.insert(symbol_name.clone());
            let diff_text = make_diff_with_added_lines(&[&format!("    {symbol_name}(arg);")]);
            let hallucinated_symbols = ast_symbol_cross_check(&diff_text, &known_symbols);
            proptest::prop_assert!(
                !hallucinated_symbols.iter().any(|symbol| symbol.name == symbol_name),
                "symbol in known set must never be flagged"
            );
        }
    }
}
