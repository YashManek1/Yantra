//! # forge-router: GitHub Models Provider
//!
//! Implements Tier 1 calls to GitHub Models through the Azure-hosted `OpenAI`
//! compatible endpoint. The provider reads `GITHUB_TOKEN` by default.
//!
//! ## Input
//! - Neutral completion requests and GitHub token credentials
//!
//! ## Output
//! - OpenAI-compatible requests and neutral completion responses
//!
//! ## Related
//! - `forge-router::translator` — builds OpenAI-compatible JSON
//! - `forge-router::provider` — defines the `ModelProvider` trait

use std::env;
use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use yantra_core::{ModelCapability, ModelTier};

use crate::error::ProviderError;
use crate::provider::{CompletionRequest, CompletionResponse, ModelProvider, ProviderStatus};
use crate::translator::PromptTranslator;

/// Tier 1 provider backed by GitHub Models.
#[derive(Debug, Clone)]
pub struct GitHubModelsProvider {
    id: String,
    model: String,
    endpoint: String,
    api_key: String,
    client: Client,
}

impl GitHubModelsProvider {
    /// Creates a GitHub Models provider from environment variables.
    ///
    /// # Errors
    ///
    /// Returns `ProviderError::MissingConfiguration` when `GITHUB_TOKEN` is
    /// unset.
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = env::var("GITHUB_TOKEN")
            .map_err(|_| ProviderError::MissingConfiguration("GITHUB_TOKEN".to_string()))?;
        Ok(Self::new(
            api_key,
            "gpt-4o-mini",
            "https://models.inference.ai.azure.com/chat/completions",
        ))
    }

    /// Creates a GitHub Models provider with explicit settings.
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Self {
        let model = model.into();
        Self {
            id: format!("github-models:{model}"),
            model,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            client: Client::new(),
        }
    }
}

#[async_trait]
impl ModelProvider for GitHubModelsProvider {
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
            Ok(response) if response.status().is_success() => ProviderStatus::Available,
            Ok(response) => ProviderStatus::Down {
                since: Instant::now(),
                reason: format!("GitHub Models status {}", response.status()),
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
