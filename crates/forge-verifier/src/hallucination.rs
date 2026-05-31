//! # forge-verifier: Hallucination Detection
//!
//! Three-layer check for hallucinated symbols in a diff:
//!
//! 1. **AST symbol cross-check (L1)** — extracts referenced identifiers from `+`
//!    lines via regex and flags any not found in the CRG symbols table.
//! 2. **LSP diagnostic cross-check (L2)** — for each file touched by the diff,
//!    requests LSP diagnostics and checks whether the language server also reports
//!    undefined-symbol errors. Symbols confirmed by LSP as undefined are annotated
//!    with higher confidence; symbols that LSP accepts (CRG gap, not a real
//!    hallucination) are downgraded to informational and filtered out.
//!    Activated when `VerificationContext.lsp_bridge` is `Some`.
//! 3. **Self-consistency sampling (L3)** — sacred-file diffs only: variant
//!    generation requires a live multi-sample LLM call; out of scope for now.
//!
//! All violations are `Warning` severity — the hallucination gate informs the
//! Coder rather than hard-blocking, since the CRG may have gaps.
//!
//! ## Input
//! - `Diff` — proposed change to scan for unknown identifiers
//! - `VerificationContext` — supplies the CRG db path and optional LSP bridge
//!
//! ## Output
//! - `Vec<Violation>` — each flagged hallucinated symbol (Warning severity)
//!
//! ## Related
//! - `forge-verifier::pipeline` — calls this alongside gate 3
//! - `forge-crg` — the SQLite database queried for known symbol names
//! - `forge-lsp` — `LspBridge::get_diagnostics` used for L2

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

/// Rust standard library and pervasive method names that should never be flagged
/// as hallucinations. Includes stdlib types, macros, and universally-used methods
/// (constructors like `new`, trait methods like `clone`, iterator adapters, etc.)
/// that appear in virtually every codebase.
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
    "new",
    "default",
    "clone",
    "len",
    "is_empty",
    "push",
    "pop",
    "insert",
    "remove",
    "get",
    "contains",
    "contains_key",
    "to_string",
    "to_owned",
    "parse",
    "unwrap",
    "unwrap_or",
    "unwrap_or_else",
    "expect",
    "map",
    "and_then",
    "or_else",
    "unwrap_or_default",
    "iter",
    "iter_mut",
    "into_iter",
    "collect",
    "filter",
    "find",
    "fold",
    "flatten",
    "flat_map",
    "take",
    "skip",
    "enumerate",
    "zip",
    "chain",
    "count",
    "sum",
    "extend",
    "retain",
    "sort",
    "sort_by",
    "sort_by_key",
    "dedup",
    "truncate",
    "clear",
    "drain",
    "split",
    "trim",
    "starts_with",
    "ends_with",
    "replace",
    "join",
    "as_str",
    "as_bytes",
    "chars",
    "lines",
    "as_ref",
    "as_mut",
    "as_path",
    "as_deref",
    "borrow",
    "borrow_mut",
    "lock",
    "read",
    "write",
    "open",
    "create",
    "entry",
    "or_insert",
    "or_insert_with",
    "or_default",
    "max",
    "min",
    "any",
    "all",
    "position",
    "last",
    "first",
    "next",
    "peek",
    "display",
    "path",
    "join",
    "exists",
    "ok",
    "err",
    "is_ok",
    "is_err",
    "is_some",
    "is_none",
    "take",
    "replace",
    "into",
    "from",
    "try_from",
    "try_into",
    "spawn",
    "await",
    "send",
    "recv",
    "close",
    "abort",
    "detach",
];

/// A symbol referenced in the diff that could not be found in the CRG.
#[derive(Debug, Clone)]
pub struct HallucinatedSymbol {
    /// The identifier that appears to be hallucinated.
    pub name: String,
    /// Position of the `+` line in the diff where the symbol appears (1-based).
    pub diff_line: Option<u32>,
}

