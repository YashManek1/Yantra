//! # forge-agents: Researcher Agent
//!
//! Gathers external knowledge relevant to a task by querying three MCP data
//! sources (search, browser, knowledge-graph), then asks a Tier-1 LLM to
//! synthesise the raw results into a confidence-scored `ResearchMemo`. The
//! Researcher never produces code or diffs.
//!
//! ## Input
//! - `TaskNode` carrying a valid `TruthToken`
//! - `AgentContext` with an `McpRouter` that MAY have `search`, `browser`, and
//!   `kg` servers registered (missing servers are silently skipped)
//!
//! ## Output
//! - `TaskResult` with `outcome = Success` and `summary` containing a serialised
//!   `ResearchMemo`; `diff` is always `None`
//!
//! ## Related
//! - `forge-agents::agent` — `Agent` trait + shared types
//! - `forge-router` — Tier-1 LLM call for synthesis
//! - `forge-orchestrator` — dispatches the Researcher before the Coder

use std::sync::LazyLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use yantra_core::{AgentCapability, AgentKind, DecisionId, Outcome, TaskId, TaskNode};
use yantra_router::routing::{RoutedCompletionRequest, TaskDescription};
use yantra_router::{CompletionRequest, Message, MessageRole};

use crate::agent::{Agent, AgentContext, TaskResult};
use crate::error::AgentError;

const RESEARCHER_SYSTEM_PROMPT: &str = include_str!("../prompts/researcher.md");

static PATTERN_FINDING_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^FINDING:\s*(.+?)\s*\|\s*confidence:\s*([\d.]+)(?:\s*\|\s*source:\s*(\S+))?(?:\s*\|\s*url:\s*(\S+))?")
        .expect("finding pattern is valid")
});

static PATTERN_CONFIDENCE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^CONFIDENCE:\s*([\d.]+)").expect("confidence pattern is valid"));

static PATTERN_SUMMARY_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^SUMMARY:\s*(.+)").expect("summary pattern is valid"));

/// A single evidence-backed claim produced by the Researcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchFinding {
    /// The factual claim the agent is asserting.
    pub claim: String,
    /// Confidence in this claim on a 0.0–1.0 scale.
    pub confidence: f32,
    /// MCP tool name that provided the evidence.
    pub source_tool: String,
    /// URL of the source document, if any.
    pub source_url: Option<String>,
}

/// Complete research output returned by the Researcher agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchMemo {
    /// Task this research was conducted for.
    pub task_id: TaskId,
    /// Unique decision identifier for the archaeology log.
    pub decision_id: DecisionId,
    /// One-sentence synthesis of the most important finding.
    pub summary: String,
    /// All evidence-backed findings, ordered by confidence descending.
    pub findings: Vec<ResearchFinding>,
    /// Aggregate confidence across all findings (arithmetic mean).
    pub overall_confidence: f32,
    /// Wall-clock time the memo was assembled.
    pub created_at: DateTime<Utc>,
}

impl ResearchMemo {
    /// Serialises the memo to a compact JSON string.
    ///
    /// # Errors
    ///
    /// Returns `AgentError::PromptAssembly` if JSON serialisation fails.
    pub fn to_json(&self) -> Result<String, AgentError> {
        serde_json::to_string(self)
            .map_err(|json_error| AgentError::PromptAssembly(json_error.to_string()))
    }
}

/// Specialist agent that gathers and synthesises research for a task.
pub struct ResearcherAgent;

impl ResearcherAgent {
    /// Creates a new `ResearcherAgent`.
    pub fn new() -> Self {
        Self
    }

    async fn gather_tool_data(
        task_description: &str,
        context: &AgentContext,
    ) -> Vec<(String, Value)> {
        let mut tool_results: Vec<(String, Value)> = Vec::new();

        if let Ok(search_result) = context
            .tools
            .handle(
                "search.query",
                json!({ "query": task_description, "limit": 5 }),
            )
            .await
        {
            tool_results.push(("search".to_owned(), search_result));
        }

        if let Ok(kg_result) = context
            .tools
            .handle("kg.query", json!({ "query": task_description }))
            .await
        {
            tool_results.push(("kg".to_owned(), kg_result));
        }

        tool_results
    }

