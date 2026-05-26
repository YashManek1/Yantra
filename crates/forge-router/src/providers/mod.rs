//! # forge-router: Provider Implementations
//!
//! Exposes concrete model provider backends for local Ollama, `OpenRouter`,
//! and GitHub Models. Each provider implements the shared `ModelProvider`
//! trait and keeps backend-specific HTTP details private.
//!
//! ## Input
//! - Provider endpoints, API keys, and neutral completion requests
//!
//! ## Output
//! - Provider-specific HTTP requests and neutral completion responses
//!
//! ## Related
//! - `forge-router::provider` — defines the backend contract
//! - `forge-router::translator` — maps neutral requests to provider JSON

pub mod github_models;
pub mod ollama;
pub mod openrouter;
