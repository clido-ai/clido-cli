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
use tracing::warn;

use crate::provider::{ModelProvider, StreamEvent};

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model,
        }
    }

    /// Build request body and POST to API.
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

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 8192,
            "system": system,
            "messages": anthropic_messages,
            "tools": anthropic_tools
        });

        const MAX_ATTEMPTS: u32 = 3;
        let mut attempt = 0u32;
        loop {
            let res = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| ClidoError::Provider(e.to_string()))?;

            let status = res.status();
            let text = res
                .text()
                .await
                .map_err(|e| ClidoError::Provider(e.to_string()))?;

            if status.is_success() {
                let json: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| ClidoError::Provider(e.to_string()))?;
                return parse_anthropic_response(&json);
            }

            let retriable = status.as_u16() == 429 || status.is_server_error();
            if retriable && attempt < MAX_ATTEMPTS - 1 {
                attempt += 1;
                let delay_ms = match attempt {
                    1 => 1000,
                    2 => 2000,
                    _ => 4000,
                };
                warn!(
                    "Provider {} (attempt {}/{}), retrying in {}ms: {}",
                    status,
                    attempt,
                    MAX_ATTEMPTS,
                    delay_ms,
                    text.chars().take(200).collect::<String>()
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                continue;
            }

            return Err(ClidoError::Provider(format!(
                "API error {}: {}",
                status, text
            )));
        }
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
