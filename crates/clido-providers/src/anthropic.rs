//! Anthropic Messages API client.

use async_trait::async_trait;
use clido_core::{
    AgentConfig, ContentBlock, Message, ModelResponse, Role, StopReason, ToolSchema, Usage,
};
use clido_core::{ClidoError, Result};
use futures::Stream;
use std::pin::Pin;
use std::time::Duration;

use crate::backoff::{
    network_backoff_secs, parse_retry_after, parse_retry_after_secs, rate_limit_backoff_secs,
    server_error_backoff_secs, MAX_NETWORK_ATTEMPTS, MAX_RATE_LIMIT_ATTEMPTS,
    MAX_SERVER_ERROR_ATTEMPTS,
};
use tracing::{debug, warn};

use crate::provider::{ModelProvider, StreamEvent};

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self::new_with_user_agent(
            api_key,
            model,
            &format!("clido/{}", env!("CARGO_PKG_VERSION")),
        )
    }

    /// Like [`new`] but with an explicit User-Agent header.
    pub fn new_with_user_agent(api_key: String, model: String, user_agent: &str) -> Self {
        let client = crate::http_client::build_http_client(user_agent);
        Self {
            client,
            api_key,
            model,
        }
    }

    /// Build request body and POST to API, with robust retry/backoff.
    async fn complete_impl(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> Result<ModelResponse> {
        let mut system = config
            .system_prompt
            .as_deref()
            .unwrap_or("You are a helpful coding assistant.")
            .to_string();
        for m in messages.iter() {
            if m.role == Role::System {
                for b in &m.content {
                    if let ContentBlock::Text { text } = b {
                        system.push('\n');
                        system.push_str(text);
                    }
                }
            }
        }
        let anthropic_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(message_to_anthropic)
            .collect::<Result<Vec<_>>>()?;

        let anthropic_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema
                })
            })
            .collect();

        let system_blocks = serde_json::json!([{
            "type": "text",
            "text": system,
            "cache_control": {"type": "ephemeral"}
        }]);

        let max_tokens = config.max_output_tokens.unwrap_or(8192);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": system_blocks,
            "messages": anthropic_messages,
            "tools": anthropic_tools
        });

        let mut rate_limit_attempts = 0u32;
        let mut server_error_attempts = 0u32;
        let mut network_attempts = 0u32;

        loop {
            let res = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "prompt-caching-2024-07-31")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await;

            let res = match res {
                Ok(r) => r,
                Err(e) => {
                    // Network / connection / timeout error — retry a few times.
                    network_attempts += 1;
                    if network_attempts < MAX_NETWORK_ATTEMPTS {
                        let delay_secs = network_backoff_secs(network_attempts);
                        warn!(
                            "Network error (attempt {}/{}), retrying in {}s: {}",
                            network_attempts, MAX_NETWORK_ATTEMPTS, delay_secs, e
                        );
                        tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                        continue;
                    }
                    return Err(ClidoError::Provider(format!(
                        "Connection failed after {} attempts: {}",
                        MAX_NETWORK_ATTEMPTS,
                        friendly_network_error(&e)
                    )));
                }
            };

            let status = res.status();

            if status.as_u16() == 429 {
                // Rate limited — respect Retry-After or use exponential backoff.
                let retry_after = parse_retry_after(res.headers());
                let retry_after_secs = parse_retry_after_secs(res.headers());
                let body = res.text().await.unwrap_or_default();

                // Subscription/quota limits have long reset times or specific error text.
                let is_subscription = retry_after_secs.is_some_and(|s| s > 300)
                    || body.contains("quota")
                    || body.contains("subscription")
                    || body.contains("limit exceeded")
                    || body.contains("allowance");

                if is_subscription {
                    let reset_msg = if let Some(secs) = retry_after_secs {
                        let hrs = secs / 3600;
                        let mins = (secs % 3600) / 60;
                        if hrs > 0 {
                            format!("resets in ~{}h {}m", hrs, mins)
                        } else {
                            format!("resets in ~{}m", mins)
                        }
                    } else {
                        "reset time unknown".to_string()
                    };
                    return Err(ClidoError::RateLimited {
                        message: format!(
                            "Subscription rate limit reached ({}). {}",
                            reset_msg,
                            if body.len() > 200 {
                                &body[..200]
                            } else {
                                &body
                            }
                        ),
                        retry_after_secs,
                        is_subscription_limit: true,
                    });
                }

                rate_limit_attempts += 1;
                if rate_limit_attempts < MAX_RATE_LIMIT_ATTEMPTS {
                    let delay = retry_after.unwrap_or_else(|| {
                        Duration::from_secs(rate_limit_backoff_secs(rate_limit_attempts))
                    });
                    debug!(
                        "Rate limited (attempt {}/{}), waiting {:.0}s…",
                        rate_limit_attempts,
                        MAX_RATE_LIMIT_ATTEMPTS,
                        delay.as_secs_f64()
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(ClidoError::RateLimited {
                    message: format!(
                        "Rate limit exceeded after {} retries. Please wait and try again.",
                        MAX_RATE_LIMIT_ATTEMPTS
                    ),
                    retry_after_secs,
                    is_subscription_limit: false,
                });
            }

            let text = res
                .text()
                .await
                .map_err(|e| ClidoError::Provider(e.to_string()))?;

            // 402 Payment Required or insufficient credits/balance in 4xx body
            if status.as_u16() == 402 || {
                let lower = text.to_lowercase();
                status.is_client_error()
                    && (lower.contains("insufficient")
                        || lower.contains("balance")
                        || lower.contains("credits")
                        || lower.contains("payment")
                        || lower.contains("billing")
                        || lower.contains("exceeded your current usage"))
            } {
                let preview: String = text.chars().take(300).collect();
                return Err(ClidoError::RateLimited {
                    message: format!(
                        "Insufficient credits or payment required ({}): {}",
                        status.as_u16(),
                        preview
                    ),
                    retry_after_secs: None,
                    is_subscription_limit: true,
                });
            }

            if status.is_server_error() {
                server_error_attempts += 1;
                if server_error_attempts < MAX_SERVER_ERROR_ATTEMPTS {
                    let delay_secs = server_error_backoff_secs(server_error_attempts);
                    warn!(
                        "Server error {} (attempt {}/{}), retrying in {}s",
                        status, server_error_attempts, MAX_SERVER_ERROR_ATTEMPTS, delay_secs
                    );
                    tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                    continue;
                }
                return Err(ClidoError::Provider(format!(
                    "API server error ({}) after {} attempts. Please try again later.",
                    status.as_u16(),
                    MAX_SERVER_ERROR_ATTEMPTS
                )));
            }

            if status.is_success() {
                let json: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| ClidoError::Provider(e.to_string()))?;
                return parse_anthropic_response(&json);
            }

            // Other client errors (400, 401, 403, etc.) — don't retry.
            return Err(ClidoError::Provider(extract_api_error(status, &text)));
        }
    }
}

