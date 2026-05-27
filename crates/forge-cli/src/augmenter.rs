//! # yantra-cli: Questionnaire Augmenter Implementation
//!
//! Implements the `AugmenterPort` trait, delegating dynamic question generation
//! to a Tier 0 or Tier 1 model via the Yantra prompt router. Uses strict JSON schema
//! prompting and fallback parsing to fail safely on malformed model responses.
//!
//! ## Input
//! - `task_description: &str` — natural-language task description
//! - `task_class: TaskClass` — classified task class
//! - `existing_question_ids: Vec<String>` — question IDs already queued to prevent duplication
//! - `max_count: usize` — maximum number of questions to generate
//!
//! ## Output
//! - `Vec<(String, String, bool)>` — dynamic question triples: (id, text, required)
//!
//! ## Related
//! - `yantra-router` — routes prompt completions to configured LLMs
//! - `forge-core::task::AugmenterPort` — target trait being implemented

use serde::Deserialize;
use std::sync::Arc;
use yantra_core::{AugmenterPort, ModelTier, TaskClass};
use yantra_router::routing::RoutedCompletionRequest;
use yantra_router::{CompletionRequest, Message, MessageRole, Router};

#[derive(Debug, Deserialize)]
struct GeneratedQuestionJson {
    id: String,
    text: String,
    required: bool,
    suggested_answer: Option<String>,
    help_text: Option<String>,
}

/// Dynamic question generator driven by LLM completions.
pub struct CliQuestionAugmenter {
    router: Arc<Router>,
}

impl CliQuestionAugmenter {
    /// Creates a new `CliQuestionAugmenter` wrapping a `Router` instance.
    pub fn new(router: Arc<Router>) -> Self {
        Self { router }
    }
}

impl AugmenterPort for CliQuestionAugmenter {
    fn augment(
        &self,
        task_description: &str,
        task_class: TaskClass,
        existing_question_ids: Vec<String>,
        _max_count: usize,
    ) -> yantra_core::AugmentFuture {
        let router = self.router.clone();
        let description = task_description.to_owned();

        Box::pin(async move {
            let existing_list = existing_question_ids.join(", ");
            let system_prompt = "You are the Yantra Interrogator questionnaire designer. Your job is to generate a highly focused, task-specific developer question to gather missing implementation details.".to_string();

            let user_content = format!(
                "Task Description: \"{description}\"\n\
                 Task Class: \"{task_class:?}\"\n\n\
                 Existing questions already covered (Do NOT duplicate or ask similar questions): [{existing_list}]\n\n\
                 Generate exactly one highly relevant question to gather missing technical context about this task.\n\
                 Provide a customized \"suggested_answer\" that is a realistic developer response (e.g. concrete code pattern or config name) based on the task description,\n\
                 and a descriptive \"help_text\" explaining why this detail is needed.\n\n\
                 Respond with a single valid JSON object containing exactly these fields, and absolutely NO other text, markdown, or backticks:\n\
                 {{\n  \"id\": \"snake_case_identifier\",\n  \"text\": \"Question text?\",\n  \"required\": false,\n  \"suggested_answer\": \"a realistic default answer matching this task context\",\n  \"help_text\": \"explanation of why this parameter is important\"\n}}"
            );

            let messages = vec![
                Message {
                    role: MessageRole::System,
                    content: system_prompt,
                    tool_calls: Vec::new(),
                },
                Message {
                    role: MessageRole::User,
                    content: user_content,
                    tool_calls: Vec::new(),
                },
            ];

            let completion_request = CompletionRequest {
                messages,
                max_tokens: Some(256),
                temperature: 0.1,
                tools: None,
                stop_sequences: Vec::new(),
            };

            let routed_request = RoutedCompletionRequest {
                required_tier: ModelTier::Tier0,
                completion_request,
            };

            let provider = match router.route(&routed_request).await {
                Ok(p) => p,
                Err(_) => return Vec::new(),
            };

            let response_result = provider.complete(routed_request.completion_request).await;
            let response = match response_result {
                Ok(res) => res,
                Err(_) => return Vec::new(),
            };

            // Clean the output (strip backticks if model ignored instruction)
            let raw_content = response.content.trim();
            let clean_json = if raw_content.starts_with("```") {
                let lines: Vec<&str> = raw_content.lines().collect();
                if lines.len() >= 2
                    && lines[0].starts_with("```")
                    && lines[lines.len() - 1] == "```"
                {
                    lines[1..lines.len() - 1].join("\n")
                } else {
                    raw_content.to_string()
                }
            } else {
                raw_content.to_string()
            };

            let parsed: Result<GeneratedQuestionJson, _> = serde_json::from_str(&clean_json);
            match parsed {
                Ok(question) => {
                    vec![(
                        question.id,
                        question.text,
                        question.required,
                        question.suggested_answer,
                        question.help_text,
                    )]
                }
                Err(_) => Vec::new(),
            }
        })
    }
}
