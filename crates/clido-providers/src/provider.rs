//! Model provider trait and shared types.

use async_trait::async_trait;
use clido_core::{AgentConfig, Message, ModelResponse, ToolSchema, Usage};
use futures::Stream;
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

/// A model returned by a provider's model-discovery API.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelEntry {
    /// Model identifier (used in API calls).
    pub id: String,
    /// False when the provider reports this model has no usable endpoints.
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
}
