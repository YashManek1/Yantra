//! # forge-orchestrator: Dependency Inference
//!
//! Makes a single Tier-1 LLM call to infer inter-task dependency edges from
//! a batch of task descriptions. The result is a JSON adjacency list that is
//! applied to the `TaskDag` by the caller.
//!
//! ## Input
//! - A slice of `(TaskId, description)` pairs representing tasks to analyse
//! - A `Router` configured with at least one Tier-1 provider
//!
//! ## Output
//! - `HashMap<TaskId, Vec<TaskId>>` where each key task depends on its values
//!
//! ## Related
//! - `forge-router` — provides `Router` and `RoutedCompletionRequest`
//! - `forge-orchestrator::dag` — receives inferred edges via `add_dependency`

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use yantra_core::{ModelTier, TaskId};
use yantra_router::{CompletionRequest, Message, MessageRole, RoutedCompletionRequest, Router};

use crate::error::OrchestratorError;

/// Infers dependency edges between tasks using a Tier-1 LLM call.
pub struct DependencyInferrer {
    router: Arc<Router>,
}

impl DependencyInferrer {
    /// Creates an inferrer backed by the given router.
    pub fn new(router: Arc<Router>) -> Self {
        Self { router }
    }

    /// Calls a Tier-1 model once to infer dependencies among the given tasks.
    ///
    /// Returns a map where `result[A] = [B, C]` means task A depends on
    /// tasks B and C (B and C must complete before A can start).
    ///
    /// Silently returns an empty map when the model response cannot be parsed
    /// as the expected JSON adjacency list.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Router` when no Tier-1 provider is
    /// registered, or `OrchestratorError::Provider` on a model call failure.
    pub async fn infer(
        &self,
        tasks: &[(TaskId, String)],
    ) -> Result<HashMap<TaskId, Vec<TaskId>>, OrchestratorError> {
        if tasks.is_empty() {
            return Ok(HashMap::new());
        }

        let prompt_content = build_inference_prompt(tasks);
        let completion_request = CompletionRequest {
            messages: vec![
                Message {
                    role: MessageRole::System,
                    content: SYSTEM_PROMPT.to_owned(),
                    tool_calls: vec![],
                },
                Message {
                    role: MessageRole::User,
                    content: prompt_content,
                    tool_calls: vec![],
                },
            ],
            max_tokens: Some(1_024),
            temperature: 0.0,
            tools: None,
            stop_sequences: vec![],
        };

        let routed_request = RoutedCompletionRequest {
            required_tier: ModelTier::Tier1,
            completion_request: completion_request.clone(),
        };
        let provider = self
            .router
            .route(&routed_request)
            .await
            .map_err(|router_error| OrchestratorError::Router(router_error.to_string()))?;
        let response = provider
            .complete(completion_request)
            .await
            .map_err(|provider_error| OrchestratorError::Provider(provider_error.to_string()))?;

        Ok(parse_adjacency_response(&response.content, tasks))
    }
}

const SYSTEM_PROMPT: &str = "\
You are a dependency analyser for software engineering tasks. \
Given a JSON array of tasks, return a JSON object where each key is a task ID \
and the value is an array of task IDs that the key task depends on. \
Only include tasks that have at least one dependency. \
Return only valid JSON with no explanation or markdown.";

fn build_inference_prompt(tasks: &[(TaskId, String)]) -> String {
    let task_objects: Vec<serde_json::Value> = tasks
        .iter()
        .map(|(task_id, description)| {
            serde_json::json!({
                "id": task_id.to_string(),
                "description": description,
            })
        })
        .collect();
    format!(
        "Tasks:\n{}",
        serde_json::to_string_pretty(&task_objects).unwrap_or_default()
    )
}

fn parse_adjacency_response(
    response_content: &str,
    known_tasks: &[(TaskId, String)],
) -> HashMap<TaskId, Vec<TaskId>> {
    let known_id_strings: std::collections::HashSet<String> = known_tasks
        .iter()
        .map(|(task_id, _)| task_id.to_string())
        .collect();

    let parsed_json: serde_json::Value =
        if let Ok(json_value) = serde_json::from_str(response_content.trim()) {
            json_value
        } else {
            tracing::warn!("dependency inference returned non-JSON response; using empty graph");
            return HashMap::new();
        };

    let adjacency_object = if let Some(object) = parsed_json.as_object() {
        object
    } else {
        tracing::warn!("dependency inference returned non-object JSON; using empty graph");
        return HashMap::new();
    };

    let mut dependencies = HashMap::new();
    for (dependent_id_str, dep_array) in adjacency_object {
        if !known_id_strings.contains(dependent_id_str.as_str()) {
            continue;
        }
        let dependent_id = match TaskId::from_str(dependent_id_str) {
            Ok(parsed_id) => parsed_id,
            Err(_) => continue,
        };
        let dep_list = match dep_array.as_array() {
            Some(array) => array,
            None => continue,
        };
        let mut resolved_deps = Vec::new();
        for dep_value in dep_list {
            let dep_id_str = match dep_value.as_str() {
                Some(string_value) => string_value,
                None => continue,
            };
            if !known_id_strings.contains(dep_id_str) {
                continue;
            }
            if let Ok(dep_id) = TaskId::from_str(dep_id_str) {
                resolved_deps.push(dep_id);
            }
        }
        if !resolved_deps.is_empty() {
            dependencies.insert(dependent_id, resolved_deps);
        }
    }

    dependencies
}
