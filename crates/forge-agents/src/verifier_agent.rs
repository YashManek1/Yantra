//! # forge-agents: Verifier Agent
//!
//! LLM-driven operator of the Boolean Exit Gate. Runs the project's test suite
//! via subprocess, sends the raw output to a Tier-1 model for structured
//! diagnosis, parses the `VERDICT: PASS/FAIL` response, and returns a binary
//! outcome with a structured reason.
//!
//! ## Input
//! - `TaskNode` carrying a valid `TruthToken` and a `description` that may
//!   include a `project_root:` hint (e.g. `"project_root:/path verify: …"`)
//! - `AgentContext` with a Tier-1-capable `Router`
//!
//! ## Output
//! - `TaskResult` with `outcome = Success` when tests pass, `Failure` when they
//!   fail; `summary` contains the structured `VerifierVerdict` as JSON
//!
//! ## Related
//! - `forge-verifier::gate3_boolean_exit` — `parse_test_outcome` is reused here
//! - `forge-verifier::pipeline` — this agent wraps just the test-run + LLM layer
//! - `forge-agents::committer` — only dispatched after this agent returns Pass

use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use yantra_core::{AgentCapability, AgentKind, DecisionId, ModelTier, Outcome, TaskNode};
use yantra_router::routing::RoutedCompletionRequest;
use yantra_router::{CompletionRequest, Message, MessageRole};
use yantra_verifier::parse_test_outcome;

use crate::agent::{Agent, AgentContext, TaskResult};
use crate::error::AgentError;

const VERIFIER_SYSTEM_PROMPT: &str = include_str!("../prompts/verifier.md");

static PATTERN_VERDICT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^VERDICT:\s*(PASS|FAIL)").expect("verdict pattern is valid"));

static PATTERN_REASON_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^REASON:\s*(.+)").expect("reason pattern is valid"));

static PATTERN_FAILING_TEST_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^-\s+(.+?)\s*:\s*(.+)").expect("failing test line pattern is valid")
});

static PATTERN_RECOMMENDED_FIX_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^RECOMMENDED_FIX:\s*(.+)").expect("recommended fix pattern is valid")
});

/// Structured verdict produced by the Verifier agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifierVerdict {
    /// `true` when all tests passed.
    pub passed: bool,
    /// One-sentence explanation of the verdict.
    pub reason: String,
    /// Names and first-line failure messages of each failing test.
    pub failing_test_summaries: Vec<String>,
    /// Recommended action for the Coder on failure.
    pub recommended_fix: Option<String>,
}

impl VerifierVerdict {
    /// Serialises the verdict to compact JSON.
    ///
    /// # Errors
    ///
    /// Returns `AgentError::PromptAssembly` if JSON serialisation fails.
    pub fn to_json(&self) -> Result<String, AgentError> {
        serde_json::to_string(self)
            .map_err(|json_error| AgentError::PromptAssembly(json_error.to_string()))
    }
}

/// Specialist agent that runs tests and issues an LLM-driven binary verdict.
pub struct VerifierAgent;

impl VerifierAgent {
    /// Creates a new `VerifierAgent`.
    pub fn new() -> Self {
        Self
    }

    async fn run_test_suite(project_root: &std::path::Path) -> String {
        let output_result = tokio::process::Command::new("cargo")
            .args(["test"])
            .current_dir(project_root)
            .output()
            .await;

        match output_result {
            Ok(command_output) => format!(
                "{}\n{}",
                String::from_utf8_lossy(&command_output.stdout),
                String::from_utf8_lossy(&command_output.stderr)
            ),
            Err(spawn_error) => {
                format!("ERROR: could not spawn cargo test: {spawn_error}")
            }
        }
    }

