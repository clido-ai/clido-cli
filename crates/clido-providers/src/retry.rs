//! Retry wrapper for transient provider failures with exponential backoff.

use crate::provider::{ModelEntry, StreamEvent};
use crate::ModelProvider;
use async_trait::async_trait;
use clido_core::{AgentConfig, Message, ModelResponse, ToolSchema};
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::warn;

/// Maximum number of retries for transient errors.
const MAX_RETRIES: usize = 3;
/// Base delay for exponential backoff (milliseconds).
const BASE_DELAY_MS: u64 = 500;

/// A provider wrapper that retries transient failures with exponential backoff.
///
/// Only non-streaming `complete` calls are retried; streaming is delegated directly
/// because partially-consumed streams cannot be safely replayed.
pub struct RetryProvider {
    inner: Arc<dyn ModelProvider>,
}

impl RetryProvider {
    pub fn new(inner: Arc<dyn ModelProvider>) -> Self {
        Self { inner }
    }

    /// Convenience: wrap a provider and return it as `Arc<dyn ModelProvider>`.
    pub fn wrap(inner: Arc<dyn ModelProvider>) -> Arc<dyn ModelProvider> {
        Arc::new(Self::new(inner))
    }

    /// Returns `true` if the error looks transient (rate-limit, server error, timeout).
    fn is_transient(err: &clido_core::ClidoError) -> bool {
        // Subscription rate limits are NOT transient — they won't clear on retry.
        if matches!(
            err,
            clido_core::ClidoError::RateLimited {
                is_subscription_limit: true,
                ..
            }
        ) {
            return false;
        }
        // Burst rate limits are already retried by the inner provider; don't double-retry.
        if matches!(err, clido_core::ClidoError::RateLimited { .. }) {
            return false;
        }
        let msg = err.to_string().to_lowercase();
        msg.contains("429")
            || msg.contains("rate limit")
            || msg.contains("500")
            || msg.contains("502")
            || msg.contains("503")
            || msg.contains("504")
            || msg.contains("timeout")
            || msg.contains("connection")
            || msg.contains("overloaded")
    }

    /// Returns `true` if the error is a non-retryable client/auth error.
    fn is_permanent(err: &clido_core::ClidoError) -> bool {
        if matches!(
            err,
            clido_core::ClidoError::RateLimited {
                is_subscription_limit: true,
                ..
            }
        ) {
            return true;
        }
        let msg = err.to_string().to_lowercase();
        msg.contains("401")
            || msg.contains("403")
            || msg.contains("400")
            || msg.contains("unauthorized")
            || msg.contains("forbidden")
            || msg.contains("invalid")
            || msg.contains("content policy")
    }
}

#[async_trait]
impl ModelProvider for RetryProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> clido_core::Result<ModelResponse> {
        let mut last_err = None;
        for attempt in 0..=MAX_RETRIES {
            match self.inner.complete(messages, tools, config).await {
                Ok(resp) => return Ok(resp),
                Err(e) if Self::is_permanent(&e) => return Err(e),
                Err(e) if Self::is_transient(&e) && attempt < MAX_RETRIES => {
                    let delay = Duration::from_millis(BASE_DELAY_MS * 2u64.pow(attempt as u32));
                    warn!(
                        attempt = attempt + 1,
                        max = MAX_RETRIES,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "Transient provider error, retrying"
                    );
                    sleep(delay).await;
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.expect("retry loop guarantees at least one error"))
    }

    async fn complete_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> clido_core::Result<Pin<Box<dyn Stream<Item = clido_core::Result<StreamEvent>> + Send>>>
    {
        self.inner.complete_stream(messages, tools, config).await
    }

    async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String> {
        self.inner.list_models().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clido_core::{ClidoError, StopReason, Usage};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fake provider that fails N times then succeeds.
    struct FlakyProvider {
        remaining_failures: AtomicUsize,
        error_msg: String,
    }

    impl FlakyProvider {
        fn new(failures: usize, error_msg: &str) -> Arc<Self> {
            Arc::new(Self {
                remaining_failures: AtomicUsize::new(failures),
                error_msg: error_msg.to_string(),
            })
        }
    }

    #[async_trait]
    impl ModelProvider for FlakyProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            let remaining = self.remaining_failures.fetch_sub(1, Ordering::SeqCst);
            if remaining > 0 {
                return Err(ClidoError::Provider(self.error_msg.clone()));
            }
            Ok(ModelResponse {
                id: String::new(),
                model: String::new(),
                content: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
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
            Ok(vec![])
        }
    }

    fn test_config() -> AgentConfig {
        AgentConfig {
            model: "test".to_string(),
            ..AgentConfig::default()
        }
    }

    #[tokio::test]
    async fn retries_on_transient_error_then_succeeds() {
        let inner = FlakyProvider::new(2, "HTTP 503 service unavailable");
        let provider = RetryProvider::wrap(inner);
        let result = provider.complete(&[], &[], &test_config()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn does_not_retry_permanent_error() {
        let inner = FlakyProvider::new(5, "HTTP 401 unauthorized");
        let provider = RetryProvider::wrap(inner);
        let result = provider.complete(&[], &[], &test_config()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("401"));
    }

    #[tokio::test]
    async fn exhausts_retries_on_persistent_transient_error() {
        let inner = FlakyProvider::new(10, "HTTP 429 rate limit exceeded");
        let provider = RetryProvider::wrap(inner);
        let result = provider.complete(&[], &[], &test_config()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("429"));
    }

    #[tokio::test]
    async fn succeeds_without_retry_on_first_try() {
        let inner = FlakyProvider::new(0, "should not matter");
        let provider = RetryProvider::wrap(inner);
        let result = provider.complete(&[], &[], &test_config()).await;
        assert!(result.is_ok());
    }
}
