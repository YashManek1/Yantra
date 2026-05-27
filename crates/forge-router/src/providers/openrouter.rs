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
        let mut attempts = 0;
        let mut delay = std::time::Duration::from_secs(1);
        let response_body = loop {
            match self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&request_body)
                .send()
                .await
            {
                Ok(response) => match response.error_for_status() {
                    Ok(res) => match res.json::<Value>().await {
                        Ok(json) => break json,
                        Err(err) => {
                            attempts += 1;
                            if attempts >= 3 {
                                return Err(ProviderError::InvalidResponse(err.to_string()));
                            }
                            tokio::time::sleep(delay).await;
                            delay *= 2;
                        }
                    },
                    Err(err) => {
                        attempts += 1;
                        if attempts >= 3 {
                            return Err(ProviderError::Http(err));
                        }
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                    }
                },
                Err(err) => {
                    attempts += 1;
                    if attempts >= 3 {
                        return Err(ProviderError::Http(err));
                    }
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
            }
        };
        PromptTranslator::from_openai(&response_body)
    }

    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> Result<
        std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<String, ProviderError>> + Send>>,
        ProviderError,
    > {
        let mut request_body = PromptTranslator::to_openai(&request, &self.model);
        request_body["stream"] = serde_json::Value::Bool(true);

        let mut attempts = 0;
        let mut delay = std::time::Duration::from_secs(1);
        let response = loop {
            match self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&request_body)
                .send()
                .await
            {
                Ok(res) => match res.error_for_status() {
                    Ok(ok_res) => break ok_res,
                    Err(err) => {
                        attempts += 1;
                        if attempts >= 3 {
                            return Err(ProviderError::Http(err));
                        }
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                    }
                },
                Err(err) => {
                    attempts += 1;
                    if attempts >= 3 {
                        return Err(ProviderError::Http(err));
                    }
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
            }
        };

        let (sender, receiver) = tokio::sync::mpsc::channel(100);
        let mut bytes_stream = response.bytes_stream();

        tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut line_buffer = String::new();
            while let Some(chunk_result) = bytes_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        if let Ok(text) = std::str::from_utf8(&bytes) {
                            line_buffer.push_str(text);
                            while let Some(index) = line_buffer.find('\n') {
                                let line = line_buffer[..index].trim().to_string();
                                line_buffer = line_buffer[index + 1..].to_string();
                                if let Some(data_content) = line.strip_prefix("data: ") {
                                    if data_content == "[DONE]" {
                                        return;
                                    }
                                    if let Ok(value) =
                                        serde_json::from_str::<serde_json::Value>(data_content)
                                    {
                                        if let Some(choices) =
                                            value.get("choices").and_then(Value::as_array)
                                        {
                                            if let Some(choice) = choices.first() {
                                                if let Some(content) = choice
                                                    .get("delta")
                                                    .and_then(|d| d.get("content"))
                                                    .and_then(Value::as_str)
                                                {
                                                    if sender
                                                        .send(Ok(content.to_string()))
                                                        .await
                                                        .is_err()
                                                    {
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        let _ = sender.send(Err(ProviderError::Http(err))).await;
                        return;
                    }
                }
            }
        });

        let receiver_stream = crate::provider::ReceiverStream::new(receiver);
        Ok(Box::pin(receiver_stream))
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