    fn build_verdict_prompt(test_output: &str, task_description: &str) -> Vec<Message> {
        let user_content = format!(
            "## Task\n{task_description}\n\n\
             ## Test Suite Output\n```\n{test_output}\n```\n\n\
             Issue your VERDICT following the output format."
        );

        vec![
            Message {
                role: MessageRole::System,
                content: VERIFIER_SYSTEM_PROMPT.to_owned(),
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

impl Default for VerifierAgent {
    fn default() -> Self {
        Self::new()
    }
}

/// Extracts the project root path from a task description.
///
/// Looks for a `project_root:<path>` prefix in the task description. Falls back
/// to `std::env::current_dir()` when no hint is found.
pub fn extract_project_root_from_task(task_description: &str) -> std::path::PathBuf {
    if let Some(rest) = task_description.strip_prefix("project_root:") {
        let path_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let extracted_path = &rest[..path_end];
        if !extracted_path.is_empty() {
            return std::path::PathBuf::from(extracted_path);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Parses the LLM verdict response into a `VerifierVerdict`.
pub fn parse_verifier_verdict(response_text: &str) -> VerifierVerdict {
    let mut passed = false;
    let mut reason = "no reason provided".to_owned();
    let mut failing_test_summaries: Vec<String> = Vec::new();
    let mut recommended_fix: Option<String> = None;
    let mut in_failing_tests_section = false;

    for response_line in response_text.lines() {
        let trimmed_line = response_line.trim();

        if let Some(verdict_captures) = PATTERN_VERDICT_LINE.captures(trimmed_line) {
            passed = &verdict_captures[1] == "PASS";
            in_failing_tests_section = false;
            continue;
        }

        if let Some(reason_captures) = PATTERN_REASON_LINE.captures(trimmed_line) {
            reason_captures[1].clone_into(&mut reason);
            in_failing_tests_section = false;
            continue;
        }

        if trimmed_line == "FAILING_TESTS:" {
            in_failing_tests_section = true;
            continue;
        }

        if let Some(fix_captures) = PATTERN_RECOMMENDED_FIX_LINE.captures(trimmed_line) {
            recommended_fix = Some(fix_captures[1].to_owned());
            in_failing_tests_section = false;
            continue;
        }

        if in_failing_tests_section {
            if let Some(test_captures) = PATTERN_FAILING_TEST_LINE.captures(trimmed_line) {
                failing_test_summaries
                    .push(format!("{}: {}", &test_captures[1], &test_captures[2]));
            }
        }
    }

    VerifierVerdict {
        passed,
        reason,
        failing_test_summaries,
        recommended_fix,
    }
}

#[async_trait]
impl Agent for VerifierAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::IntegrityChecker
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![AgentCapability::ReadFiles]
    }

    async fn run(&self, task: TaskNode, context: AgentContext) -> Result<TaskResult, AgentError> {
        let _truth_token =
            task.truth_token
                .as_ref()
                .ok_or_else(|| AgentError::MissingTruthToken {
                    task_id: task.id.to_string(),
                })?;

        let project_root = extract_project_root_from_task(&task.description);
        let test_output = Self::run_test_suite(&project_root).await;

        let pre_parse_outcome = parse_test_outcome(&test_output);

        let verdict_messages = Self::build_verdict_prompt(&test_output, &task.description);

        let prompt_token_estimate = verdict_messages
            .iter()
            .map(|message| message.content.len() / 4)
            .sum::<usize>();

        let completion_request = CompletionRequest {
            messages: verdict_messages,
            max_tokens: Some(512),
            temperature: 0.0,
            tools: None,
            stop_sequences: Vec::new(),
        };

        let routed_request = RoutedCompletionRequest {
            required_tier: ModelTier::Tier1,
            completion_request: completion_request.clone(),
        };

        tracing::debug!(
            task_id = %task.id,
            pre_parse_passed = pre_parse_outcome.all_passed,
            token_estimate = prompt_token_estimate,
            "verifier routing verdict request to Tier-1"
        );

        let model_provider = context
            .router
            .route(&routed_request)
            .map_err(|router_error| AgentError::ModelProvider(router_error.to_string()))?;

        let completion_response = model_provider
            .complete(completion_request)
            .await
            .map_err(|provider_error| AgentError::ModelProvider(provider_error.to_string()))?;

        let verdict = parse_verifier_verdict(&completion_response.content);
        let verdict_json = verdict.to_json()?;

        let outcome = if verdict.passed {
            Outcome::Success
        } else {
            Outcome::Failure
        };

        Ok(TaskResult {
            task_id: task.id,
            outcome,
            diff: None,
            summary: verdict_json,
            decision_id: DecisionId::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_verifier_verdict_pass_case() {
        let llm_response = "VERDICT: PASS\nREASON: All 42 tests passed; no regressions detected.";
        let verdict = parse_verifier_verdict(llm_response);
        assert!(verdict.passed);
        assert_eq!(
            verdict.reason,
            "All 42 tests passed; no regressions detected."
        );
        assert!(verdict.failing_test_summaries.is_empty());
    }

    #[test]
    fn parse_verifier_verdict_fail_case_with_failing_tests() {
        let llm_response = "VERDICT: FAIL\n\
            REASON: auth::login_works panicked on missing JWT secret.\n\
            FAILING_TESTS:\n\
            - auth::login_works: thread panicked at src/auth.rs:42\n\
            - auth::logout_clears_session: assertion failed\n\
            RECOMMENDED_FIX: Add JWT_SECRET to the test environment in src/auth/mod.rs.";

        let verdict = parse_verifier_verdict(llm_response);
        assert!(!verdict.passed);
        assert_eq!(verdict.failing_test_summaries.len(), 2);
        assert_eq!(
            verdict.recommended_fix.as_deref(),
            Some("Add JWT_SECRET to the test environment in src/auth/mod.rs.")
        );
    }

    #[test]
    fn extract_project_root_from_explicit_hint() {
        let task_description = "project_root:/tmp/my-project verify all tests pass";
        let project_root = extract_project_root_from_task(task_description);
        assert_eq!(project_root, std::path::PathBuf::from("/tmp/my-project"));
    }

    #[test]
    fn extract_project_root_falls_back_to_current_dir_when_no_hint() {
        let task_description = "verify that all tests pass after JWT refactor";
        let project_root = extract_project_root_from_task(task_description);
        let expected_fallback =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        assert_eq!(project_root, expected_fallback);
    }

    #[test]
    fn verifier_verdict_serialises_to_json() {
        let verdict = VerifierVerdict {
            passed: true,
            reason: "all tests pass".to_owned(),
            failing_test_summaries: Vec::new(),
            recommended_fix: None,
        };
        let json_text = verdict.to_json().expect("serialisation succeeds");
        assert!(json_text.contains("all tests pass"));
    }
}
