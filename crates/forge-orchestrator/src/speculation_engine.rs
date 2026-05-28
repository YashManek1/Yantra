//! # forge-orchestrator: Speculation Engine
//!
//! After a task completes, predicts the top-3 most likely next tasks using a
//! Tier-0 (local/free) model. Predictions are cached in a 128-entry LRU cache
//! keyed on normalised task descriptions to avoid redundant model calls.
//!
//! The hit-rate target is 60 % when the same task type recurs across sessions.
//!
//! ## Input
//! - The description of a just-completed task
//! - A `Router` with at least one Tier-0 provider registered
//!
//! ## Output
//! - `Vec<SpeculatedTask>` — up to 3 predicted next-task descriptions and classes
//!
//! ## Related
//! - `forge-router` — supplies Tier-0 model access
//! - `forge-orchestrator::scheduler` — calls `predict_next` after `TaskCompleted`

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use yantra_core::{ModelTier, TaskClass};
use yantra_router::{CompletionRequest, Message, MessageRole, RoutedCompletionRequest, Router};

use crate::error::OrchestratorError;

/// LRU cache capacity: 128 entries.
const CACHE_CAPACITY: usize = 128;

/// Maximum number of task predictions returned per call.
const MAX_PREDICTIONS: usize = 3;

/// A task predicted by the speculation engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeculatedTask {
    /// Natural language description of the predicted task.
    pub description: String,
    /// Task class inferred for the predicted task.
    pub predicted_class: TaskClass,
}

struct LruCache {
    capacity: usize,
    entries: HashMap<String, Vec<SpeculatedTask>>,
    access_order: VecDeque<String>,
}

impl LruCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            access_order: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<Vec<SpeculatedTask>> {
        if !self.entries.contains_key(key) {
            return None;
        }
        self.access_order.retain(|existing_key| existing_key != key);
        self.access_order.push_back(key.to_owned());
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: String, predictions: Vec<SpeculatedTask>) {
        if self.entries.contains_key(&key) {
            self.access_order
                .retain(|existing_key| existing_key != &key);
        } else if self.entries.len() >= self.capacity {
            if let Some(evicted_key) = self.access_order.pop_front() {
                self.entries.remove(&evicted_key);
            }
        }
        self.entries.insert(key.clone(), predictions);
        self.access_order.push_back(key);
    }
}

/// Speculation engine that pre-generates likely follow-up tasks.
///
/// Thread-safe via `Arc<Mutex<LruCache>>`. Multiple callers may share one
/// instance.
pub struct SpeculationEngine {
    router: Arc<Router>,
    cache: Mutex<LruCache>,
}

impl SpeculationEngine {
    /// Creates a speculation engine backed by the given router.
    pub fn new(router: Arc<Router>) -> Self {
        Self {
            router,
            cache: Mutex::new(LruCache::new(CACHE_CAPACITY)),
        }
    }

    /// Predicts the top-3 tasks likely to follow the completed description.
    ///
    /// Checks the cache first. On a cache miss, calls the Tier-0 model and
    /// stores the result before returning.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Router` when no Tier-0 provider is
    /// registered, or `OrchestratorError::Provider` on a model call failure.
    pub async fn predict_next(
        &self,
        completed_description: &str,
    ) -> Result<Vec<SpeculatedTask>, OrchestratorError> {
        let normalised_key = normalise_description(completed_description);

        if let Some(cached_predictions) = self.cache.lock().get(&normalised_key) {
            return Ok(cached_predictions);
        }

        let predictions = self.call_tier0_model(completed_description).await?;

        self.cache
            .lock()
            .insert(normalised_key, predictions.clone());

        Ok(predictions)
    }
}

