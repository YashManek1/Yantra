//! # forge-agents: Coder Agent
//!
//! Implements the Coder specialist agent. On each run it queries the CRG for a
//! token-budgeted subgraph, assembles a prompt from the system persona
//! (`prompts/coder.md`) and the subgraph, routes the request through the model
//! router, and parses any `// File: <path>` unified-diff blocks out of the
//! model response.
//!
//! ## Input
//! - `TaskNode` with a valid `TruthToken`
//! - `AgentContext` carrying a `Router` and an `McpRouter` with `crg.subgraph` registered
//!
//! ## Output
//! - `TaskResult` with `outcome`, `diff` (first file-diff block found), and a summary line
//!
//! ## Related
//! - `forge-agents::agent` — defines the `Agent` trait and shared types
//! - `forge-tools::crg` — the `CrgMcpServer` that handles `crg.subgraph`
//! - `forge-router` — routes the assembled prompt to a model provider

use async_trait::async_trait;
use serde_json::json;
use yantra_core::{AgentCapability, AgentKind, DecisionId, Outcome, TaskNode};
use yantra_router::routing::RoutedCompletionRequest;
use yantra_router::{CompletionRequest, Message, MessageRole, TaskDescription};

use crate::agent::{Agent, AgentContext, Diff, FileDiff, TaskResult};
use crate::error::AgentError;

const CODER_SYSTEM_PROMPT: &str = include_str!("../prompts/coder.md");

/// Specialist agent that produces code diffs from CRG context and truth tokens.
pub struct CoderAgent;

impl CoderAgent {
    /// Creates a new `CoderAgent`.
    pub fn new() -> Self {
        Self
    }

    /// Builds the two-message prompt list for a Coder run.
    ///
    /// The system message is the compiled persona from `prompts/coder.md`.
    /// The user message contains the truth summary, CRG subgraph, and task.
    pub(crate) fn assemble_prompt_messages(
        truth_summary: &str,
        subgraph_text: &str,
        task_description: &str,
    ) -> Vec<Message> {
        let user_content = format!(
            "## Source Truth\n{truth_summary}\n\n\
             ## Code Context (CRG Subgraph)\n{subgraph_text}\n\n\
             ## Task\n{task_description}\n\n\
             Provide your changes as unified diffs, \
             each preceded by a `// File: <relative/path>` comment."
        );
        vec![
            Message {
                role: MessageRole::System,
                content: CODER_SYSTEM_PROMPT.to_owned(),
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

impl Default for CoderAgent {
    fn default() -> Self {
        Self::new()
    }
}

/// Parses all `// File: <path>` unified-diff blocks from a model response.
fn parse_diffs_from_response(response_text: &str) -> Vec<FileDiff> {
    let normalized = response_text.replace("\r\n", "\n");
    let searchable = format!("\n{normalized}");

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

#[async_trait]
impl Agent for CoderAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Coder
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
            .map_err(|error| AgentError::McpToolCall(error.message))?;

        let subgraph_text = crg_response
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_owned();

        let truth_summary = format!(
            "Task ID: {}\nClass: {:?}\nStrictness: {:?}",
            truth_token.task_id, truth_token.task_class, truth_token.strictness
        );
        let messages =
            Self::assemble_prompt_messages(&truth_summary, &subgraph_text, &task.description);

        let prompt_text: String = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let completion_request = CompletionRequest {
            messages,
            max_tokens: Some(2048),
            temperature: 0.2,
            tools: None,
            stop_sequences: Vec::new(),
        };

        let task_desc = TaskDescription {
            description: task.description.clone(),
            class: task.class,
            tokens_estimated: prompt_text.len() / 4,
            tool_calls_predicted: 0,
            touches_sacred_files: false,
            multi_file: false,
        };
        let required_tier = context.router.policy().classify(&task_desc);
        let routed = RoutedCompletionRequest {
            required_tier,
            completion_request: completion_request.clone(),
        };
        let provider = context
            .router
            .route(&routed)
            .map_err(|error| AgentError::ModelProvider(error.to_string()))?;
        let completion_response = provider
            .complete(completion_request)
            .await
            .map_err(|error| AgentError::ModelProvider(error.to_string()))?;

        let file_diffs = parse_diffs_from_response(&completion_response.content);
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
        let summary = completion_response
            .content
            .lines()
            .next()
            .unwrap_or("")
            .to_owned();

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
    use yantra_router::MessageRole;

    use super::*;

    #[test]
    fn prompt_assembly_includes_all_sections() {
        let truth_summary = "Task ID: test\nClass: Docstring\nStrictness: Trust";
        let subgraph_text =
            "## Symbol: add_numbers\n```rust\npub fn add_numbers(a: i32, b: i32) -> i32 { a + b }\n```";
        let task_description = "add docstring to add_numbers";

        let messages =
            CoderAgent::assemble_prompt_messages(truth_summary, subgraph_text, task_description);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[1].content.contains("Code Context"));
        assert!(messages[1].content.contains(subgraph_text));
        assert!(messages[1].content.contains(task_description));
        assert!(messages[1].content.contains(truth_summary));
    }

    #[test]
    fn prompt_assembly_system_message_contains_persona() {
        let messages = CoderAgent::assemble_prompt_messages("", "", "");
        assert!(!messages[0].content.is_empty());
        assert!(messages[0].content.contains("Coder"));
    }

    #[test]
    fn diff_parsing_extracts_file_path_and_unified_diff() {
        let response = "Here are the changes:\n\n\
            // File: src/lib.rs\n\
            --- a/src/lib.rs\n\
            +++ b/src/lib.rs\n\
            @@ -1,3 +1,4 @@\n\
            +/// Adds two integers and returns their sum.\n\
             pub fn add_numbers(a: i32, b: i32) -> i32 {\n\
                 a + b\n\
             }";

        let file_diffs = parse_diffs_from_response(response);

        assert_eq!(file_diffs.len(), 1);
        assert_eq!(file_diffs[0].file_path, "src/lib.rs");
        assert!(file_diffs[0].unified_diff.contains("Adds two integers"));
        assert!(file_diffs[0].unified_diff.contains("---"));
    }

    #[test]
    fn diff_parsing_handles_multiple_files() {
        let response = "// File: src/a.rs\n\
            --- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n+/// A.\n pub fn a() {}\n\
            // File: src/b.rs\n\
            --- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n+/// B.\n pub fn b() {}";

        let file_diffs = parse_diffs_from_response(response);

        assert_eq!(file_diffs.len(), 2);
        assert_eq!(file_diffs[0].file_path, "src/a.rs");
        assert_eq!(file_diffs[1].file_path, "src/b.rs");
    }

    #[test]
    fn diff_parsing_returns_empty_for_no_diff_blocks() {
        let response = "I cannot produce a diff because the function already has a docstring.";
        let file_diffs = parse_diffs_from_response(response);
        assert!(file_diffs.is_empty());
    }

    #[test]
    fn diff_parsing_ignores_empty_diff_sections() {
        let response = "// File: src/lib.rs\n";
        let file_diffs = parse_diffs_from_response(response);
        assert!(file_diffs.is_empty());
    }
}
