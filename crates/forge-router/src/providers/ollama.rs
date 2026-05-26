//! # forge-router: Ollama Provider
//!
//! Implements Tier 0 local model calls against Ollama's `/api/chat` endpoint.
//! This provider has zero marginal cost and reports status by checking
//! `/api/tags`.
//!
//! ## Input
//! - Neutral completion requests and local Ollama endpoint configuration
//!
//! ## Output
//! - Ollama chat requests and neutral completion responses
//!
//! ## Related
//! - `forge-router::translator` — builds Ollama-compatible JSON
//! - `forge-router::provider` — defines the `ModelProvider` trait

use std::time::Instant;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use yantra_core::{ModelCapability, ModelTier};

use crate::error::ProviderError;
use crate::provider::{CompletionRequest, CompletionResponse, ModelProvider, ProviderStatus};
use crate::translator::PromptTranslator;

/// Tier 0 provider backed by local Ollama.
#[derive(Debug, Clone)]
pub struct OllamaProvider {
    id: String,
    model: String,
    endpoint: String,
    client: Client,
}

impl OllamaProvider {
    /// Creates an Ollama provider using `http://localhost:11434`.
    pub fn new(model: impl Into<String>) -> Self {
        Self::with_endpoint(model, "http://localhost:11434")
    }

    /// Creates an Ollama provider using a custom endpoint.
    pub fn with_endpoint(model: impl Into<String>, endpoint: impl Into<String>) -> Self {
        let model = model.into();
        Self {
            id: format!("ollama:{model}"),
            model,
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            client: Client::new(),
        }
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn tier(&self) -> ModelTier {
        ModelTier::Tier0
    }

    fn capability(&self) -> ModelCapability {
        ModelCapability {
            context_limit: 32_768,
            supports_tools: true,
            cost_per_1k_in: 0.0,
            cost_per_1k_out: 0.0,
        }
    }

    async fn status(&self) -> ProviderStatus {
        match self
            .client
            .get(format!("{}/api/tags", self.endpoint))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => ProviderStatus::Available,
            Ok(response) => ProviderStatus::Down {
                since: Instant::now(),
                reason: format!("Ollama status {}", response.status()),
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
        let request_body = PromptTranslator::to_ollama(&request, &self.model);
        let response_body = self
            .client
            .post(format!("{}/api/chat", self.endpoint))
            .json(&request_body)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        PromptTranslator::from_ollama(&response_body)
    }
}
