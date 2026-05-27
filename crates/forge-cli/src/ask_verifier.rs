//! # Symbol Allowlist Verifier
//!
//! Scans LLM answers to the `ask` command for codebase-specific symbols or file paths
//! and verifies them against the symbols/paths included in the extracted CRG subgraph.
//! Returns a list of potentially hallucinated or unverified symbols/paths.
//!
//! ## Input
//! - `subgraph: &RenderedSubgraph` — the extracted CRG subgraph containing included symbols
//! - `graph_cache: &GraphCache` — the pre-built global graph cache to resolve symbol names
//! - `answer: &str` — the raw natural-language completion response text from the LLM
//!
//! ## Output
//! - `Vec<String>` — a list of unverified symbol references or file paths found in the answer
//!
//! ## Related
//! - `forge-crg::subgraph` — defines `RenderedSubgraph` and `GraphCache`
//! - `forge-cli::main` — invokes the verifier and prints warning messages if needed

use regex::Regex;
use std::collections::HashSet;
use std::path::Path;
use yantra_crg::{GraphCache, RenderedSubgraph};

/// Verifies whether symbols and paths cited in the LLM answer are grounded in the subgraph.
pub struct SymbolAllowlistVerifier;

impl SymbolAllowlistVerifier {
    /// Inspects the natural-language answer text for codebase-specific symbols or paths
    /// and cross-references them with the set of allowed nodes in the rendered subgraph.
    pub fn verify(
        subgraph: &RenderedSubgraph,
        graph_cache: &GraphCache,
        answer: &str,
    ) -> Vec<String> {
        let mut allowed_symbols = HashSet::new();
        let mut allowed_files = HashSet::new();
        let mut allowed_basenames = HashSet::new();

        // 1. Gather all allowed symbols and files from the subgraph included nodes
        for symbol_id in &subgraph.included_nodes {
            if let Some(details) = graph_cache.symbol_details.get(symbol_id) {
                allowed_symbols.insert(details.name.clone());
                allowed_files.insert(details.file_path.clone());

                if let Some(basename) = Path::new(&details.file_path)
                    .file_name()
                    .and_then(|os_str| os_str.to_str())
                {
                    allowed_basenames.insert(basename.to_string());
                }
            }
        }

        // Add common built-in, standard Rust, and ubiquitous architectural keywords to avoid false alerts
        let mut standard_exclusions = HashSet::new();
        let common_words = vec![
            "String",
            "Vec",
            "Result",
            "Option",
            "Some",
            "None",
            "Ok",
            "Err",
            "Self",
            "self",
            "u8",
            "u16",
            "u32",
            "u64",
            "u128",
            "usize",
            "i8",
            "i16",
            "i32",
            "i64",
            "i128",
            "isize",
            "f32",
            "f64",
            "bool",
            "char",
            "str",
            "Connection",
            "Mutex",
            "Arc",
            "Router",
            "commands",
            "main",
            "yantra",
            "run",
            "ask",
            "index",
            "status",
            "version",
            "task_id",
            "session_id",
            "span_id",
            "true",
            "false",
            "anyhow",
            "Result",
            "Error",
            "impl",
            "struct",
            "enum",
            "trait",
        ];
        for word in common_words {
            standard_exclusions.insert(word.to_string());
        }

        let mut unverified_references = HashSet::new();

        // 2. Scan for backticked identifiers e.g. `SymbolId` or `crates/forge-core/src/id.rs`
        if let Ok(backtick_regex) = Regex::new(r"`([^`\s]+)`") {
            for capture in backtick_regex.captures_iter(answer) {
                if let Some(matched_group) = capture.get(1) {
                    let identifier = matched_group.as_str().to_string();

                    // Strip rust path prefix/suffix if present e.g. `yantra_core::SymbolId` -> `SymbolId`
                    let clean_identifier = if identifier.contains("::") {
                        identifier
                            .split("::")
                            .last()
                            .unwrap_or(&identifier)
                            .to_string()
                    } else {
                        identifier.clone()
                    };

                    if standard_exclusions.contains(&clean_identifier) {
                        continue;
                    }

                    // Check if it is a path or a symbol name
                    let is_file_path = clean_identifier.contains('/')
                        || clean_identifier.contains('\\')
                        || Path::new(&clean_identifier)
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"));

                    if is_file_path {
                        if !allowed_files.contains(&clean_identifier)
                            && !allowed_basenames.contains(&clean_identifier)
                        {
                            unverified_references.insert(identifier);
                        }
                    } else if !allowed_symbols.contains(&clean_identifier) {
                        unverified_references.insert(identifier);
                    }
                }
            }
        }

        // 3. Scan for unquoted path-like strings in the text e.g. src/auth/jwt.rs or crates/forge-stvp/src/questionnaire.rs
        if let Ok(path_regex) =
            Regex::new(r"\b[a-zA-Z0-9_\-\./]+\.rs\b|\b[a-zA-Z0-9_\-]+/[a-zA-Z0-9_\-\./]+\b")
        {
            for capture in path_regex.captures_iter(answer) {
                if let Some(matched_group) = capture.get(0) {
                    let path_candidate = matched_group.as_str().to_string();
                    let is_path_candidate = path_candidate.contains('/')
                        || path_candidate.contains('\\')
                        || Path::new(&path_candidate)
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"));

                    if is_path_candidate
                        && !allowed_files.contains(&path_candidate)
                        && !allowed_basenames.contains(&path_candidate)
                    {
                        unverified_references.insert(path_candidate);
                    }
                }
            }
        }

        let mut sorted_unverified: Vec<String> = unverified_references.into_iter().collect();
        sorted_unverified.sort();
        sorted_unverified
    }
}