/// Try to extract a clean message from an Anthropic error JSON body.
fn extract_api_error(status: reqwest::StatusCode, text: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(msg) = json["error"]["message"].as_str() {
            return format!("API error ({}): {}", status.as_u16(), msg);
        }
    }
    let preview: String = text.chars().take(300).collect();
    format!("API error ({}): {}", status.as_u16(), preview)
}

/// Produce a human-readable message for a reqwest network error.
fn friendly_network_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "request timed out".to_string()
    } else if e.is_connect() {
        "could not connect to api.anthropic.com — check your internet connection".to_string()
    } else {
        e.to_string()
    }
}

fn message_to_anthropic(m: &Message) -> Result<serde_json::Value> {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    };

    let content: Vec<serde_json::Value> = m
        .content
        .iter()
        .map(content_block_to_anthropic)
        .collect::<Result<Vec<_>>>()?;

    Ok(serde_json::json!({ "role": role, "content": content }))
}

fn content_block_to_anthropic(b: &ContentBlock) -> Result<serde_json::Value> {
    match b {
        ContentBlock::Text { text } => Ok(serde_json::json!({ "type": "text", "text": text })),
        ContentBlock::ToolUse { id, name, input } => Ok(serde_json::json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input
        })),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Ok(serde_json::json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error
        })),
        ContentBlock::Thinking { thinking } => Ok(serde_json::json!({
            "type": "thinking",
            "thinking": thinking
        })),
        ContentBlock::Image {
            media_type,
            base64_data,
        } => Ok(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": base64_data
            }
        })),
    }
}

