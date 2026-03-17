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
}
