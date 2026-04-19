//! Model provider trait and shared types.

use async_trait::async_trait;
use clido_core::{AgentConfig, Message, ModelResponse, ToolSchema, Usage};
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;

use clido_core::Result;

/// Stream event for incremental model output (Phase 3 streaming).
#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ToolUseStart {
        id: String,
        name: String,
    },
    ToolUseDelta {
        id: String,
        partial_json: String,
    },
    ToolUseEnd {
        id: String,
    },
    MessageDelta {
        stop_reason: clido_core::StopReason,
        usage: Usage,
    },
}

/// Pricing information for a model.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// Capabilities of a model.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ModelCapabilities {
    pub reasoning: bool,
    pub tool_call: bool,
    pub vision: bool,
    pub temperature: bool,
}

/// Lifecycle status of a model.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    #[default]
    Active,
    Beta,
    Deprecated,
}

/// Rich metadata for a model, from the models API or provider discovery.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelMetadata {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub pricing: Option<ModelPricing>,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    #[serde(default)]
    pub status: ModelStatus,
    #[serde(default)]
    pub release_date: Option<String>,
    /// True when the provider's live API confirmed this model is available.
    #[serde(default = "default_available")]
    pub available: bool,
}

fn default_available() -> bool {
    true
}

impl ModelMetadata {
    pub fn available(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            context_window: None,
            pricing: None,
            capabilities: ModelCapabilities::default(),
            status: ModelStatus::Active,
            release_date: None,
            available: true,
        }
    }

    pub fn unavailable(id: impl Into<String>) -> Self {
        let mut m = Self::available(id);
        m.available = false;
        m
    }
}

/// A model returned by a provider's model-discovery API.
/// Kept for backward compatibility; prefer `ModelMetadata` in new code.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelEntry {
    /// Model identifier (used in API calls).
    pub id: String,
    /// False when the provider reports this model has no endpoints.
    /// Such models are shown greyed-out in pickers and cannot be selected.
    pub available: bool,
}

impl std::fmt::Display for ModelEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

impl ModelEntry {
    /// Convenience ctor: model is available.
    pub fn available(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            available: true,
        }
    }
    /// Convenience ctor: model exists but has no endpoints.
    pub fn unavailable(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            available: false,
        }
    }
}

impl From<&ModelMetadata> for ModelEntry {
    fn from(m: &ModelMetadata) -> Self {
        Self {
            id: m.id.clone(),
            available: m.available,
        }
    }
}

impl From<ModelMetadata> for ModelEntry {
    fn from(m: ModelMetadata) -> Self {
        Self {
            id: m.id,
            available: m.available,
        }
    }
}

/// Snapshot of all providers and their models from the models API.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ModelsSnapshot {
    pub providers: HashMap<String, ProviderModels>,
    /// Unix timestamp (seconds) when this snapshot was fetched.
    #[serde(default)]
    pub fetched_at: Option<u64>,
}

/// Models and metadata for a single provider from the models API.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderModels {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub is_subscription: Option<bool>,
    pub models: HashMap<String, ModelMetadata>,
}

/// Provider for chat completion (single call or stream).
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Non-streaming completion. Used by the PoC loop.
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> Result<ModelResponse>;

    /// Streaming completion (for Phase 3). PoC returns empty stream.
    async fn complete_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>>;

    /// List models available from this provider.
    /// Returns `Err(message)` on authentication failure or network error.
    /// Returns `Ok(vec![])` if the provider doesn't support model discovery.
    /// `available=false` entries are shown greyed-out in pickers.
    async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String>;

    /// List models with rich metadata (pricing, context, capabilities).
    /// Default implementation wraps `list_models()` with minimal metadata.
    /// Override in provider implementations to return full metadata.
    async fn list_models_metadata(&self) -> std::result::Result<Vec<ModelMetadata>, String> {
        let entries = self.list_models().await?;
        Ok(entries
            .into_iter()
            .map(|e| ModelMetadata {
                id: e.id,
                name: None,
                context_window: None,
                pricing: None,
                capabilities: ModelCapabilities::default(),
                status: ModelStatus::Active,
                release_date: None,
                available: e.available,
            })
            .collect())
    }

    /// Update the model used for subsequent API calls.
    /// This allows live model switching without recreating the provider.
    /// Default implementation is a no-op for providers that don't support model switching.
    fn set_model(&self, _model: String) {}
}
