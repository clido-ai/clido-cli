//! Provider wrapper that falls back to an alternative on primary failure.

use crate::provider::{ModelEntry, ModelProvider, StreamEvent};
use async_trait::async_trait;
use clido_core::{AgentConfig, Message, ModelResponse, ToolSchema};
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use tracing::warn;

/// A provider that tries the primary first, then falls back to an alternative on failure.
pub struct FallbackProvider {
    primary: Arc<dyn ModelProvider>,
    fallback: Arc<dyn ModelProvider>,
    /// Model name override for the fallback provider.
    fallback_model: String,
}

impl FallbackProvider {
    pub fn new(
        primary: Arc<dyn ModelProvider>,
        fallback: Arc<dyn ModelProvider>,
        fallback_model: String,
    ) -> Self {
        Self {
            primary,
            fallback,
            fallback_model,
        }
    }

    /// Convenience: wrap a provider pair and return as `Arc<dyn ModelProvider>`.
    pub fn wrap(
        primary: Arc<dyn ModelProvider>,
        fallback: Arc<dyn ModelProvider>,
        fallback_model: String,
    ) -> Arc<dyn ModelProvider> {
        Arc::new(Self::new(primary, fallback, fallback_model))
    }

    /// Build a fallback-adjusted config by overriding the model name.
    fn fallback_config(&self, config: &AgentConfig) -> AgentConfig {
        AgentConfig {
            model: self.fallback_model.clone(),
            ..config.clone()
        }
    }
}

#[async_trait]
impl ModelProvider for FallbackProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> clido_core::Result<ModelResponse> {
        match self.primary.complete(messages, tools, config).await {
            Ok(resp) => Ok(resp),
            Err(primary_err) => {
                warn!(
                    error = %primary_err,
                    fallback_model = %self.fallback_model,
                    "Primary provider failed, trying fallback"
                );
                let fallback_config = self.fallback_config(config);
                self.fallback
                    .complete(messages, tools, &fallback_config)
                    .await
                    .map_err(|fallback_err| {
                        clido_core::ClidoError::Provider(format!(
                            "Primary failed: {}; Fallback also failed: {}",
                            primary_err, fallback_err
                        ))
                    })
            }
        }
    }

    async fn complete_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> clido_core::Result<Pin<Box<dyn Stream<Item = clido_core::Result<StreamEvent>> + Send>>>
    {
        match self.primary.complete_stream(messages, tools, config).await {
            Ok(stream) => Ok(stream),
            Err(primary_err) => {
                warn!(
                    error = %primary_err,
                    fallback_model = %self.fallback_model,
                    "Primary provider stream failed, trying fallback"
                );
                let fallback_config = self.fallback_config(config);
                self.fallback
                    .complete_stream(messages, tools, &fallback_config)
                    .await
                    .map_err(|fallback_err| {
                        clido_core::ClidoError::Provider(format!(
                            "Primary failed: {}; Fallback also failed: {}",
                            primary_err, fallback_err
                        ))
                    })
            }
        }
    }

    async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String> {
        self.primary.list_models().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clido_core::{ClidoError, ContentBlock, StopReason, Usage};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FailProvider;

    #[async_trait]
    impl ModelProvider for FailProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            Err(ClidoError::Provider("primary boom".into()))
        }
        async fn complete_stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<Pin<Box<dyn Stream<Item = clido_core::Result<StreamEvent>> + Send>>>
        {
            Err(ClidoError::Provider("primary stream boom".into()))
        }
        async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String> {
            Ok(vec![])
        }
    }

    struct OkProvider {
        call_count: AtomicUsize,
    }

    impl OkProvider {
        fn new() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for OkProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ModelResponse {
                id: "test".into(),
                model: "test-model".into(),
                content: vec![ContentBlock::Text { text: "ok".into() }],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
        async fn complete_stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<Pin<Box<dyn Stream<Item = clido_core::Result<StreamEvent>> + Send>>>
        {
            Ok(Box::pin(futures::stream::empty()))
        }
        async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String> {
            Ok(vec![ModelEntry::available("test-model")])
        }
    }

    fn test_config() -> AgentConfig {
        AgentConfig {
            model: "primary-model".into(),
            ..AgentConfig::default()
        }
    }

    #[tokio::test]
    async fn primary_success_skips_fallback() {
        let primary = Arc::new(OkProvider::new());
        let fallback = Arc::new(OkProvider::new());
        let provider = FallbackProvider::new(primary.clone(), fallback.clone(), "fb-model".into());

        let resp = provider.complete(&[], &[], &test_config()).await.unwrap();
        assert_eq!(resp.model, "test-model");
        assert_eq!(primary.call_count.load(Ordering::SeqCst), 1);
        assert_eq!(fallback.call_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fallback_on_primary_failure() {
        let primary: Arc<dyn ModelProvider> = Arc::new(FailProvider);
        let fallback = Arc::new(OkProvider::new());
        let provider = FallbackProvider::new(primary, fallback.clone(), "fb-model".into());

        let resp = provider.complete(&[], &[], &test_config()).await.unwrap();
        assert_eq!(resp.model, "test-model");
        assert_eq!(fallback.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn both_fail_returns_combined_error() {
        let primary: Arc<dyn ModelProvider> = Arc::new(FailProvider);
        let fallback: Arc<dyn ModelProvider> = Arc::new(FailProvider);
        let provider = FallbackProvider::new(primary, fallback, "fb-model".into());

        let err = provider
            .complete(&[], &[], &test_config())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Primary failed"), "got: {msg}");
        assert!(msg.contains("Fallback also failed"), "got: {msg}");
    }
}
