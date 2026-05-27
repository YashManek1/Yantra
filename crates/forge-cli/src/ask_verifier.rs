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

    /// Scans the answer for potential cross-crate role conflations between PreFlight and Runtime symbols.
    pub fn check_cross_crate_conflations(
        subgraph: &RenderedSubgraph,
        graph_cache: &GraphCache,
        answer: &str,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        let normalized_answer = answer.to_lowercase();

        let mut symbol_phases = std::collections::HashMap::new();
        for symbol_id in &subgraph.included_nodes {
            if let Some(details) = graph_cache.symbol_details.get(symbol_id) {
                let phase = get_lifecycle_phase(&details.file_path);
                symbol_phases.insert(details.name.to_lowercase(), (details.name.clone(), phase));
            }
        }

        let segments: Vec<&str> = normalized_answer.split(['.', '?', '!', '\n']).collect();

        for segment in segments {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                continue;
            }

            for (symbol_name_lower, (symbol_original_name, symbol_phase)) in &symbol_phases {
                if trimmed.contains(symbol_name_lower) {
                    if *symbol_phase == "Runtime" {
                        for word in PRE_FLIGHT_KEYWORDS {
                            if trimmed.contains(word) {
                                let warning = format!(
                                    "Symbol `{symbol_original_name}` (Runtime) is cited near PreFlight concepts like \"{word}\""
                                );
                                if !warnings.contains(&warning) {
                                    warnings.push(warning);
                                }
                            }
                        }
                    } else if *symbol_phase == "PreFlight" {
                        for word in RUNTIME_KEYWORDS {
                            if trimmed.contains(word) {
                                let warning = format!(
                                    "Symbol `{symbol_original_name}` (PreFlight) is cited near Runtime concepts like \"{word}\""
                                );
                                if !warnings.contains(&warning) {
                                    warnings.push(warning);
                                }
                            }
                        }
                    }
                }
            }
        }

        warnings
    }
}

const PRE_FLIGHT_KEYWORDS: &[&str] = &[
    "validate",
    "validation",
    "consistency",
    "truth token",
    "interrogator",
    "questionnaire",
    "drift",
];

const RUNTIME_KEYWORDS: &[&str] = &[
    "agent", "run", "schedule", "dispatch", "execute", "commit", "mcp",
];

