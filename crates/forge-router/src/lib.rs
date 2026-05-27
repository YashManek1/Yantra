//! # forge-router: Model Router and Provider Trait
//!
//! Defines the `ModelProvider` trait that every LLM backend must implement,
//! and applies the four-tier routing policy (Tier 0 Ollama → Tier 1 free
//! APIs → Tier 2 low-cost → Tier 3 frontier) to each outgoing prompt.
//!
//! ## Input
//! - A `RoutedPrompt` carrying the rendered context, task class, and
//!   cost/latency budget from `configs/routing.toml`
//!
//! ## Output
//! - A `ModelResponse` from whichever provider was selected
//! - A `RoutingDecision` record written to the decision log
//!
//! ## Related
//! - `forge-agents` — every agent sends prompts through this router
//! - `forge-obs` — router emits a span per call with tier and cost

pub mod error;
pub mod health;
pub mod provider;
pub mod providers;
pub mod routing;
pub mod translator;

pub use error::{ProviderError, RouterError, RouterResult};
pub use health::{spawn_health_polling, HealthEvent, ProviderHealth};
pub use provider::{
    CompletionRequest, CompletionResponse, FinishReason, Message, MessageRole, ModelProvider,
    ProviderStatus, ToolCall, ToolSpec,
};
pub use providers::{
    github_models::GitHubModelsProvider, ollama::OllamaProvider, openrouter::OpenRouterProvider,
};
pub use routing::{RoutedCompletionRequest, Router, RoutingPolicy, TaskDescription};
pub use translator::PromptTranslator;
