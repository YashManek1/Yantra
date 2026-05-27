//! # forge-agents: Refactorer Agent
//!
//! Receives passing code and produces a behaviour-preserving refactoring diff
//! that reduces cyclomatic complexity, enforces naming consistency, and removes
//! dead code. The diff is passed through the Boolean Exit Gate by the pipeline;
//! if the gate fails the orchestrator discards it and does not retry.
//!
//! ## Input
//! - `TaskNode` carrying a valid `TruthToken`
//! - `AgentContext` with `crg.subgraph` registered in the `McpRouter`
//!
//! ## Output
//! - `TaskResult` containing the refactored diff and a summary of what was changed
//!
//! ## Related
//! - `forge-agents::coder` — shares the `// File:` diff-block parsing pattern
//! - `forge-verifier::pipeline` — enforces that no tests regress after the diff
//! - `forge-router` — Tier-1 LLM call for complex structural reasoning

use async_trait::async_trait;
use serde_json::json;
use yantra_core::{AgentCapability, AgentKind, DecisionId, ModelTier, Outcome, TaskNode};
use yantra_router::routing::RoutedCompletionRequest;
use yantra_router::{CompletionRequest, Message, MessageRole};

use crate::agent::{Agent, AgentContext, Diff, FileDiff, TaskResult};
use crate::error::AgentError;

const REFACTORER_SYSTEM_PROMPT: &str = include_str!("../prompts/refactorer.md");

/// Specialist agent that produces behaviour-preserving code-structure improvements.
pub struct RefactorerAgent;

impl RefactorerAgent {
    /// Creates a new `RefactorerAgent`.
    pub fn new() -> Self {
        Self
    }

    fn build_refactor_prompt(
        truth_summary: &str,
        subgraph_text: &str,
        task_description: &str,
    ) -> Vec<Message> {
        let user_content = format!(
            "## Source Truth\n{truth_summary}\n\n\
             ## Code to Refactor (CRG Subgraph)\n{subgraph_text}\n\n\
             ## Refactoring Goal\n{task_description}\n\n\
             Apply the GROUND + DIFF protocol. Focus on cyclomatic complexity, \
             naming consistency, and dead code. Do NOT change observable behaviour."
        );

        vec![
            Message {
                role: MessageRole::System,
                content: REFACTORER_SYSTEM_PROMPT.to_owned(),
                tool_calls: Vec::new(),
            },
            Message {
                role: MessageRole::User,
                content: user_content,
                tool_calls: Vec::new(),
            },
        ]
    }
}

impl Default for RefactorerAgent {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_refactor_diffs_from_response(response_text: &str) -> Vec<FileDiff> {
    let normalised = response_text.replace("\r\n", "\n");
    let searchable = format!("\n{normalised}");

    searchable
        .split("\n// File: ")
        .skip(1)
        .filter_map(|section| {
            let (path_line, diff_content) = section.split_once('\n').unwrap_or((section, ""));
            let file_path = path_line.trim().to_owned();
            let unified_diff = diff_content.trim_end().to_owned();
            if unified_diff.is_empty() {
                None
            } else {
                Some(FileDiff {
                    file_path,
                    unified_diff,
                })
            }
        })
        .collect()
}

/// Extracts a one-line refactoring summary from the LLM response.
///
/// Looks for the first non-blank line that is not part of the GROUNDED SYMBOLS
/// block, which is typically the reasoning summary the refactorer writes between
/// Phase 1 and the first `// File:` block.
pub fn extract_refactor_summary(response_text: &str) -> String {
    let skip_prefixes = [
        "GROUNDED SYMBOLS",
        "INSERTION POINT",
        "PRIMARY FILES",
        "-",
        "//",
        "+",
        "@",
    ];

    for response_line in response_text.lines() {
        let trimmed = response_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if skip_prefixes
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
        {
            continue;
        }
        return trimmed.to_owned();
    }
    "refactoring applied".to_owned()
}

#[async_trait]
impl Agent for RefactorerAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Refactorer
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![AgentCapability::ReadFiles, AgentCapability::WriteFiles]
    }

    async fn run(&self, task: TaskNode, context: AgentContext) -> Result<TaskResult, AgentError> {
        let truth_token =
            task.truth_token
                .as_ref()
                .ok_or_else(|| AgentError::MissingTruthToken {
                    task_id: task.id.to_string(),
                })?;

        let crg_response = context
            .tools
            .handle(
                "crg.subgraph",
                json!({ "task": task.description, "budget": 4096_u64 }),
            )
            .await
            .map_err(|mcp_error| AgentError::McpToolCall(mcp_error.message))?;

        let subgraph_text = crg_response
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_owned();

        let truth_summary = format!(
            "Task ID: {}\nClass: {:?}\nStrictness: {:?}",
            truth_token.task_id, truth_token.task_class, truth_token.strictness
        );

        let refactor_messages =
            Self::build_refactor_prompt(&truth_summary, &subgraph_text, &task.description);

        let prompt_token_estimate = refactor_messages
            .iter()
            .map(|message| message.content.len() / 4)
            .sum::<usize>();

        let completion_request = CompletionRequest {
            messages: refactor_messages,
            max_tokens: Some(2048),
            temperature: 0.1,
            tools: None,
            stop_sequences: Vec::new(),
        };

        let routed_request = RoutedCompletionRequest {
            required_tier: ModelTier::Tier1,
            completion_request: completion_request.clone(),
        };

        tracing::debug!(
            task_id = %task.id,
            token_estimate = prompt_token_estimate,
            "refactorer routing request to Tier-1"
        );

        let model_provider = context
            .router
            .route(&routed_request)
            .map_err(|router_error| AgentError::ModelProvider(router_error.to_string()))?;

        let completion_response = model_provider
            .complete(completion_request)
            .await
            .map_err(|provider_error| AgentError::ModelProvider(provider_error.to_string()))?;

        let file_diffs = parse_refactor_diffs_from_response(&completion_response.content);
        let diff = if file_diffs.is_empty() {
            None
        } else {
            Some(Diff { files: file_diffs })
        };

        let outcome = if diff.is_some() {
            Outcome::Success
        } else {
            Outcome::Failure
        };

        let summary = extract_refactor_summary(&completion_response.content);

        Ok(TaskResult {
            task_id: task.id,
            outcome,
            diff,
            summary,
            decision_id: DecisionId::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_summary_skips_phase1_block() {
        let response = "GROUNDED SYMBOLS:\n- process_items  [src/lib.rs]\n\n\
            INSERTION POINT: process_items  [src/lib.rs]\n\
            PRIMARY FILES TO EDIT: src/lib.rs\n\n\
            Extracted inner loop into a dedicated helper to reduce nesting depth.\n\n\
            // File: src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs";

        let summary = extract_refactor_summary(response);
        assert_eq!(
            summary,
            "Extracted inner loop into a dedicated helper to reduce nesting depth."
        );
    }

    #[test]
    fn parse_refactor_diffs_extracts_file() {
        let response = "Some reasoning.\n\n\
            // File: src/lib.rs\n\
            --- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,2 @@\n-let v = 1;\n+let value = 1;";

        let file_diffs = parse_refactor_diffs_from_response(response);
        assert_eq!(file_diffs.len(), 1);
        assert_eq!(file_diffs[0].file_path, "src/lib.rs");
    }

    #[test]
    fn parse_refactor_diffs_returns_empty_when_no_diff() {
        let response = "GROUNDED SYMBOLS: none\nNo refactoring needed.";
        let file_diffs = parse_refactor_diffs_from_response(response);
        assert!(file_diffs.is_empty());
    }
}