fn get_lifecycle_phase(file_path: &str) -> &'static str {
    let lower_path = file_path.to_lowercase();
    if lower_path.contains("forge-agents")
        || lower_path.contains("agents")
        || lower_path.contains("forge-orchestrator")
        || lower_path.contains("orchestrator")
        || lower_path.contains("forge-night")
        || lower_path.contains("night")
        || lower_path.contains("forge-sidecar")
        || lower_path.contains("sidecar")
        || lower_path.contains("forge-serve")
        || lower_path.contains("serve")
        || lower_path.contains("forge-cli")
        || lower_path.contains("cli")
        || lower_path.contains("forge-eval")
        || lower_path.contains("eval")
        || lower_path.contains("forge-router")
        || lower_path.contains("router")
        || lower_path.contains("forge-lsp")
        || lower_path.contains("lsp")
        || lower_path.contains("forge-tools")
        || lower_path.contains("tools")
        || lower_path.contains("forge-swarm")
        || lower_path.contains("swarm")
    {
        "Runtime"
    } else if lower_path.contains("forge-stvp")
        || lower_path.contains("stvp")
        || lower_path.contains("forge-verifier")
        || lower_path.contains("verifier")
    {
        "PreFlight"
    } else if lower_path.contains("forge-obs")
        || lower_path.contains("obs")
        || lower_path.contains("tracing")
    {
        "Observability"
    } else {
        "Persistence"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use yantra_core::SymbolId;
    use yantra_crg::{RenderedSubgraph, SubgraphManifest, SymbolDetails};

    #[test]
    fn test_verify_symbol_allowlist() {
        let first_symbol_id = SymbolId::new("VerifierAgent").unwrap();
        let second_symbol_id = SymbolId::new("CodebaseRealityValidator").unwrap();

        let mut symbol_details_map = HashMap::new();
        symbol_details_map.insert(
            first_symbol_id.clone(),
            SymbolDetails {
                symbol_id: first_symbol_id.clone(),
                name: "VerifierAgent".to_string(),
                kind: "struct".to_string(),
                start_line: 10,
                signature: None,
                docstring: None,
                connectivity_score: 5,
                file_path: "crates/forge-agents/src/verifier_agent.rs".to_string(),
                file_id: "1".to_string(),
                token_cost_with_docstring: 5,
                token_cost_no_docstring: 5,
            },
        );
        symbol_details_map.insert(
            second_symbol_id.clone(),
            SymbolDetails {
                symbol_id: second_symbol_id.clone(),
                name: "CodebaseRealityValidator".to_string(),
                kind: "struct".to_string(),
                start_line: 20,
                signature: None,
                docstring: None,
                connectivity_score: 10,
                file_path: "crates/forge-stvp/src/validation.rs".to_string(),
                file_id: "2".to_string(),
                token_cost_with_docstring: 10,
                token_cost_no_docstring: 10,
            },
        );

        let graph_cache = GraphCache {
            symbol_details: symbol_details_map,
            adjacency_index: HashMap::new(),
            symbols_by_name: HashMap::new(),
            symbols_by_file_id: HashMap::new(),
            symbol_to_file_id: HashMap::new(),
        };

        let rendered_subgraph = RenderedSubgraph {
            text: String::new(),
            included_nodes: vec![first_symbol_id.clone()],
            token_cost: 0,
            manifest: SubgraphManifest { nodes: vec![] },
        };

        let unverified_symbol_references = SymbolAllowlistVerifier::verify(
            &rendered_subgraph,
            &graph_cache,
            "We should look at `VerifierAgent` and `SomeHallucinatedSymbol` in `crates/forge-cli/src/main.rs`."
        );

        assert!(unverified_symbol_references.contains(&"SomeHallucinatedSymbol".to_string()));
        assert!(!unverified_symbol_references.contains(&"VerifierAgent".to_string()));
    }

    #[test]
    fn test_cross_crate_conflations() {
        let first_symbol_id = SymbolId::new("VerifierAgent").unwrap();
        let second_symbol_id = SymbolId::new("CodebaseRealityValidator").unwrap();

        let mut symbol_details_map = HashMap::new();
        symbol_details_map.insert(
            first_symbol_id.clone(),
            SymbolDetails {
                symbol_id: first_symbol_id.clone(),
                name: "VerifierAgent".to_string(),
                kind: "struct".to_string(),
                start_line: 10,
                signature: None,
                docstring: None,
                connectivity_score: 5,
                file_path: "crates/forge-agents/src/verifier_agent.rs".to_string(),
                file_id: "1".to_string(),
                token_cost_with_docstring: 5,
                token_cost_no_docstring: 5,
            },
        );
        symbol_details_map.insert(
            second_symbol_id.clone(),
            SymbolDetails {
                symbol_id: second_symbol_id.clone(),
                name: "CodebaseRealityValidator".to_string(),
                kind: "struct".to_string(),
                start_line: 20,
                signature: None,
                docstring: None,
                connectivity_score: 10,
                file_path: "crates/forge-stvp/src/validation.rs".to_string(),
                file_id: "2".to_string(),
                token_cost_with_docstring: 10,
                token_cost_no_docstring: 10,
            },
        );

        let graph_cache = GraphCache {
            symbol_details: symbol_details_map,
            adjacency_index: HashMap::new(),
            symbols_by_name: HashMap::new(),
            symbols_by_file_id: HashMap::new(),
            symbol_to_file_id: HashMap::new(),
        };

        let rendered_subgraph = RenderedSubgraph {
            text: String::new(),
            included_nodes: vec![first_symbol_id.clone(), second_symbol_id.clone()],
            token_cost: 0,
            manifest: SubgraphManifest { nodes: vec![] },
        };

        let cross_crate_conflation_warnings = SymbolAllowlistVerifier::check_cross_crate_conflations(
            &rendered_subgraph,
            &graph_cache,
            "The `VerifierAgent` validates the workspace. The `CodebaseRealityValidator` will run the scheduler."
        );

        assert!(cross_crate_conflation_warnings
            .iter()
            .any(|warning_message| warning_message.contains("VerifierAgent")
                && warning_message.contains("validate")));
        assert!(cross_crate_conflation_warnings
            .iter()
            .any(
                |warning_message| warning_message.contains("CodebaseRealityValidator")
                    && warning_message.contains("run")
            ));
    }
}
