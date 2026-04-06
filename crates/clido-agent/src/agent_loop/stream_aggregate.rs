//! Fold provider `StreamEvent` stream into a single [`ModelResponse`] for the agent loop.

use std::collections::HashMap;
use std::pin::Pin;
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
) -> Result<ModelResponse> {
    let mut text_buf = String::new();
    let mut content: Vec<ContentBlock> = Vec::new();
    let mut tool_json: HashMap<String, String> = HashMap::new();
    let mut tool_name: HashMap<String, String> = HashMap::new();
    let mut stop = StopReason::EndTurn;
    let mut usage = Usage::default();
    let mut saw_message_delta = false;

    while let Some(ev) = stream.next().await {
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
                flush_text(&mut content, &mut text_buf);
                tool_name.insert(id.clone(), name);
                tool_json.insert(id, String::new());
            }
            StreamEvent::ToolUseDelta { id, partial_json } => {
                tool_json.entry(id).or_default().push_str(&partial_json);
            }
            StreamEvent::ToolUseEnd { id } => {
                let name = tool_name.remove(&id).unwrap_or_default();
                let json_str = tool_json.remove(&id).unwrap_or_else(|| "{}".to_string());
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

    use clido_core::ClidoError;
    use clido_providers::StreamEvent;
    use futures::stream;
    use futures::Stream;

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
        let r = collect_stream_to_model_response(st, "test-model".to_string(), None).await;
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
}
