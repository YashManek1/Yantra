//! # forge-router: Provider Contract
//!
//! Defines the backend-neutral model provider trait and completion data
//! structures. Every model backend implements this trait so the rest of
//! Yantra routes through policy rather than provider-specific code.
//!
//! ## Input
//! - Chat messages, tool definitions, decoding settings, and stop sequences
//!
//! ## Output
//! - Provider status values and completion responses
//!
//! ## Related
//! - `forge-router::providers` — concrete provider implementations
//! - `forge-router::routing` — selects providers by tier and health

use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use yantra_core::{ModelCapability, ModelTier};

use crate::error::ProviderError;

/// Core trait implemented by every model backend.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Stable provider identifier.
    fn id(&self) -> &str;

    /// Routing tier served by this provider.
    fn tier(&self) -> ModelTier;

    /// Model capability and cost metadata.
    fn capability(&self) -> ModelCapability;

    /// Current provider health and rate-limit status.
    async fn status(&self) -> ProviderStatus;

    /// Executes one chat completion request.
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError>;

    /// Executes one chat completion request and streams the response.
    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> Result<
        std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<String, ProviderError>> + Send>>,
        ProviderError,
    >;
}

/// Current provider availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStatus {
    /// Provider is usable now.
    Available,
    /// Provider is rate-limited until the given instant.
    RateLimited {
        /// Instant after which retry is permitted.
        retry_after: Instant,
    },
    /// Provider is unavailable.
    Down {
        /// Instant at which downtime began.
        since: Instant,
        /// Human-readable reason.
        reason: String,
    },
}

/// Provider-neutral chat completion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// Ordered chat messages.
    pub messages: Vec<Message>,
    /// Maximum output tokens.
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: f32,
    /// Optional tool declarations.
    pub tools: Option<Vec<ToolSpec>>,
    /// Stop sequences.
    pub stop_sequences: Vec<String>,
}

/// Provider-neutral chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Message role.
    pub role: MessageRole,
    /// Message content.
    pub content: String,
    /// Tool calls requested by this message.
    pub tool_calls: Vec<ToolCall>,
}

/// Chat message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MessageRole {
    /// System instruction.
    System,
    /// User message.
    User,
    /// Assistant response.
    Assistant,
    /// Tool response.
    Tool,
}

/// Tool declaration supplied to a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON schema for tool parameters.
    pub parameters_json_schema: serde_json::Value,
}

/// Tool call emitted by a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider tool call identifier.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// JSON arguments encoded as a string.
    pub arguments: String,
}

/// Provider-neutral completion response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// Text content returned by the model.
    pub content: String,
    /// Tool calls returned by the model.
    pub tool_calls: Vec<ToolCall>,
    /// Input tokens consumed.
    pub tokens_in: u64,
    /// Output tokens produced.
    pub tokens_out: u64,
    /// Completion stop reason.
    pub finish_reason: FinishReason,
}

/// Completion stop reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    /// Model stopped naturally.
    Stop,
    /// Model reached max-token limit.
    Length,
    /// Model requested tool execution.
    ToolCalls,
    /// Provider returned another reason.
    Other(String),
}

/// Helper struct that implements `futures_core::Stream` for a `tokio::sync::mpsc::Receiver`.
pub struct ReceiverStream<T> {
    receiver: tokio::sync::mpsc::Receiver<T>,
}

impl<T> ReceiverStream<T> {
    /// Creates a new `ReceiverStream` wrapping the given receiver.
    pub fn new(receiver: tokio::sync::mpsc::Receiver<T>) -> Self {
        Self { receiver }
    }
}

impl<T> futures_core::Stream for ReceiverStream<T> {
    type Item = T;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}
