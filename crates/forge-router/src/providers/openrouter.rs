//! # forge-router: `OpenRouter` Provider
//!
//! Implements Tier 1 `OpenRouter` calls through its OpenAI-compatible chat
//! completions endpoint. The provider reads `OPENROUTER_API_KEY` by default
//! and uses `openrouter/auto` unless configured otherwise.
//!
//! ## Input
//! - Neutral completion requests and `OpenRouter` API credentials
//!
//! ## Output
//! - OpenAI-compatible requests and neutral completion responses
//!
//! ## Related
//! - `forge-router::translator` — builds OpenAI-compatible JSON
//! - `forge-router::provider` — defines the `ModelProvider` trait

use std::env;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use yantra_core::{ModelCapability, ModelTier};

use crate::error::ProviderError;
use crate::provider::{CompletionRequest, CompletionResponse, ModelProvider, ProviderStatus};
use crate::translator::PromptTranslator;

/// Tier 1 provider backed by `OpenRouter`.
#[derive(Debug, Clone)]
pub struct OpenRouterProvider {
    id: String,
    model: String,
    endpoint: String,
    api_key: String,
    client: Client,
}

impl OpenRouterProvider {
    /// Creates an `OpenRouter` provider from environment variables.
    ///
    /// # Errors
    ///
    /// Returns `ProviderError::MissingConfiguration` when `OPENROUTER_API_KEY`
    /// is unset.
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = env::var("OPENROUTER_API_KEY")
            .map_err(|_| ProviderError::MissingConfiguration("OPENROUTER_API_KEY".to_string()))?;
        Ok(Self::new(
            api_key,
            "openrouter/auto",
            "https://openrouter.ai/api/v1/chat/completions",
        ))
    }

    /// Creates an `OpenRouter` provider with explicit settings.
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Self {
        let model = model.into();
        Self {
            id: format!("openrouter:{model}"),
            model,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            client: Client::new(),
        }
    }
}

#[async_trait]
impl ModelProvider for OpenRouterProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn tier(&self) -> ModelTier {
        ModelTier::Tier1
    }

    fn capability(&self) -> ModelCapability {
        ModelCapability {
            context_limit: 128_000,
            supports_tools: true,
            cost_per_1k_in: 0.0,
            cost_per_1k_out: 0.0,
        }
    }

    async fn status(&self) -> ProviderStatus {
        match self
            .client
            .head(&self.endpoint)
            .bearer_auth(&self.api_key)
            .send()
            .await
        {
            Ok(response) if response.status().as_u16() == 429 => ProviderStatus::RateLimited {
                retry_after: retry_after_from_headers(response.headers()),
            },
            Ok(response) if response.status().is_success() => ProviderStatus::Available,
            Ok(response) => ProviderStatus::Down {
                since: Instant::now(),
                reason: format!("OpenRouter status {}", response.status()),
            },
            Err(error) => ProviderStatus::Down {
                since: Instant::now(),
                reason: error.to_string(),
            },
        }
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        let request_body = PromptTranslator::to_openai(&request, &self.model);
        let response_body = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request_body)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        PromptTranslator::from_openai(&response_body)
    }
}

fn retry_after_from_headers(headers: &reqwest::header::HeaderMap) -> Instant {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map_or_else(
            || Instant::now() + Duration::from_secs(30),
            |seconds| Instant::now() + Duration::from_secs(seconds),
        )
}
