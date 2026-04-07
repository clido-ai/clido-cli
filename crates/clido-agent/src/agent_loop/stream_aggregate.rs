//! Fold provider `StreamEvent` stream into a single [`ModelResponse`] for the agent loop.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clido_core::{ClidoError, ContentBlock, ModelResponse, Result, StopReason, Usage};
use clido_providers::StreamEvent;
use futures::Stream;
use futures::StreamExt;

use super::EventEmitter;

/// Consume a full assistant response from streaming events (Anthropic/OpenAI-compatible `StreamEvent`s).
pub async fn collect_stream_to_model_response(
    mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>,
    model_id: String,
    emit: Option<Arc<dyn EventEmitter>>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ModelResponse> {
    let mut text_buf = String::new();
    let mut content: Vec<ContentBlock> = Vec::new();
    let mut tool_json: HashMap<String, String> = HashMap::new();
    let mut tool_name: HashMap<String, String> = HashMap::new();
    let mut stop = StopReason::EndTurn;
    let mut usage = Usage::default();
    let mut saw_message_delta = false;

    while let Some(ev) = stream.next().await {
        if cancel
            .as_ref()
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            return Err(ClidoError::Interrupted);
        }
        match ev? {
            StreamEvent::TextDelta(t) => {
                if !t.is_empty() {
                    if let Some(ref e) = emit {
                        e.on_assistant_text(&t).await;
                    }
                    text_buf.push_str(&t);
                }
            }
            StreamEvent::ToolUseStart { id, name } => {
                if id.is_empty() {
                    return Err(ClidoError::MalformedModelOutput {
                        detail: "streaming tool_use: empty tool id on tool_use_start".into(),
                    });
                }
                if name.trim().is_empty() {
                    return Err(ClidoError::MalformedModelOutput {
                        detail: format!("streaming tool_use: empty tool name on start (id={id})"),
                    });
                }
                flush_text(&mut content, &mut text_buf);
                tool_name.insert(id.clone(), name);
                tool_json.insert(id, String::new());
            }
            StreamEvent::ToolUseDelta { id, partial_json } => {
                let Some(buf) = tool_json.get_mut(&id) else {
                    return Err(ClidoError::MalformedModelOutput {
                        detail: format!(
                            "streaming tool_use: delta for unknown id={id} (missing tool_use_start)"
                        ),
                    });
                };
                buf.push_str(&partial_json);
            }
            StreamEvent::ToolUseEnd { id } => {
                let Some(name) = tool_name.remove(&id) else {
                    return Err(ClidoError::MalformedModelOutput {
                        detail: format!(
                            "streaming tool_use: end for unknown or duplicate id={id} (missing matching start)"
                        ),
                    });
                };
                let Some(json_str) = tool_json.remove(&id) else {
                    return Err(ClidoError::MalformedModelOutput {
                        detail: format!(
                            "streaming tool_use: internal state missing json buffer for id={id}"
                        ),
                    });
                };
                let input = serde_json::from_str::<serde_json::Value>(&json_str).map_err(|e| {
                    ClidoError::MalformedModelOutput {
                        detail: format!(
                            "streaming tool_use id={id} name={name:?}: invalid arguments JSON ({e}); raw_len={} bytes",
                            json_str.len()
                        ),
                    }
                })?;
                content.push(ContentBlock::ToolUse { id, name, input });
            }
            StreamEvent::MessageDelta {
                stop_reason,
                usage: u,
            } => {
                stop = stop_reason;
                usage.output_tokens = usage.output_tokens.saturating_add(u.output_tokens);
                usage.input_tokens = usage.input_tokens.saturating_add(u.input_tokens);
                if let Some(c) = u.cache_creation_input_tokens {
                    *usage.cache_creation_input_tokens.get_or_insert(0) += c;
                }
                if let Some(r) = u.cache_read_input_tokens {
                    *usage.cache_read_input_tokens.get_or_insert(0) += r;
                }
                saw_message_delta = true;
            }
        }
    }

    flush_text(&mut content, &mut text_buf);

    if !tool_name.is_empty() || !tool_json.is_empty() {
        return Err(ClidoError::MalformedModelOutput {
            detail: format!(
                "stream ended with incomplete tool_use block(s): {} tool call(s) missing tool_use end",
                tool_name.len()
            ),
        });
    }

    if !saw_message_delta {
        return Err(ClidoError::Provider(
            "stream ended without message_delta (incomplete response)".into(),
        ));
    }

    Ok(ModelResponse {
        id: String::new(),
        model: model_id,
        content,
        stop_reason: stop,
        usage,
    })
}

