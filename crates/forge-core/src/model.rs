//! # forge-core: Model Routing Types
//!
//! Defines provider-neutral model identifiers, routing tiers, and model
//! capability metadata. The router uses these types without coupling the
//! runtime to any provider implementation.
//!
//! ## Input
//! - Model names from routing configuration
//! - Capability metadata from provider discovery
//!
//! ## Output
//! - `ModelTier`, `ModelId`, and `ModelCapability`
//!
//! ## Related
//! - `forge-core::span` — records model usage and cost

use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// Cost and capability tier for a model route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelTier {
    /// Local model, such as Ollama.
    Tier0,
    /// Free hosted model.
    Tier1,
    /// Low-cost hosted model.
    Tier2,
    /// Frontier model reserved for high-value decisions.
    Tier3,
}

/// Provider-qualified model identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(String);

impl ModelId {
    /// Creates a model identifier from a provider-qualified string.
    ///
    /// # Errors
    ///
    /// Returns `CoreError::InvalidModelId` when the identifier is empty.
    pub fn new(identifier: impl Into<String>) -> CoreResult<Self> {
        let identifier = identifier.into();
        if identifier.trim().is_empty() {
            return Err(CoreError::InvalidModelId {
                reason: "model id cannot be empty".to_string(),
            });
        }
        Ok(Self(identifier))
    }

    /// Returns the model identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ModelId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ModelId {
    type Err = CoreError;

    fn from_str(input: &str) -> CoreResult<Self> {
        Self::new(input)
    }
}

/// Provider-neutral model capability and cost metadata.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelCapability {
    /// Maximum context window in tokens.
    pub context_limit: usize,
    /// Whether the model supports native tool calls.
    pub supports_tools: bool,
    /// Input cost in USD per 1,000 tokens.
    pub cost_per_1k_in: f32,
    /// Output cost in USD per 1,000 tokens.
    pub cost_per_1k_out: f32,
}
