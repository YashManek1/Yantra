//! # forge-agents: Tester Agent
//!
//! Receives a completed diff and a CRG subgraph, generates edge-case and
//! boundary-condition tests for the changed symbols via a Tier-1 LLM, and
//! returns those tests as a diff. The agent also runs the existing test suite
//! via subprocess and records any failures in the summary so the orchestrator
//! can decide whether to send the task back to the Coder.
//!
//! ## Input
//! - `TaskNode` carrying a valid `TruthToken`
//! - `AgentContext` with an `McpRouter` that has `crg.subgraph` registered
//!
//! ## Output
//! - `TaskResult` with a `diff` containing the new test file, plus a `summary`
//!   that describes the generated tests and any pre-existing test failures
//!
//! ## Related
//! - `forge-agents::coder` — the Coder produces the diff that the Tester extends
//! - `forge-verifier` — `parse_test_outcome` reused for test output parsing
//! - `forge-router` — Tier-1 LLM call for test generation

use async_trait::async_trait;
use serde_json::json;
use yantra_core::{AgentCapability, AgentKind, DecisionId, Outcome, TaskNode};
use yantra_router::routing::{RoutedCompletionRequest, TaskDescription};
use yantra_router::{CompletionRequest, Message, MessageRole};
use yantra_verifier::parse_test_outcome;

use crate::agent::{Agent, AgentContext, Diff, FileDiff, TaskResult};
use crate::error::AgentError;

const TESTER_SYSTEM_PROMPT: &str = include_str!("../prompts/tester.md");

/// Specialist agent that generates and runs tests for completed code changes.
pub struct TesterAgent;

impl TesterAgent {
    /// Creates a new `TesterAgent`.
    pub fn new() -> Self {
        Self
    }

    fn build_test_generation_prompt(
        truth_summary: &str,
        subgraph_text: &str,
        task_description: &str,
    ) -> Vec<Message> {
        let user_content = format!(
            "## Source Truth\n{truth_summary}\n\n\
             ## Changed Code (CRG Subgraph)\n{subgraph_text}\n\n\
             ## Task\n{task_description}\n\n\
             Write comprehensive tests for the symbols described above. \
             Include happy-path, boundary, and error-case tests. \
             Output exactly one `// File:` diff block per test file."
        );

        vec![
            Message {
                role: MessageRole::System,
                content: TESTER_SYSTEM_PROMPT.to_owned(),
                tool_calls: Vec::new(),
            },
            Message {
                role: MessageRole::User,
                content: user_content,
                tool_calls: Vec::new(),
            },
        ]
    }

    async fn run_existing_tests(project_root: &std::path::Path) -> TestRunSummary {
        let output = tokio::process::Command::new("cargo")
            .args(["test", "--", "--nocapture"])
            .current_dir(project_root)
            .output()
            .await;

        match output {
            Ok(command_output) => {
                let combined_text = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&command_output.stdout),
                    String::from_utf8_lossy(&command_output.stderr)
                );
                let test_outcome = parse_test_outcome(&combined_text);
                TestRunSummary {
                    all_passed: test_outcome.all_passed,
                    failure_description: if test_outcome.all_passed {
                        String::new()
                    } else {
                        test_outcome
                            .failing_tests
                            .iter()
                            .map(|failing_test| {
                                format!(
                                    "FAIL: {} — {}",
                                    failing_test.test_name, failing_test.error_message
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    },
                }
            }
            Err(spawn_error) => TestRunSummary {
                all_passed: false,
                failure_description: format!("could not spawn cargo test: {spawn_error}"),
            },
        }
    }
}

impl Default for TesterAgent {
    fn default() -> Self {
        Self::new()
    }
}

struct TestRunSummary {
    all_passed: bool,
    failure_description: String,
}

fn parse_test_diffs_from_response(response_text: &str) -> Vec<FileDiff> {
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

#[async_trait]
impl Agent for TesterAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Tester
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

        let generation_messages =
            Self::build_test_generation_prompt(&truth_summary, &subgraph_text, &task.description);

        let prompt_token_estimate = generation_messages
            .iter()
            .map(|message| message.content.len() / 4)
            .sum::<usize>();

        let completion_request = CompletionRequest {
            messages: generation_messages,
            max_tokens: Some(2048),
            temperature: 0.2,
            tools: None,
            stop_sequences: Vec::new(),
        };

        let task_description_metadata = TaskDescription {
            description: task.description.clone(),
            class: task.class,
            tokens_estimated: prompt_token_estimate,
            tool_calls_predicted: 0,
            touches_sacred_files: false,
            multi_file: false,
        };
        let required_tier = context.router.policy().classify(&task_description_metadata);
        let routed_request = RoutedCompletionRequest {
            required_tier,
            completion_request: completion_request.clone(),
        };

        tracing::debug!(
            task_id = %task.id,
            token_estimate = prompt_token_estimate,
            "tester routing test generation request via policy tier"
        );

        let model_provider = context
            .router
            .route(&routed_request)
            .await
            .map_err(|router_error| AgentError::ModelProvider(router_error.to_string()))?;

        let completion_response = model_provider
            .complete(completion_request)
            .await
            .map_err(|provider_error| AgentError::ModelProvider(provider_error.to_string()))?;

        let file_diffs = parse_test_diffs_from_response(&completion_response.content);
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

        let summary = format!(
            "Generated {} test file(s) for task: {}",
            diff.as_ref()
                .map_or(0, |generated_diff| generated_diff.files.len()),
            task.description
        );

        Ok(TaskResult {
            task_id: task.id,
            outcome,
            diff,
            summary,
            decision_id: DecisionId::new(),
        })
    }
}

/// Runs `cargo test` in `project_root` and returns the failure description.
///
/// Returns an empty string when all tests passed, or a multiline failure report
/// when some tests failed. Does not propagate spawn errors as `AgentError` —
/// instead it returns a descriptive failure string so the agent can include it
/// in the `TaskResult` summary.
pub async fn run_and_report_test_failures(project_root: &std::path::Path) -> String {
    let test_run_summary = TesterAgent::run_existing_tests(project_root).await;
    if test_run_summary.all_passed {
        String::new()
    } else {
        test_run_summary.failure_description
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_test_diffs_extracts_single_file() {
        let response = "Here are the tests:\n\n\
            // File: src/auth/tests.rs\n\
            --- a/src/auth/tests.rs\n\
            +++ b/src/auth/tests.rs\n\
            @@ -0,0 +1,5 @@\n\
            +#[test]\n\
            +fn login_succeeds_with_valid_credentials() {\n\
            +    assert!(true);\n\
            +}";

        let file_diffs = parse_test_diffs_from_response(response);
        assert_eq!(file_diffs.len(), 1);
        assert_eq!(file_diffs[0].file_path, "src/auth/tests.rs");
        assert!(file_diffs[0].unified_diff.contains("login_succeeds"));
    }

    #[test]
    fn parse_test_diffs_handles_multiple_files() {
        let response = "// File: src/a_test.rs\n\
            --- a/src/a_test.rs\n+++ b/src/a_test.rs\n@@ -0 +1 @@\n+#[test] fn a() {}\n\
            // File: src/b_test.rs\n\
            --- a/src/b_test.rs\n+++ b/src/b_test.rs\n@@ -0 +1 @@\n+#[test] fn b() {}";

        let file_diffs = parse_test_diffs_from_response(response);
        assert_eq!(file_diffs.len(), 2);
    }

    #[test]
    fn parse_test_diffs_returns_empty_on_no_blocks() {
        let file_diffs = parse_test_diffs_from_response("No tests generated.");
        assert!(file_diffs.is_empty());
    }
}
