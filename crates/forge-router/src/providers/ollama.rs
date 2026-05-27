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
        let mut attempts = 0;
        let mut delay = std::time::Duration::from_secs(1);
        let response_body = loop {
            match self
                .client
                .post(format!("{}/api/chat", self.endpoint))
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
        PromptTranslator::from_ollama(&response_body)
    }

    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> Result<
        std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<String, ProviderError>> + Send>>,
        ProviderError,
    > {
        let mut request_body = PromptTranslator::to_ollama(&request, &self.model);
        request_body["stream"] = serde_json::Value::Bool(true);

        let mut attempts = 0;
        let mut delay = std::time::Duration::from_secs(1);
        let response = loop {
            match self
                .client
                .post(format!("{}/api/chat", self.endpoint))
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
                                if !line.is_empty() {
                                    if let Ok(value) =
                                        serde_json::from_str::<serde_json::Value>(&line)
                                    {
                                        if let Some(content) = value["message"]["content"].as_str()
                                        {
                                            if sender.send(Ok(content.to_string())).await.is_err() {
                                                return;
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