    fn build_synthesis_prompt(
        task_description: &str,
        tool_results: &[(String, Value)],
    ) -> Vec<Message> {
        let tool_data_text = if tool_results.is_empty() {
            "No tool results available.".to_owned()
        } else {
            tool_results
                .iter()
                .map(|(tool_name, tool_value)| {
                    format!(
                        "## {tool_name}\n{}",
                        serde_json::to_string_pretty(tool_value).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };

        let user_content = format!(
            "## Task\n{task_description}\n\n\
             ## Raw Tool Output\n{tool_data_text}\n\n\
             Synthesise the above into a research memo following your output format."
        );

        vec![
            Message {
                role: MessageRole::System,
                content: RESEARCHER_SYSTEM_PROMPT.to_owned(),
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

impl Default for ResearcherAgent {
    fn default() -> Self {
        Self::new()
    }
}

/// Parses an LLM response into a `ResearchMemo`.
pub fn parse_research_memo(
    response_text: &str,
    task_id: TaskId,
    decision_id: DecisionId,
) -> ResearchMemo {
    let mut summary = String::new();
    let mut overall_confidence = 0.5_f32;
    let mut findings: Vec<ResearchFinding> = Vec::new();

    for response_line in response_text.lines() {
        let trimmed_line = response_line.trim();

        if let Some(summary_captures) = PATTERN_SUMMARY_LINE.captures(trimmed_line) {
            summary_captures[1].clone_into(&mut summary);
            continue;
        }

        if let Some(confidence_captures) = PATTERN_CONFIDENCE_LINE.captures(trimmed_line) {
            overall_confidence = confidence_captures[1]
                .parse::<f32>()
                .unwrap_or(0.5)
                .clamp(0.0, 1.0);
            continue;
        }

        if let Some(finding_captures) = PATTERN_FINDING_LINE.captures(trimmed_line) {
            let claim = finding_captures[1].to_owned();
            let confidence = finding_captures[2]
                .parse::<f32>()
                .unwrap_or(0.5)
                .clamp(0.0, 1.0);
            let source_tool = finding_captures.get(3).map_or_else(
                || "unknown".to_owned(),
                |capture| capture.as_str().to_owned(),
            );
            let source_url = finding_captures
                .get(4)
                .map(|capture| capture.as_str().to_owned());

            findings.push(ResearchFinding {
                claim,
                confidence,
                source_tool,
                source_url,
            });
        }
    }

    if summary.is_empty() {
        response_text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no summary available")
            .trim()
            .clone_into(&mut summary);
    }

    if !findings.is_empty() {
        overall_confidence = findings
            .iter()
            .map(|finding| finding.confidence)
            .sum::<f32>()
            / findings.len() as f32;
    }

    findings.sort_by(|left_finding, right_finding| {
        right_finding
            .confidence
            .partial_cmp(&left_finding.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    ResearchMemo {
        task_id,
        decision_id,
        summary,
        findings,
        overall_confidence,
        created_at: Utc::now(),
    }
}

#[async_trait]
impl Agent for ResearcherAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Researcher
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![AgentCapability::AccessNetwork, AgentCapability::ReadFiles]
    }

    async fn run(&self, task: TaskNode, context: AgentContext) -> Result<TaskResult, AgentError> {
        let _truth_token =
            task.truth_token
                .as_ref()
                .ok_or_else(|| AgentError::MissingTruthToken {
                    task_id: task.id.to_string(),
                })?;

        let tool_results = Self::gather_tool_data(&task.description, &context).await;

        let synthesis_messages = Self::build_synthesis_prompt(&task.description, &tool_results);

        let prompt_token_estimate = synthesis_messages
            .iter()
            .map(|message| message.content.len() / 4)
            .sum::<usize>();

        let completion_request = CompletionRequest {
            messages: synthesis_messages,
            max_tokens: Some(1024),
            temperature: 0.3,
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
            "researcher routing synthesis request to Tier-1"
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

        let decision_id = DecisionId::new();
        let research_memo = parse_research_memo(&completion_response.content, task.id, decision_id);

        let memo_json = research_memo.to_json()?;

        Ok(TaskResult {
            task_id: task.id,
            outcome: Outcome::Success,
            diff: None,
            summary: memo_json,
            decision_id: research_memo.decision_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use yantra_core::TaskId;

    use super::*;

    #[test]
    fn parse_research_memo_extracts_summary_and_findings() {
        let response_text = "SUMMARY: JWT rotation should use the existing token module.\n\
            CONFIDENCE: 0.8\n\
            FINDING: sign_jwt() exists in src/auth/token.rs | confidence: 0.9 | source: kg\n\
            FINDING: jwt-simple crate provides rotate() API | confidence: 0.7 | source: search | url: docs.rs/jwt-simple";

        let task_id = TaskId::new();
        let decision_id = DecisionId::new();
        let research_memo = parse_research_memo(response_text, task_id, decision_id);

        assert_eq!(
            research_memo.summary,
            "JWT rotation should use the existing token module."
        );
        assert_eq!(research_memo.findings.len(), 2);
        assert!((research_memo.findings[0].confidence - 0.9).abs() < f32::EPSILON);
        assert_eq!(research_memo.findings[0].source_tool, "kg");
        assert_eq!(
            research_memo.findings[1].source_url.as_deref(),
            Some("docs.rs/jwt-simple")
        );
    }

    #[test]
    fn parse_research_memo_handles_empty_response() {
        let task_id = TaskId::new();
        let decision_id = DecisionId::new();
        let research_memo = parse_research_memo("", task_id, decision_id);
        assert!(!research_memo.summary.is_empty());
        assert!(research_memo.findings.is_empty());
    }

    #[test]
    fn parse_research_memo_sorts_findings_by_confidence_descending() {
        let response_text = "SUMMARY: test\n\
            FINDING: low confidence claim | confidence: 0.3 | source: search\n\
            FINDING: high confidence claim | confidence: 0.9 | source: kg\n\
            FINDING: medium confidence claim | confidence: 0.6 | source: search";

        let task_id = TaskId::new();
        let decision_id = DecisionId::new();
        let research_memo = parse_research_memo(response_text, task_id, decision_id);

        assert!((research_memo.findings[0].confidence - 0.9).abs() < f32::EPSILON);
        assert!((research_memo.findings[1].confidence - 0.6).abs() < f32::EPSILON);
        assert!((research_memo.findings[2].confidence - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn research_memo_serialises_to_json() {
        let task_id = TaskId::new();
        let decision_id = DecisionId::new();
        let research_memo = parse_research_memo(
            "SUMMARY: test summary\nCONFIDENCE: 0.7",
            task_id,
            decision_id,
        );
        let json_output = research_memo.to_json().expect("serialisation succeeds");
        assert!(json_output.contains("test summary"));
    }
}