fn flush_text(content: &mut Vec<ContentBlock>, buf: &mut String) {
    if buf.is_empty() {
        return;
    }
    if let Some(ContentBlock::Text { text }) = content.last_mut() {
        text.push_str(buf.as_str());
    } else {
        content.push(ContentBlock::Text {
            text: std::mem::take(buf),
        });
    }
    buf.clear();
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use clido_core::{ClidoError, ContentBlock, StopReason, Usage};
    use clido_providers::StreamEvent;
    use futures::stream;
    use futures::Stream;

    use super::super::EventEmitter;
    use super::collect_stream_to_model_response;

    fn box_stream<I>(s: I) -> Pin<Box<dyn Stream<Item = clido_core::Result<StreamEvent>> + Send>>
    where
        I: Stream<Item = clido_core::Result<StreamEvent>> + Send + 'static,
    {
        Box::pin(s)
    }

    #[tokio::test]
    async fn stream_rejects_invalid_tool_json() {
        let events = vec![
            Ok(StreamEvent::ToolUseStart {
                id: "tu_1".to_string(),
                name: "Read".to_string(),
            }),
            Ok(StreamEvent::ToolUseDelta {
                id: "tu_1".to_string(),
                partial_json: "not-json".to_string(),
            }),
            Ok(StreamEvent::ToolUseEnd {
                id: "tu_1".to_string(),
            }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: clido_core::StopReason::ToolUse,
                usage: Default::default(),
            }),
        ];
        let st = box_stream(stream::iter(events));
        let r = collect_stream_to_model_response(st, "test-model".to_string(), None, None).await;
        assert!(r.is_err(), "expected error, got {r:?}");
        let err = r.unwrap_err();
        match err {
            ClidoError::MalformedModelOutput { detail } => {
                assert!(
                    detail.contains("tu_1") && detail.contains("invalid arguments JSON"),
                    "detail={detail}"
                );
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_rejects_tool_use_end_without_start() {
        let events = vec![
            Ok(StreamEvent::ToolUseEnd {
                id: "tu_x".to_string(),
            }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: clido_core::StopReason::ToolUse,
                usage: Default::default(),
            }),
        ];
        let st = box_stream(stream::iter(events));
        let r = collect_stream_to_model_response(st, "m".to_string(), None, None).await;
        assert!(r.is_err(), "expected error, got {r:?}");
    }

    #[tokio::test]
    async fn stream_rejects_delta_before_start() {
        let events = vec![
            Ok(StreamEvent::ToolUseDelta {
                id: "tu_1".to_string(),
                partial_json: "{}".to_string(),
            }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: clido_core::StopReason::ToolUse,
                usage: Default::default(),
            }),
        ];
        let st = box_stream(stream::iter(events));
        let r = collect_stream_to_model_response(st, "m".to_string(), None, None).await;
        assert!(r.is_err(), "expected error, got {r:?}");
    }

    #[tokio::test]
    async fn stream_rejects_incomplete_tool_at_eof() {
        let events = vec![
            Ok(StreamEvent::ToolUseStart {
                id: "tu_1".to_string(),
                name: "Read".to_string(),
            }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: clido_core::StopReason::ToolUse,
                usage: Default::default(),
            }),
        ];
        let st = box_stream(stream::iter(events));
        let r = collect_stream_to_model_response(st, "m".to_string(), None, None).await;
        assert!(r.is_err(), "expected error, got {r:?}");
        match r.unwrap_err() {
            ClidoError::MalformedModelOutput { detail } => {
                assert!(
                    detail.contains("incomplete") || detail.contains("missing tool_use end"),
                    "detail={detail}"
                );
            }
            other => panic!("wrong err: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_rejects_empty_tool_name_on_start() {
        let events = vec![
            Ok(StreamEvent::ToolUseStart {
                id: "tu_1".to_string(),
                name: "  ".to_string(),
            }),
            Ok(StreamEvent::ToolUseEnd {
                id: "tu_1".to_string(),
            }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: clido_core::StopReason::ToolUse,
                usage: Default::default(),
            }),
        ];
        let st = box_stream(stream::iter(events));
        let r = collect_stream_to_model_response(st, "m".to_string(), None, None).await;
        assert!(r.is_err(), "expected error, got {r:?}");
    }

    struct CountTextEmitter(Arc<AtomicU32>);

    #[async_trait]
    impl EventEmitter for CountTextEmitter {
        async fn on_tool_start(&self, _: &str, _: &str, _: &serde_json::Value) {}

        async fn on_tool_done(
            &self,
            _: &str,
            _: &str,
            _: bool,
            _: Option<String>,
        ) {
        }

        async fn on_assistant_text(&self, _text: &str) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn stream_happy_path_merges_text_and_usage() {
        let events = vec![
            Ok(StreamEvent::TextDelta("a ".into())),
            Ok(StreamEvent::TextDelta("b".into())),
            Ok(StreamEvent::MessageDelta {
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    cache_creation_input_tokens: Some(5),
                    cache_read_input_tokens: Some(6),
                },
            }),
        ];
        let st = box_stream(stream::iter(events));
        let r = collect_stream_to_model_response(st, "mid".into(), None, None)
            .await
            .unwrap();
        assert_eq!(r.model, "mid");
        match r.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "a b"),
            _ => panic!("unexpected content: {:?}", r.content),
        }
        assert_eq!(r.usage.input_tokens, 10);
        assert_eq!(r.usage.output_tokens, 20);
        assert_eq!(r.usage.cache_creation_input_tokens, Some(5));
        assert_eq!(r.usage.cache_read_input_tokens, Some(6));
    }

    #[tokio::test]
    async fn stream_rejects_empty_tool_id_on_start() {
        let events = vec![
            Ok(StreamEvent::ToolUseStart {
                id: String::new(),
                name: "Read".into(),
            }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: StopReason::ToolUse,
                usage: Default::default(),
            }),
        ];
        let st = box_stream(stream::iter(events));
        let r = collect_stream_to_model_response(st, "m".into(), None, None).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn stream_emits_text_deltas_to_emitter() {
        let ctr = Arc::new(AtomicU32::new(0));
        let emit: Arc<dyn EventEmitter> = Arc::new(CountTextEmitter(ctr.clone()));
        let events = vec![
            Ok(StreamEvent::TextDelta("x".into())),
            Ok(StreamEvent::MessageDelta {
                stop_reason: StopReason::EndTurn,
                usage: Default::default(),
            }),
        ];
        let st = box_stream(stream::iter(events));
        let _ = collect_stream_to_model_response(st, "e".into(), Some(emit), None)
            .await
            .unwrap();
        assert_eq!(ctr.load(Ordering::Relaxed), 1);
    }
}
