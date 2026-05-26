//! # forge-router: Error Types
//!
//! Defines typed errors for provider calls, prompt translation, routing
//! decisions, and configuration loading. The router keeps these failures
//! provider-neutral so callers can make policy decisions mechanically.
//!
//! ## Input
//! - HTTP client errors, JSON errors, missing credentials, and routing misses
//!
//! ## Output
//! - `ProviderError`, `RouterError`, and `RouterResult<T>`
//!
//! ## Related
//! - `forge-router::provider` — provider trait returns `ProviderError`
//! - `forge-router::routing` — router returns `RouterError`

use thiserror::Error;

/// Result alias for router operations.
pub type RouterResult<T> = Result<T, RouterError>;

/// Error returned by model providers.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// Required API key or endpoint configuration was missing.
    #[error("missing provider configuration: {0}")]
    MissingConfiguration(String),

    /// HTTP request failed.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization or deserialization failed.
    #[error("JSON conversion failed: {0}")]
    Json(#[from] serde_json::Error),

    /// Provider returned an invalid or unsupported response.
    #[error("invalid provider response: {0}")]
    InvalidResponse(String),
}

/// Error returned by routing and policy operations.
#[derive(Debug, Error)]
pub enum RouterError {
    /// Provider call failed.
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    /// No provider is registered for the requested tier.
    #[error("no provider available for tier {0:?}")]
    NoProvider(yantra_core::ModelTier),

    /// Policy configuration could not be read.
    #[error("routing policy configuration error: {0}")]
    PolicyConfiguration(String),
}
