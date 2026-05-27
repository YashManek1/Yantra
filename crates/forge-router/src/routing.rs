//! # forge-router: Routing Policy
//!
//! Classifies tasks into model tiers without model calls and selects a
//! registered provider for the requested tier. The policy is intentionally
//! mechanical so model selection remains auditable and deterministic.
//!
//! ## Input
//! - Task metadata, token estimates, tool-call estimates, and provider lists
//!
//! ## Output
//! - `ModelTier` classifications and selected `ModelProvider` references
//!
//! ## Related
//! - `forge-router::provider` — provider trait selected by the router
//! - `forge-core::task` — supplies task classes

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yantra_core::{ModelTier, TaskClass};

use crate::error::{RouterError, RouterResult};
use crate::provider::{CompletionRequest, ModelProvider};

/// Routing policy loaded from configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// Maximum tokens for Tier 0 local routing.
    pub tier0_token_limit: usize,
    /// Maximum predicted tool calls for Tier 0 local routing.
    pub tier0_tool_call_limit: usize,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            tier0_token_limit: 2_000,
            tier0_tool_call_limit: 3,
        }
    }
}

impl RoutingPolicy {
    /// Loads routing policy from a TOML-like config file.
    ///
    /// # Errors
    ///
    /// Returns `RouterError::PolicyConfiguration` when the file cannot be read
    /// or a numeric key cannot be parsed.
    pub fn from_file(path: &Path) -> RouterResult<Self> {
        let config_text = fs::read_to_string(path)
            .map_err(|error| RouterError::PolicyConfiguration(error.to_string()))?;
        Self::from_config_text(&config_text)
    }

    /// Parses routing policy from TOML-like text.
    ///
    /// # Errors
    ///
    /// Returns `RouterError::PolicyConfiguration` when a numeric key cannot be
    /// parsed.
    pub fn from_config_text(config_text: &str) -> RouterResult<Self> {
        let mut policy = Self::default();
        for line in config_text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('"');
                match key {
                    "tier0_token_limit" => {
                        policy.tier0_token_limit = value.parse().map_err(|error| {
                            RouterError::PolicyConfiguration(format!(
                                "invalid tier0_token_limit: {error}"
                            ))
                        })?;
                    }
                    "tier0_tool_call_limit" => {
                        policy.tier0_tool_call_limit = value.parse().map_err(|error| {
                            RouterError::PolicyConfiguration(format!(
                                "invalid tier0_tool_call_limit: {error}"
                            ))
                        })?;
                    }
                    _ => {}
                }
            }
        }
        Ok(policy)
    }

    /// Classifies a task into a model tier.
    pub fn classify(&self, task: &TaskDescription) -> ModelTier {
        if matches!(task.class, TaskClass::Migration | TaskClass::Integration)
            || (task.class == TaskClass::NewFeature && task.touches_sacred_files)
        {
            return ModelTier::Tier3;
        }

        if task.class == TaskClass::Refactor && task.multi_file {
            return ModelTier::Tier2;
        }

        if task.tokens_estimated < self.tier0_token_limit
            && task.tool_calls_predicted < self.tier0_tool_call_limit
        {
            return ModelTier::Tier0;
        }

        ModelTier::Tier1
    }
}

/// Metadata used by the pure routing classifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDescription {
    /// Natural language task text.
    pub description: String,
    /// Task class assigned by `STVP`.
    pub class: TaskClass,
    /// Estimated input tokens.
    pub tokens_estimated: usize,
    /// Predicted number of tool calls.
    pub tool_calls_predicted: usize,
    /// Whether sacred files are involved.
    pub touches_sacred_files: bool,
    /// Whether the task spans multiple files.
    pub multi_file: bool,
}

/// Router holding registered providers.
#[derive(Clone)]
pub struct Router {
    policy: RoutingPolicy,
    providers: Vec<Arc<dyn ModelProvider>>,
}

impl Router {
    /// Creates a router from a policy and provider list.
    pub fn new(policy: RoutingPolicy, providers: Vec<Arc<dyn ModelProvider>>) -> Self {
        Self { policy, providers }
    }

    /// Returns the policy used by this router.
    pub fn policy(&self) -> &RoutingPolicy {
        &self.policy
    }

    /// Selects a provider for a request tier.
    ///
    /// # Errors
    ///
    /// Returns `RouterError::NoProvider` when no provider for the tier exists.
    pub async fn route(
        &self,
        request: &RoutedCompletionRequest,
    ) -> RouterResult<&dyn ModelProvider> {
        let requested_tier = request.required_tier;
        let tiers_to_try = match requested_tier {
            ModelTier::Tier0 => vec![
                ModelTier::Tier0,
                ModelTier::Tier1,
                ModelTier::Tier2,
                ModelTier::Tier3,
            ],
            ModelTier::Tier1 => vec![
                ModelTier::Tier1,
                ModelTier::Tier2,
                ModelTier::Tier3,
                ModelTier::Tier0,
            ],
            ModelTier::Tier2 => vec![
                ModelTier::Tier2,
                ModelTier::Tier3,
                ModelTier::Tier1,
                ModelTier::Tier0,
            ],
            ModelTier::Tier3 => vec![
                ModelTier::Tier3,
                ModelTier::Tier2,
                ModelTier::Tier1,
                ModelTier::Tier0,
            ],
        };

        for &tier in &tiers_to_try {
            for provider in &self.providers {
                if provider.tier() == tier {
                    let provider_status = provider.status().await;
                    if matches!(provider_status, crate::provider::ProviderStatus::Available) {
                        return Ok(provider.as_ref());
                    }
                }
            }
        }

        for provider in &self.providers {
            if provider.tier() == requested_tier {
                return Ok(provider.as_ref());
            }
        }

        Err(RouterError::NoProvider(requested_tier))
    }

    /// Selects a provider for a task by first classifying the task.
    ///
    /// # Errors
    ///
    /// Returns `RouterError::NoProvider` when no provider for the classified
    /// tier exists.
    pub async fn route_task(
        &self,
        task: &TaskDescription,
        completion_request: CompletionRequest,
    ) -> RouterResult<&dyn ModelProvider> {
        let required_tier = self.policy.classify(task);
        self.route(&RoutedCompletionRequest {
            required_tier,
            completion_request,
        })
        .await
    }

    /// Returns cloned provider handles for background health polling.
    pub fn providers(&self) -> Vec<Arc<dyn ModelProvider>> {
        self.providers.clone()
    }
}

/// Completion request after routing classification.
#[derive(Debug, Clone)]
pub struct RoutedCompletionRequest {
    /// Required tier.
    pub required_tier: ModelTier,
    /// Neutral completion request.
    pub completion_request: CompletionRequest,
}
