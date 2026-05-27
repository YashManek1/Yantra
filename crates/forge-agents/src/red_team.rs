//! # forge-agents: Red Team Agent
//!
//! LLM-driven security and robustness audit. Receives the Coder's proposed diff,
//! prompts a Tier-1 model to identify security vulnerabilities, edge cases,
//! and concurrency bugs, and returns a binary outcome with structured feedback.
//!
//! ## Input
//! - `TaskNode` carrying a valid `TruthToken`
//! - `AgentContext` with a Tier-1-capable `Router` and `upstream_results` containing the Coder's diff
//!
//! ## Output
//! - `TaskResult` with `outcome = Success` when no major flaws are found,
//!   or `outcome = Failure` (forcing a Coder retry) when vulnerabilities are identified
//!
//! ## Related
//! - `forge-agents::coder` — provides the diff to attack
//! - `forge-orchestrator` — routes tasks and runs the DAG

use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use yantra_core::{AgentCapability, AgentKind, DecisionId, ModelTier, Outcome, TaskNode};
use yantra_router::routing::RoutedCompletionRequest;
use yantra_router::{CompletionRequest, Message, MessageRole};

use crate::agent::{Agent, AgentContext, TaskResult};
use crate::error::AgentError;

const RED_TEAM_SYSTEM_PROMPT: &str = "\
You are the Yantra Red Team Agent. Your role is to perform a security and robustness audit on a proposed code diff.
Analyze the task description, the source truth, and the proposed changes to identify:
1. Security vulnerabilities (e.g., SQL injection, XSS, insecure deserialization, auth bypass, buffer overflows).
2. Edge cases, logic bugs, or memory safety issues.
3. Concurrency issues (e.g., race conditions, deadlocks, data races).

Output format:
Your output must end with a verdict section:
VERDICT: PASS
(if the code has no major security flaws or critical bugs)

Or:
VERDICT: FAIL
REASON: <Single-sentence description of the vulnerability/bug>
RECOMMENDED_FIX: <How to fix it>
";

static PATTERN_VERDICT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^VERDICT:\s*(PASS|FAIL)").expect("verdict pattern is valid"));

static PATTERN_REASON_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^REASON:\s*(.+)").expect("reason pattern is valid"));

static PATTERN_RECOMMENDED_FIX_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^RECOMMENDED_FIX:\s*(.+)").expect("recommended fix pattern is valid")
});

/// Structured verdict produced by the Red Team agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedTeamVerdict {
    /// True when the proposed changes pass security and robustness audits.
    pub passed: bool,
    /// One-sentence explanation of the verdict.
    pub reason: String,
    /// Recommended action for the Coder on failure.
    pub recommended_fix: Option<String>,
}

impl RedTeamVerdict {
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

/// Specialist agent that reviews code for security and concurrency flaws.
pub struct RedTeamAgent;

impl RedTeamAgent {
    /// Creates a new `RedTeamAgent`.
    pub fn new() -> Self {
        Self
    }

    fn build_prompt(diff_text: &str, task_description: &str) -> Vec<Message> {
        let user_content = format!(
            "## Task\n{task_description}\n\n\
             ## Proposed Diff\n```\n{diff_text}\n```\n\n\
             Perform your audit and issue a VERDICT."
        );

        vec![
            Message {
                role: MessageRole::System,
                content: RED_TEAM_SYSTEM_PROMPT.to_owned(),
                tool_calls: Vec::new(),
            },
            Message {
                role: MessageRole::User,
                content: user_content,
                tool_calls: Vec::new(),
            },
        ]
    }

    /// Parses the LLM response into a `RedTeamVerdict`.
    pub fn parse_verdict(response_text: &str) -> RedTeamVerdict {
        let mut passed = true;
        let mut reason = "no issues detected".to_owned();
        let mut recommended_fix = None;

        for response_line in response_text.lines() {
            let trimmed_line = response_line.trim();

            if let Some(verdict_captures) = PATTERN_VERDICT_LINE.captures(trimmed_line) {
                passed = &verdict_captures[1] == "PASS";
                continue;
            }

            if let Some(reason_captures) = PATTERN_REASON_LINE.captures(trimmed_line) {
                reason_captures[1].clone_into(&mut reason);
                continue;
            }

            if let Some(fix_captures) = PATTERN_RECOMMENDED_FIX_LINE.captures(trimmed_line) {
                recommended_fix = Some(fix_captures[1].to_owned());
            }
        }

        RedTeamVerdict {
            passed,
            reason,
            recommended_fix,
        }
    }
}

impl Default for RedTeamAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Agent for RedTeamAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::RedTeam
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

        let coder_diff = context
            .upstream_results
            .iter()
            .find_map(|result| result.diff.as_ref())
            .ok_or_else(|| {
                AgentError::PromptAssembly(
                    "no upstream coder diff found for red teaming".to_owned(),
                )
            })?;

        let mut diff_text = String::new();
        for file_diff in &coder_diff.files {
            diff_text.push_str(&format!(
                "// File: {}\n{}\n",
                file_diff.file_path, file_diff.unified_diff
            ));
        }

        let verdict_messages = Self::build_prompt(&diff_text, &task.description);

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

        let model_provider = context
            .router
            .route(&routed_request)
            .await
            .map_err(|router_error| AgentError::ModelProvider(router_error.to_string()))?;

        let completion_response = model_provider
            .complete(completion_request)
            .await
            .map_err(|provider_error| AgentError::ModelProvider(provider_error.to_string()))?;

        let verdict = Self::parse_verdict(&completion_response.content);
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