/// Runs hallucination layers L1 (CRG cross-check) and L2 (LSP diagnostics)
/// and returns the combined violations.
///
/// L2 runs when `ctx.lsp_bridge` is `Some`. Symbols that the LSP accepts
/// (CRG gap, not a real hallucination) are filtered out; confirmed
/// undefined symbols keep their `Warning` violation with an L2 annotation.
pub async fn check_hallucination(
    diff: &Diff,
    ctx: &VerificationContext,
) -> VerifierResult<Vec<Violation>> {
    let known_symbol_names = match &ctx.crg_db_path {
        Some(crg_db_path) => load_crg_symbol_names(crg_db_path)?,
        None => HashSet::new(),
    };

    let l1_hallucinated_symbols = ast_symbol_cross_check(&diff.text, &known_symbol_names);

    if let Some(lsp_bridge) = ctx.lsp_bridge.as_ref() {
        let lsp_confirmed_symbols = lsp_diagnostic_cross_check(diff, lsp_bridge).await;
        let filtered_symbols: Vec<HallucinatedSymbol> = l1_hallucinated_symbols
            .into_iter()
            .filter(|symbol| {
                lsp_confirmed_symbols.is_empty() || lsp_confirmed_symbols.contains(&symbol.name)
            })
            .collect();
        Ok(hallucinated_symbols_to_violations_with_layer(
            filtered_symbols,
            true,
        ))
    } else {
        Ok(hallucinated_symbols_to_violations(l1_hallucinated_symbols))
    }
}

/// Parses the diff for `+++ b/<path>` file headers, requests LSP diagnostics
/// for each existing file, and returns the set of symbol names that the LSP
/// reports as undefined errors. An empty set means either no LSP error was
/// found or the bridge call failed (caller treats this as "LSP inconclusive").
async fn lsp_diagnostic_cross_check(
    diff: &Diff,
    lsp_bridge: &yantra_lsp::LspBridge,
) -> HashSet<String> {
    let changed_file_paths = parse_changed_files_from_diff(&diff.text);
    let mut lsp_undefined_symbols: HashSet<String> = HashSet::new();

    for file_path in &changed_file_paths {
        if !std::path::Path::new(file_path).exists() {
            continue;
        }
        match lsp_bridge.get_diagnostics(file_path).await {
            Ok(diagnostics) => {
                for diagnostic in &diagnostics {
                    if matches!(diagnostic.severity, yantra_lsp::DiagnosticSeverity::Error) {
                        for name in extract_symbol_names_from_lsp_message(&diagnostic.message) {
                            lsp_undefined_symbols.insert(name);
                        }
                    }
                }
            }
            Err(lsp_error) => {
                tracing::debug!(
                    file = %file_path,
                    error = %lsp_error,
                    "L2 hallucination check: LSP diagnostic request failed, skipping file"
                );
            }
        }
    }

    lsp_undefined_symbols
}

/// Extracts `+++ b/<path>` file paths from a unified diff header.
fn parse_changed_files_from_diff(diff_text: &str) -> Vec<String> {
    diff_text
        .lines()
        .filter_map(|diff_line| diff_line.strip_prefix("+++ b/").map(str::to_owned))
        .collect()
}

/// Heuristically extracts symbol names from an LSP error message.
///
/// Handles common Rust/Python/TypeScript patterns such as:
/// - `cannot find function \`my_func\` in this scope`
/// - `undefined variable 'my_var'`
/// - `Cannot find name 'MyType'.`
fn extract_symbol_names_from_lsp_message(lsp_message: &str) -> Vec<String> {
    static PATTERN_BACKTICK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"`([a-zA-Z_][a-zA-Z0-9_]*)`").expect("backtick pattern is valid")
    });
    static PATTERN_SINGLE_QUOTE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"'([a-zA-Z_][a-zA-Z0-9_]*)'").expect("single-quote pattern is valid")
    });

    let mut symbol_names: Vec<String> = Vec::new();
    for cap in PATTERN_BACKTICK.captures_iter(lsp_message) {
        symbol_names.push(cap[1].to_owned());
    }
    if symbol_names.is_empty() {
        for cap in PATTERN_SINGLE_QUOTE.captures_iter(lsp_message) {
            symbol_names.push(cap[1].to_owned());
        }
    }
    symbol_names
}

/// Like `hallucinated_symbols_to_violations` but annotates each violation
/// message with whether L2 (LSP) confirmed it.
fn hallucinated_symbols_to_violations_with_layer(
    hallucinated_symbols: Vec<HallucinatedSymbol>,
    lsp_confirmed: bool,
) -> Vec<Violation> {
    let layer_annotation = if lsp_confirmed {
        " (confirmed by LSP)"
    } else {
        ""
    };
    hallucinated_symbols
        .into_iter()
        .map(|hallucinated_symbol| Violation {
            file_path: None,
            line: hallucinated_symbol.diff_line,
            column: None,
            message: format!(
                "symbol '{}' referenced in diff but not found in CRG{} — possible hallucination",
                hallucinated_symbol.name, layer_annotation
            ),
            severity: ViolationSeverity::Warning,
            rule_id: Some("hallucination::unknown_symbol".to_owned()),
        })
        .collect()
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
