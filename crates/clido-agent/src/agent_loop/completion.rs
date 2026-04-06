//! Provider completion: throttled batch or streaming aggregate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clido_core::{AgentConfig, Message, ModelResponse, Result, ToolSchema};
use clido_providers::ModelProvider;

use super::stream_aggregate;
use super::throttle;
use super::EventEmitter;

pub async fn invoke_model_completion(
    provider: Arc<dyn ModelProvider>,
    messages: &[Message],
    tools: &[ToolSchema],
    config: &AgentConfig,
    emit: Option<Arc<dyn EventEmitter>>,
    last_complete_end: &mut Option<std::time::Instant>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ModelResponse> {
    throttle::throttle_before_complete(last_complete_end, config.provider_min_request_interval_ms)
        .await;

    let response = if config.stream_model_completion {
        let stream = provider.complete_stream(messages, tools, config).await?;
        let r = stream_aggregate::collect_stream_to_model_response(
            stream,
            config.model.clone(),
            emit,
            cancel,
        )
        .await?;
        throttle::mark_complete_finished(last_complete_end);
        r
    } else {
        if cancel
            .as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            return Err(clido_core::ClidoError::Interrupted);
        }
        let r = provider.complete(messages, tools, config).await?;
        throttle::mark_complete_finished(last_complete_end);
        r
    };

    Ok(response)
}
