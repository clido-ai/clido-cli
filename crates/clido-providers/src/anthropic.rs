//! Anthropic Messages API client.

use async_trait::async_trait;
use clido_core::{
    AgentConfig, ContentBlock, Message, ModelResponse, Role, StopReason, ToolSchema, Usage,
};
use clido_core::{ClidoError, Result};
use futures::stream;
use futures::Stream;
use std::pin::Pin;
use std::time::Duration;
use tracing::{info, warn};

use crate::provider::{ModelProvider, StreamEvent};

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        let client = reqwest::Client::builder()
            // Total request timeout (connect + send + body read).
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
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

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 8192,
            "system": system_blocks,
            "messages": anthropic_messages,
            "tools": anthropic_tools
        });

        // Max attempts for each failure category.
        const MAX_RATE_LIMIT_ATTEMPTS: u32 = 6;
        const MAX_SERVER_ERROR_ATTEMPTS: u32 = 5;
        const MAX_NETWORK_ATTEMPTS: u32 = 4;

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
                let _text = res.text().await.unwrap_or_default();
                rate_limit_attempts += 1;
                if rate_limit_attempts < MAX_RATE_LIMIT_ATTEMPTS {
                    let delay = retry_after.unwrap_or_else(|| {
                        Duration::from_secs(rate_limit_backoff_secs(rate_limit_attempts))
                    });
                    info!(
                        "Rate limited (attempt {}/{}), waiting {:.0}s…",
                        rate_limit_attempts,
                        MAX_RATE_LIMIT_ATTEMPTS,
                        delay.as_secs_f64()
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(ClidoError::Provider(format!(
                    "Rate limit exceeded after {} attempts. Please wait and try again.",
                    MAX_RATE_LIMIT_ATTEMPTS
                )));
            }

            let text = res
                .text()
                .await
                .map_err(|e| ClidoError::Provider(e.to_string()))?;

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

/// Parse `Retry-After` header value (integer seconds only; HTTP-date not supported).
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        // Cap at 5 minutes to avoid waiting forever on a bad header value.
        .map(|secs| Duration::from_secs(secs.min(300)))
}

/// Backoff for rate limits: 15s, 30s, 60s, 90s, 120s, …
fn rate_limit_backoff_secs(attempt: u32) -> u64 {
    let base: u64 = 15 * (1u64 << (attempt - 1).min(3));
    base.min(120)
}

/// Backoff for server errors: 1s, 2s, 4s, 8s, 16s.
fn server_error_backoff_secs(attempt: u32) -> u64 {
    (1u64 << (attempt - 1).min(4)).min(16)
}

/// Backoff for network errors: 1s, 2s, 4s.
fn network_backoff_secs(attempt: u32) -> u64 {
    (1u64 << (attempt - 1).min(2)).min(4)
}

/// Try to extract a clean message from an Anthropic error JSON body.
fn extract_api_error(status: reqwest::StatusCode, text: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(msg) = json["error"]["message"].as_str() {
            return format!("API error ({}): {}", status.as_u16(), msg);
        }
    }
    let preview = &text[..text.len().min(300)];
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
        _messages: &[Message],
        _tools: &[ToolSchema],
        _config: &AgentConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::empty()))
    }
}
