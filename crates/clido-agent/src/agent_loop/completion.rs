//! Provider completion: throttled batch or streaming aggregate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clido_core::{AgentConfig, ClidoError, ContentBlock, Message, ModelResponse, Result, ToolSchema};
use clido_context::DEFAULT_MAX_INPUT_CHARS;
use clido_providers::ModelProvider;

use super::stream_aggregate;
use super::throttle;
use super::EventEmitter;

/// Compute the total character count across all message content blocks.
fn count_input_chars(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|m| {
            m.content
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.chars().count() as u64,
                    ContentBlock::ToolUse { id, name, input } => {
                        id.chars().count() as u64
                            + name.chars().count() as u64
                            + input.to_string().chars().count() as u64
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        tool_use_id.chars().count() as u64 + content.chars().count() as u64
                    }
                    ContentBlock::Thinking { thinking } => thinking.chars().count() as u64,
                    ContentBlock::Image { base64_data, .. } => base64_data.chars().count() as u64,
                })
                .sum::<u64>()
        })
        .sum()
}

pub async fn invoke_model_completion(
    provider: Arc<dyn ModelProvider>,
    messages: &[Message],
    tools: &[ToolSchema],
    config: &AgentConfig,
    emit: Option<Arc<dyn EventEmitter>>,
    last_complete_end: &mut Option<std::time::Instant>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ModelResponse> {
    // Pre-validate input character count against provider limit.
    let max_chars = config.max_input_chars.unwrap_or(DEFAULT_MAX_INPUT_CHARS);
    if max_chars > 0 {
        let total_chars = count_input_chars(messages);
        if total_chars == 0 {
            return Err(ClidoError::InputTooLong {
                chars: 0,
                max_chars,
            });
        }
        if total_chars > max_chars {
            return Err(ClidoError::InputTooLong {
                chars: total_chars,
                max_chars,
            });
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use clido_core::{ContentBlock, Message, ModelResponse, StopReason, ToolSchema, Usage};
    use clido_providers::{ModelEntry, ModelProvider, StreamEvent};
    use futures::stream;
    use futures::Stream;
    use std::pin::Pin;

    struct OkNonStreamProvider;

    #[async_trait]
    impl ModelProvider for OkNonStreamProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            Ok(ModelResponse {
                id: "id".into(),
                model: "m".into(),
                content: vec![ContentBlock::Text {
                    text: "from-complete".into(),
                }],
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
            unimplemented!()
        }

        async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String> {
            Ok(vec![])
        }
    }

    struct StreamTextProvider;

    #[async_trait]
    impl ModelProvider for StreamTextProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            unimplemented!()
        }

        async fn complete_stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<Pin<Box<dyn Stream<Item = clido_core::Result<StreamEvent>> + Send>>>
        {
            let events = vec![
                Ok(StreamEvent::TextDelta("hello ".into())),
                Ok(StreamEvent::TextDelta("stream".into())),
                Ok(StreamEvent::MessageDelta {
                    stop_reason: StopReason::EndTurn,
                    usage: Usage {
                        input_tokens: 3,
                        output_tokens: 4,
                        cache_creation_input_tokens: Some(1),
                        cache_read_input_tokens: Some(2),
                    },
                }),
            ];
            Ok(Box::pin(stream::iter(events)))
        }

        async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String> {
            Ok(vec![])
        }
    }

    struct PanicNonStreamProvider;

    #[async_trait]
    impl ModelProvider for PanicNonStreamProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            panic!("complete should not run when cancelled");
        }

        async fn complete_stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<Pin<Box<dyn Stream<Item = clido_core::Result<StreamEvent>> + Send>>>
        {
            unimplemented!()
        }

        async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn non_stream_invokes_complete_and_marks_throttle_end() {
        let mut last = None;
        let mut cfg = AgentConfig::default();
        cfg.stream_model_completion = false;
        cfg.provider_min_request_interval_ms = 0;
        let r = invoke_model_completion(
            Arc::new(OkNonStreamProvider),
            &[],
            &[],
            &cfg,
            None,
            &mut last,
            None,
        )
        .await
        .unwrap();
        match r.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "from-complete"),
            _ => panic!("unexpected content: {:?}", r.content),
        }
        assert!(last.is_some());
    }

    #[tokio::test]
    async fn non_stream_cancelled_before_provider_call() {
        let mut last = None;
        let mut cfg = AgentConfig::default();
        cfg.stream_model_completion = false;
        let cancel = Arc::new(AtomicBool::new(true));
        let err = invoke_model_completion(
            Arc::new(PanicNonStreamProvider),
            &[],
            &[],
            &cfg,
            None,
            &mut last,
            Some(cancel),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, clido_core::ClidoError::Interrupted));
    }

    #[tokio::test]
    async fn stream_path_aggregates_response() {
        let mut last = None;
        let mut cfg = AgentConfig::default();
        cfg.stream_model_completion = true;
        cfg.model = "stream-m".into();
        cfg.provider_min_request_interval_ms = 0;
        let r = invoke_model_completion(
            Arc::new(StreamTextProvider),
            &[],
            &[],
            &cfg,
            None,
            &mut last,
            None,
        )
        .await
        .unwrap();
        assert_eq!(r.model, "stream-m");
        match r.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "hello stream"),
            _ => panic!("unexpected content: {:?}", r.content),
        }
        assert_eq!(r.usage.input_tokens, 3);
        assert_eq!(r.usage.output_tokens, 4);
        assert_eq!(r.usage.cache_creation_input_tokens, Some(1));
        assert_eq!(r.usage.cache_read_input_tokens, Some(2));
        assert!(last.is_some());
    }

    #[test]
    fn count_input_chars_empty_messages() {
        assert_eq!(count_input_chars(&[]), 0);
    }

    #[test]
    fn count_input_chars_text_only() {
        let msgs = vec![Message {
            role: clido_core::Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        }];
        assert_eq!(count_input_chars(&msgs), 5);
    }

    #[test]
    fn count_input_chars_multiple_blocks() {
        let msgs = vec![
            Message {
                role: clido_core::Role::User,
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            },
            Message {
                role: clido_core::Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "ok".to_string(),
                }],
            },
        ];
        assert_eq!(count_input_chars(&msgs), 4);
    }

    #[tokio::test]
    async fn invoke_rejects_empty_input() {
        let mut cfg = AgentConfig::default();
        cfg.max_input_chars = Some(100);
        cfg.stream_model_completion = false;
        cfg.provider_min_request_interval_ms = 0;
        let mut last = None;
        let err = invoke_model_completion(
            Arc::new(OkNonStreamProvider),
            &[],
            &[],
            &cfg,
            None,
            &mut last,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, clido_core::ClidoError::InputTooLong { .. }));
    }

    #[tokio::test]
    async fn invoke_rejects_oversized_input() {
        let mut cfg = AgentConfig::default();
        cfg.max_input_chars = Some(10);
        cfg.stream_model_completion = false;
        cfg.provider_min_request_interval_ms = 0;
        let msgs = vec![Message {
            role: clido_core::Role::User,
            content: vec![ContentBlock::Text {
                text: "this is way too long for the limit".to_string(),
            }],
        }];
        let mut last = None;
        let err = invoke_model_completion(
            Arc::new(OkNonStreamProvider),
            &msgs,
            &[],
            &cfg,
            None,
            &mut last,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, clido_core::ClidoError::InputTooLong { .. }));
    }

    #[tokio::test]
    async fn invoke_accepts_input_under_limit() {
        let mut cfg = AgentConfig::default();
        cfg.max_input_chars = Some(1000);
        cfg.stream_model_completion = false;
        cfg.provider_min_request_interval_ms = 0;
        let msgs = vec![Message {
            role: clido_core::Role::User,
            content: vec![ContentBlock::Text {
                text: "short".to_string(),
            }],
        }];
        let mut last = None;
        let r = invoke_model_completion(
            Arc::new(OkNonStreamProvider),
            &msgs,
            &[],
            &cfg,
            None,
            &mut last,
            None,
        )
        .await
        .unwrap();
        match &r.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "from-complete"),
            _ => panic!("expected text block"),
        }
    }

    #[tokio::test]
    async fn invoke_skips_validation_when_max_chars_zero() {
        let mut cfg = AgentConfig::default();
        cfg.max_input_chars = Some(0); // disabled
        cfg.stream_model_completion = false;
        cfg.provider_min_request_interval_ms = 0;
        let msgs: Vec<Message> = vec![];
        let mut last = None;
        // With max_input_chars=0, empty input should NOT be rejected by pre-validation
        // (the provider will handle it or return its own error)
        let r = invoke_model_completion(
            Arc::new(OkNonStreamProvider),
            &msgs,
            &[],
            &cfg,
            None,
            &mut last,
            None,
        )
        .await
        .unwrap();
        match &r.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "from-complete"),
            _ => panic!("expected text block"),
        }
    }
}