impl SpeculationEngine {
    async fn call_tier0_model(
        &self,
        completed_description: &str,
    ) -> Result<Vec<SpeculatedTask>, OrchestratorError> {
        let prompt_content = build_speculation_prompt(completed_description);
        let completion_request = CompletionRequest {
            messages: vec![
                Message {
                    role: MessageRole::System,
                    content: SPECULATION_SYSTEM_PROMPT.to_owned(),
                    tool_calls: vec![],
                },
                Message {
                    role: MessageRole::User,
                    content: prompt_content,
                    tool_calls: vec![],
                },
            ],
            max_tokens: Some(512),
            temperature: 0.3,
            tools: None,
            stop_sequences: vec![],
        };

        let routed_request = RoutedCompletionRequest {
            required_tier: ModelTier::Tier0,
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

        Ok(parse_speculation_response(&response.content))
    }
}

const SPECULATION_SYSTEM_PROMPT: &str = "\
You are a software engineering task predictor. Given a completed task, predict \
the top 3 most likely follow-up tasks. Return a JSON array of objects with \
\"description\" (string) and \"class\" (one of: new_feature, bug_fix, refactor, \
migration, integration, exploration, chore, docstring, style) fields. \
Return only valid JSON, no explanation or markdown.";

fn build_speculation_prompt(completed_description: &str) -> String {
    format!("Completed task: {completed_description}")
}

fn normalise_description(description: &str) -> String {
    description
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_speculation_response(response_content: &str) -> Vec<SpeculatedTask> {
    let parsed_json: serde_json::Value = if let Ok(json_value) =
        serde_json::from_str(response_content.trim())
    {
        json_value
    } else {
        tracing::warn!("speculation engine returned non-JSON response; using empty predictions");
        return vec![];
    };

    let prediction_array = if let Some(array) = parsed_json.as_array() {
        array
    } else {
        tracing::warn!("speculation engine returned non-array JSON; using empty predictions");
        return vec![];
    };

    prediction_array
        .iter()
        .take(MAX_PREDICTIONS)
        .filter_map(parse_single_prediction)
        .collect()
}

fn parse_single_prediction(prediction_value: &serde_json::Value) -> Option<SpeculatedTask> {
    let prediction_object = prediction_value.as_object()?;
    let description = prediction_object.get("description")?.as_str()?.to_owned();
    let class_string = prediction_object.get("class")?.as_str()?;
    let predicted_class = match class_string {
        "new_feature" => TaskClass::NewFeature,
        "bug_fix" => TaskClass::BugFix,
        "refactor" => TaskClass::Refactor,
        "migration" => TaskClass::Migration,
        "integration" => TaskClass::Integration,
        "exploration" => TaskClass::Exploration,
        "chore" => TaskClass::Chore,
        "docstring" => TaskClass::Docstring,
        "style" => TaskClass::Style,
        _ => TaskClass::Chore,
    };
    Some(SpeculatedTask {
        description,
        predicted_class,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_description_strips_punctuation_and_lowercases() {
        let normalised = normalise_description("Add JWT! Rotation, Now.");
        assert_eq!(normalised, "add jwt rotation now");
    }

    #[test]
    fn lru_cache_evicts_oldest_entry_at_capacity() {
        let mut cache = LruCache::new(2);
        cache.insert("a".to_owned(), vec![]);
        cache.insert("b".to_owned(), vec![]);
        cache.insert("c".to_owned(), vec![]);
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_some());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn lru_cache_refreshes_access_order_on_get() {
        let mut cache = LruCache::new(2);
        cache.insert("a".to_owned(), vec![]);
        cache.insert("b".to_owned(), vec![]);
        let _ = cache.get("a");
        cache.insert("c".to_owned(), vec![]);
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn parse_speculation_response_ignores_invalid_json() {
        let predictions = parse_speculation_response("not json");
        assert!(predictions.is_empty());
    }

    #[test]
    fn parse_speculation_response_extracts_valid_entries() {
        let json = r#"[
            {"description": "Write unit tests", "class": "chore"},
            {"description": "Update docs", "class": "docstring"}
        ]"#;
        let predictions = parse_speculation_response(json);
        assert_eq!(predictions.len(), 2);
        assert_eq!(predictions[0].predicted_class, TaskClass::Chore);
        assert_eq!(predictions[1].predicted_class, TaskClass::Docstring);
    }
}