fn parse_anthropic_response(json: &serde_json::Value) -> Result<ModelResponse> {
    let id = json["id"].as_str().unwrap_or("").to_string();
    let model = json["model"].as_str().unwrap_or("").to_string();

    let content: Vec<ContentBlock> = json["content"]
        .as_array()
        .ok_or_else(|| ClidoError::Provider("missing content".into()))?
        .iter()
        .map(parse_content_block)
        .collect::<Result<Vec<_>>>()?;

    let stop_reason = match json["stop_reason"].as_str().unwrap_or("end_turn") {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    };

    let usage = Usage {
        input_tokens: json["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: json["usage"]["output_tokens"].as_u64().unwrap_or(0),
        cache_creation_input_tokens: json["usage"]["cache_creation_input_tokens"].as_u64(),
        cache_read_input_tokens: json["usage"]["cache_read_input_tokens"].as_u64(),
    };

    Ok(ModelResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

fn parse_content_block(v: &serde_json::Value) -> Result<ContentBlock> {
    let typ = v["type"]
        .as_str()
        .ok_or_else(|| ClidoError::Provider("missing type".into()))?;
    match typ {
        "text" => {
            let text = v["text"].as_str().unwrap_or("").to_string();
            Ok(ContentBlock::Text { text })
        }
        "tool_use" => {
            let id = v["id"].as_str().unwrap_or("").to_string();
            let name = v["name"].as_str().unwrap_or("").to_string();
            let input = v["input"].clone();
            Ok(ContentBlock::ToolUse { id, name, input })
        }
        "thinking" => {
            let thinking = v["thinking"].as_str().unwrap_or("").to_string();
            Ok(ContentBlock::Thinking { thinking })
        }
        // The API never returns image blocks; fall back to empty text so we don't error.
        "image" => Ok(ContentBlock::Text {
            text: String::new(),
        }),
        _ => Err(ClidoError::Provider(format!(
            "unknown content type: {}",
            typ
        ))),
    }
}

/// Parse Anthropic SSE byte stream into a stream of `StreamEvent`.
///
/// Spawns a tokio task that reads chunks, splits on newlines, and assembles
/// `event:`/`data:` pairs. Each blank line terminates an event.
fn parse_anthropic_sse(
    byte_stream: impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static,
) -> impl futures::Stream<Item = Result<StreamEvent>> + Send {
    use futures::channel::mpsc;
    use futures::SinkExt;
    use futures::StreamExt;

    let (mut tx, rx) = mpsc::unbounded::<Result<StreamEvent>>();

    tokio::spawn(async move {
        let mut line_buf = String::new();
        let mut event_type = String::new();
        let mut data_buf = String::new();
        // index → tool_use_id (needed because content_block_delta only carries index)
        let mut index_to_id: std::collections::HashMap<u64, String> =
            std::collections::HashMap::new();
        // If no bytes arrive for this long, treat it as a stall and abort.
        const STREAM_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

        let mut stream = std::pin::pin!(byte_stream);
        loop {
            let chunk = match tokio::time::timeout(STREAM_CHUNK_TIMEOUT, stream.next()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break, // stream ended naturally
                Err(_elapsed) => {
                    let _ = tx
                        .send(Err(ClidoError::Provider(
                            "streaming stalled — no data received for 90 seconds".to_string(),
                        )))
                        .await;
                    return;
                }
            };
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    let err: Result<StreamEvent> = Err(ClidoError::Provider(e.to_string()));
                    let _ = tx.send(err).await;
                    return;
                }
            };

            // Append chunk bytes to the line buffer and process complete lines.
            line_buf.push_str(&String::from_utf8_lossy(&bytes));

            loop {
                let Some(pos) = line_buf.find('\n') else {
                    break;
                };
                let line = line_buf[..pos].trim_end_matches('\r').to_string();
                line_buf = line_buf[pos + 1..].to_string();

                if line.is_empty() {
                    // Blank line = end of SSE event; process event + data.
                    if !data_buf.is_empty() {
                        // Handle Anthropic streaming error events (rate limit, overload, etc.)
                        if event_type == "error" {
                            let msg = if let Ok(json) =
                                serde_json::from_str::<serde_json::Value>(&data_buf)
                            {
                                let err_type = json["error"]["type"].as_str().unwrap_or("unknown");
                                let err_msg =
                                    json["error"]["message"].as_str().unwrap_or(&data_buf);
                                if err_type == "rate_limit_error" || err_type == "overloaded_error"
                                {
                                    let _ = tx
                                        .send(Err(ClidoError::RateLimited {
                                            message: format!("streaming {}: {}", err_type, err_msg),
                                            retry_after_secs: None,
                                            is_subscription_limit: false,
                                        }))
                                        .await;
                                    return;
                                }
                                format!("streaming error ({}): {}", err_type, err_msg)
                            } else {
                                format!("streaming error: {}", data_buf)
                            };
                            let _ = tx.send(Err(ClidoError::Provider(msg))).await;
                            return;
                        }
                        if let Some(events) =
                            decode_anthropic_event(&event_type, &data_buf, &mut index_to_id)
                        {
                            for ev in events {
                                if tx.send(Ok(ev)).await.is_err() {
                                    return; // receiver dropped
                                }
                            }
                        }
                    }
                    event_type.clear();
                    data_buf.clear();
                } else if let Some(rest) = line.strip_prefix("event:") {
                    event_type = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data:") {
                    if !data_buf.is_empty() {
                        data_buf.push('\n');
                    }
                    data_buf.push_str(rest.trim());
                }
            }
        }
    });

    rx
}

/// Decode one Anthropic SSE event into zero or more `StreamEvent` values.
fn decode_anthropic_event(
    event_type: &str,
    data: &str,
    index_to_id: &mut std::collections::HashMap<u64, String>,
) -> Option<Vec<StreamEvent>> {
    let json: serde_json::Value = serde_json::from_str(data).ok()?;
    let mut out = Vec::new();

    match event_type {
        "content_block_start" => {
            let block = &json["content_block"];
            let index = json["index"].as_u64().unwrap_or(0);
            if block["type"].as_str() == Some("tool_use") {
                let id = block["id"].as_str().unwrap_or("").to_string();
                let name = block["name"].as_str().unwrap_or("").to_string();
                index_to_id.insert(index, id.clone());
                out.push(StreamEvent::ToolUseStart { id, name });
            }
        }
        "content_block_delta" => {
            let delta = &json["delta"];
            let index = json["index"].as_u64().unwrap_or(0);
            match delta["type"].as_str() {
                Some("text_delta") => {
                    let text = delta["text"].as_str().unwrap_or("").to_string();
                    if !text.is_empty() {
                        out.push(StreamEvent::TextDelta(text));
                    }
                }
                Some("input_json_delta") => {
                    if let Some(id) = index_to_id.get(&index).cloned() {
                        let partial_json = delta["partial_json"].as_str().unwrap_or("").to_string();
                        out.push(StreamEvent::ToolUseDelta { id, partial_json });
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            let index = json["index"].as_u64().unwrap_or(0);
            if let Some(id) = index_to_id.get(&index).cloned() {
                out.push(StreamEvent::ToolUseEnd { id });
            }
        }
        "message_delta" => {
            let stop_reason = match json["delta"]["stop_reason"].as_str().unwrap_or("end_turn") {
                "tool_use" => StopReason::ToolUse,
                "max_tokens" => StopReason::MaxTokens,
                "stop_sequence" => StopReason::StopSequence,
                _ => StopReason::EndTurn,
            };
            let usage = Usage {
                input_tokens: 0,
                output_tokens: json["usage"]["output_tokens"].as_u64().unwrap_or(0),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            };
            out.push(StreamEvent::MessageDelta { stop_reason, usage });
        }
        // "message_start", "message_stop", "ping" — no StreamEvent to emit
        _ => {}
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> Result<ModelResponse> {
        self.complete_impl(messages, tools, config).await
    }

    async fn complete_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        // Build the same body as complete_impl, but with "stream": true.
        let mut system = config
            .system_prompt
            .as_deref()
            .unwrap_or("You are a helpful coding assistant.")
            .to_string();
        for m in messages.iter() {
            if m.role == Role::System {
                for b in &m.content {
                    if let ContentBlock::Text { text } = b {
                        system.push('\n');
                        system.push_str(text);
                    }
                }
            }
        }
        let anthropic_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(message_to_anthropic)
            .collect::<Result<Vec<_>>>()?;

        let anthropic_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema
                })
            })
            .collect();

        let system_blocks = serde_json::json!([{
            "type": "text",
            "text": system,
            "cache_control": {"type": "ephemeral"}
        }]);

        let max_tokens = config.max_output_tokens.unwrap_or(8192);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": system_blocks,
            "messages": anthropic_messages,
            "tools": anthropic_tools,
            "stream": true
        });

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ClidoError::Provider(format!("stream request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = crate::backoff::parse_retry_after_secs(response.headers());
            let text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                let lower = text.to_lowercase();
                let is_sub = retry_after.is_some_and(|s| s > 300)
                    || lower.contains("quota")
                    || lower.contains("subscription")
                    || lower.contains("limit exceeded")
                    || lower.contains("allowance");
                return Err(ClidoError::RateLimited {
                    message: format!(
                        "429 (model: {}): {}",
                        self.model,
                        text.chars().take(300).collect::<String>()
                    ),
                    retry_after_secs: retry_after,
                    is_subscription_limit: is_sub,
                });
            }
            return Err(ClidoError::Provider(extract_api_error(status, &text)));
        }

        let byte_stream = response.bytes_stream();
        Ok(Box::pin(parse_anthropic_sse(byte_stream)))
    }

    async fn list_models(&self) -> Vec<crate::provider::ModelEntry> {
        let resp = match self
            .client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("list_models: request failed: {}", e);
                return vec![];
            }
        };
        if !resp.status().is_success() {
            warn!("list_models: API returned status {}", resp.status());
            return vec![];
        }
        let json = match resp.json::<serde_json::Value>().await {
            Ok(j) => j,
            Err(e) => {
                warn!("list_models: failed to parse response: {}", e);
                return vec![];
            }
        };
        let mut models: Vec<crate::provider::ModelEntry> = json["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| m["id"].as_str().map(crate::provider::ModelEntry::available))
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clido_core::{ContentBlock, Role};

    // ── extract_api_error ──────────────────────────────────────────────────────

    #[test]
    fn extract_api_error_from_json_body() {
        let status = reqwest::StatusCode::from_u16(401).unwrap();
        let body = r#"{"error":{"message":"invalid api key","type":"authentication_error"}}"#;
        let msg = extract_api_error(status, body);
        assert!(msg.contains("401"));
        assert!(msg.contains("invalid api key"));
    }

    #[test]
    fn extract_api_error_non_json_body() {
        let status = reqwest::StatusCode::from_u16(500).unwrap();
        let body = "Internal Server Error";
        let msg = extract_api_error(status, body);
        assert!(msg.contains("500"));
        assert!(msg.contains("Internal Server Error"));
    }

    #[test]
    fn extract_api_error_truncates_long_body() {
        let status = reqwest::StatusCode::from_u16(400).unwrap();
        let long_body = "x".repeat(500);
        let msg = extract_api_error(status, &long_body);
        // Should not panic and should include status
        assert!(msg.contains("400"));
    }

    // ── message_to_anthropic ───────────────────────────────────────────────────

    #[test]
    fn message_to_anthropic_user_role() {
        let m = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        };
        let v = message_to_anthropic(&m).unwrap();
        assert_eq!(v["role"], "user");
        assert!(v["content"].is_array());
    }

    #[test]
    fn message_to_anthropic_assistant_role() {
        let m = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        };
        let v = message_to_anthropic(&m).unwrap();
        assert_eq!(v["role"], "assistant");
    }

    #[test]
    fn message_to_anthropic_system_role() {
        let m = Message {
            role: Role::System,
            content: vec![ContentBlock::Text {
                text: "sys".to_string(),
            }],
        };
        let v = message_to_anthropic(&m).unwrap();
        assert_eq!(v["role"], "system");
    }

    // ── content_block_to_anthropic ─────────────────────────────────────────────

    #[test]
    fn content_block_text_to_anthropic() {
        let b = ContentBlock::Text {
            text: "hello".to_string(),
        };
        let v = content_block_to_anthropic(&b).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hello");
    }

    #[test]
    fn content_block_tool_use_to_anthropic() {
        let b = ContentBlock::ToolUse {
            id: "tu_1".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"path": "foo.rs"}),
        };
        let v = content_block_to_anthropic(&b).unwrap();
        assert_eq!(v["type"], "tool_use");
        assert_eq!(v["id"], "tu_1");
        assert_eq!(v["name"], "Read");
    }

    #[test]
    fn content_block_tool_result_to_anthropic() {
        let b = ContentBlock::ToolResult {
            tool_use_id: "tu_1".to_string(),
            content: "file content".to_string(),
            is_error: false,
        };
        let v = content_block_to_anthropic(&b).unwrap();
        assert_eq!(v["type"], "tool_result");
        assert_eq!(v["tool_use_id"], "tu_1");
        assert_eq!(v["is_error"], false);
    }

    #[test]
    fn content_block_thinking_to_anthropic() {
        let b = ContentBlock::Thinking {
            thinking: "hmm".to_string(),
        };
        let v = content_block_to_anthropic(&b).unwrap();
        assert_eq!(v["type"], "thinking");
        assert_eq!(v["thinking"], "hmm");
    }

    #[test]
    fn content_block_image_to_anthropic() {
        let b = ContentBlock::Image {
            media_type: "image/png".to_string(),
            base64_data: "abc123".to_string(),
        };
        let v = content_block_to_anthropic(&b).unwrap();
        assert_eq!(v["type"], "image");
        assert_eq!(v["source"]["type"], "base64");
        assert_eq!(v["source"]["media_type"], "image/png");
        assert_eq!(v["source"]["data"], "abc123");
    }

    // ── parse_anthropic_response ───────────────────────────────────────────────

    #[test]
    fn parse_anthropic_response_end_turn() {
        let json = serde_json::json!({
            "id": "msg_1",
            "model": "claude-3-5-sonnet",
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let resp = parse_anthropic_response(&json).unwrap();
        assert_eq!(resp.id, "msg_1");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn parse_anthropic_response_tool_use() {
        let json = serde_json::json!({
            "id": "msg_2",
            "model": "claude-3-5-sonnet",
            "content": [{"type": "tool_use", "id": "tu_1", "name": "Read", "input": {"path": "x"}}],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "output_tokens": 8}
        });
        let resp = parse_anthropic_response(&json).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.content.len(), 1);
    }

    #[test]
    fn parse_anthropic_response_max_tokens() {
        let json = serde_json::json!({
            "id": "msg_3",
            "model": "m",
            "content": [{"type": "text", "text": "cut off"}],
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let resp = parse_anthropic_response(&json).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn parse_anthropic_response_stop_sequence() {
        let json = serde_json::json!({
            "id": "msg_4",
            "model": "m",
            "content": [{"type": "text", "text": "done"}],
            "stop_reason": "stop_sequence",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let resp = parse_anthropic_response(&json).unwrap();
        assert_eq!(resp.stop_reason, StopReason::StopSequence);
    }

    #[test]
    fn parse_anthropic_response_unknown_stop_reason_defaults_to_end_turn() {
        let json = serde_json::json!({
            "id": "msg_5",
            "model": "m",
            "content": [{"type": "text", "text": "x"}],
            "stop_reason": "some_new_reason",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let resp = parse_anthropic_response(&json).unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn parse_anthropic_response_missing_content_is_error() {
        let json = serde_json::json!({
            "id": "msg_6",
            "model": "m",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let result = parse_anthropic_response(&json);
        assert!(result.is_err());
    }

    #[test]
    fn parse_anthropic_response_with_cache_tokens() {
        let json = serde_json::json!({
            "id": "msg_7",
            "model": "m",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_creation_input_tokens": 200,
                "cache_read_input_tokens": 300
            }
        });
        let resp = parse_anthropic_response(&json).unwrap();
        assert_eq!(resp.usage.cache_creation_input_tokens, Some(200));
        assert_eq!(resp.usage.cache_read_input_tokens, Some(300));
    }

    #[test]
    fn parse_content_block_thinking() {
        let v = serde_json::json!({"type": "thinking", "thinking": "hmm"});
        let b = parse_content_block(&v).unwrap();
        assert!(matches!(b, ContentBlock::Thinking { .. }));
    }

    #[test]
    fn parse_content_block_image_becomes_empty_text() {
        let v = serde_json::json!({"type": "image", "source": {}});
        let b = parse_content_block(&v).unwrap();
        // Image API blocks are mapped to empty Text
        assert!(matches!(b, ContentBlock::Text { text } if text.is_empty()));
    }

    #[test]
    fn parse_content_block_unknown_type_is_error() {
        let v = serde_json::json!({"type": "unknown_type"});
        let result = parse_content_block(&v);
        assert!(result.is_err());
    }

    #[test]
    fn parse_content_block_missing_type_is_error() {
        let v = serde_json::json!({"no_type_field": "x"});
        let result = parse_content_block(&v);
        assert!(result.is_err());
    }

    // ── AnthropicProvider::new ─────────────────────────────────────────────────

    #[test]
    fn anthropic_provider_new() {
        let p = AnthropicProvider::new("sk-ant-fake".to_string(), "claude-3-haiku".to_string());
        // Just assert it constructs without panic.
        let _ = p;
    }
}
