//! OpenAI-compatible chat completions API (OpenRouter, OpenAI, etc.).

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

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// OpenAI-compatible chat provider (OpenRouter, OpenAI, local OpenAI-compatible endpoints).
pub struct OpenAICompatProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
    extra_headers: Vec<(String, String)>,
}

impl OpenAICompatProvider {
    /// Generic constructor for any OpenAI-compatible endpoint.
    pub fn new(
        api_key: String,
        model: String,
        base_url: String,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self {
            client,
            api_key,
            model,
            base_url,
            extra_headers,
        }
    }

    /// OpenRouter: same API shape, fixed base URL and required headers.
    pub fn new_openrouter(api_key: String, model: String) -> Self {
        let extra_headers = vec![
            (
                "HTTP-Referer".to_string(),
                "https://github.com/clido".to_string(),
            ),
            ("X-Title".to_string(), "Clido".to_string()),
        ];
        Self::new(
            api_key,
            model,
            OPENROUTER_BASE_URL.to_string(),
            extra_headers,
        )
    }

    fn request_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{}/chat/completions", base)
    }

    async fn complete_impl(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> Result<ModelResponse> {
        let (system_from_messages, openai_messages) = messages_to_openai(messages)?;
        let system_content = {
            let base = config
                .system_prompt
                .as_deref()
                .unwrap_or("You are clido, an AI coding agent. Always refer to yourself as clido.");
            match &system_from_messages {
                Some(s) if !s.is_empty() => format!("{}\n{}", base, s),
                _ => base.to_string(),
            }
        };
        let openai_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    }
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": 8192,
            "messages": openai_messages
        });
        body["messages"].as_array_mut().unwrap().insert(
            0,
            serde_json::json!({ "role": "system", "content": system_content }),
        );
        if !openai_tools.is_empty() {
            body["tools"] = serde_json::Value::Array(openai_tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        const MAX_RATE_LIMIT_ATTEMPTS: u32 = 6;
        const MAX_SERVER_ERROR_ATTEMPTS: u32 = 5;
        const MAX_NETWORK_ATTEMPTS: u32 = 4;

        let mut rate_limit_attempts = 0u32;
        let mut server_error_attempts = 0u32;
        let mut network_attempts = 0u32;
        let url = self.request_url();
        loop {
            let mut req = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json");
            for (k, v) in &self.extra_headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let res = match req.json(&body).send().await {
                Ok(r) => r,
                Err(e) => {
                    network_attempts += 1;
                    if network_attempts < MAX_NETWORK_ATTEMPTS {
                        let delay = network_backoff_secs(network_attempts);
                        warn!(
                            "Network error (attempt {}/{}), retrying in {}s: {}",
                            network_attempts, MAX_NETWORK_ATTEMPTS, delay, e
                        );
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        continue;
                    }
                    return Err(ClidoError::Provider(format!(
                        "Connection failed after {} attempts: {}",
                        MAX_NETWORK_ATTEMPTS,
                        if e.is_timeout() {
                            "request timed out".to_string()
                        } else if e.is_connect() {
                            "could not connect — check your internet connection".to_string()
                        } else {
                            e.to_string()
                        }
                    )));
                }
            };

            let status = res.status();

            if status.as_u16() == 429 {
                let retry_after = res.headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .map(|s| Duration::from_secs(s.min(300)));
                let _body = res.text().await.unwrap_or_default();
                rate_limit_attempts += 1;
                if rate_limit_attempts < MAX_RATE_LIMIT_ATTEMPTS {
                    let delay = retry_after.unwrap_or_else(|| {
                        Duration::from_secs(rate_limit_backoff_secs(rate_limit_attempts))
                    });
                    tracing::info!(
                        "Rate limited (attempt {}/{}), waiting {:.0}s…",
                        rate_limit_attempts, MAX_RATE_LIMIT_ATTEMPTS, delay.as_secs_f64()
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(ClidoError::Provider(format!(
                    "Rate limit exceeded after {} attempts. Please wait and try again.",
                    MAX_RATE_LIMIT_ATTEMPTS
                )));
            }

            let text = res.text().await.map_err(|e| ClidoError::Provider(e.to_string()))?;

            if status.is_server_error() {
                server_error_attempts += 1;
                if server_error_attempts < MAX_SERVER_ERROR_ATTEMPTS {
                    let delay = server_error_backoff_secs(server_error_attempts);
                    warn!(
                        "Server error {} (attempt {}/{}), retrying in {}s",
                        status, server_error_attempts, MAX_SERVER_ERROR_ATTEMPTS, delay
                    );
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    continue;
                }
                return Err(ClidoError::Provider(format!(
                    "API server error ({}) after {} attempts. Please try again later.",
                    status.as_u16(), MAX_SERVER_ERROR_ATTEMPTS
                )));
            }

            if status.is_success() {
                let json: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| ClidoError::Provider(e.to_string()))?;
                return parse_openai_response(&json);
            }

            // Non-retriable client error.
            let msg = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(m) = json["error"]["message"].as_str() {
                    format!("API error ({}): {}", status.as_u16(), m)
                } else {
                    format!("API error ({}): {}", status.as_u16(), &text[..text.len().min(300)])
                }
            } else {
                format!("API error {} (model: {}): {}", status, self.model, &text[..text.len().min(300)])
            };
            return Err(ClidoError::Provider(msg));
        }
    }
}

fn rate_limit_backoff_secs(attempt: u32) -> u64 {
    (15u64 * (1u64 << (attempt - 1).min(3))).min(120)
}

fn server_error_backoff_secs(attempt: u32) -> u64 {
    (1u64 << (attempt - 1).min(4)).min(16)
}

fn network_backoff_secs(attempt: u32) -> u64 {
    (1u64 << (attempt - 1).min(2)).min(4)
}

/// Convert Clido messages to OpenAI chat format. Returns (system_content, messages).
/// System content is merged into one string; messages are user/assistant/tool only.
fn messages_to_openai(messages: &[Message]) -> Result<(Option<String>, Vec<serde_json::Value>)> {
    let mut system_parts: Vec<String> = Vec::new();
    let mut openai_messages: Vec<serde_json::Value> = Vec::new();

    for m in messages.iter() {
        match m.role {
            Role::System => {
                for b in &m.content {
                    if let ContentBlock::Text { text } = b {
                        system_parts.push(text.clone());
                    }
                }
            }
            Role::User => {
                let has_tool_result = m
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
                if has_tool_result {
                    for b in &m.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = b
                        {
                            openai_messages.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content
                            }));
                        }
                    }
                } else {
                    let content: String = m
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    openai_messages.push(serde_json::json!({
                        "role": "user",
                        "content": content
                    }));
                }
            }
            Role::Assistant => {
                let text_parts: Vec<&str> = m
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                let text_content = text_parts.join("");
                let tool_calls: Vec<serde_json::Value> = m
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::ToolUse { id, name, input } = b {
                            Some(serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into())
                                }
                            }))
                        } else {
                            None
                        }
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    let content_val = if text_content.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(text_content)
                    };
                    openai_messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content_val,
                        "tool_calls": tool_calls
                    }));
                } else {
                    openai_messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": text_content
                    }));
                }
            }
        }
    }

    let system_content = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n"))
    };
    Ok((system_content, openai_messages))
}

fn parse_openai_response(json: &serde_json::Value) -> Result<ModelResponse> {
    let id = json["id"].as_str().unwrap_or("").to_string();
    let model = json["model"].as_str().unwrap_or("").to_string();
    let choices = json["choices"]
        .as_array()
        .ok_or_else(|| ClidoError::Provider("missing choices".into()))?;
    let choice = choices
        .first()
        .ok_or_else(|| ClidoError::Provider("empty choices".into()))?;
    let message = &choice["message"];
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");
    let stop_reason = match finish_reason {
        "tool_calls" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        _ => StopReason::EndTurn,
    };

    let content = message_to_content_blocks(message)?;
    let usage = Usage {
        input_tokens: json["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: json["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    };

    Ok(ModelResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

fn message_to_content_blocks(message: &serde_json::Value) -> Result<Vec<ContentBlock>> {
    let mut blocks = Vec::new();
    let content_val = &message["content"];
    if let Some(text) = content_val.as_str() {
        if !text.is_empty() {
            blocks.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }
    } else if let Some(arr) = content_val.as_array() {
        for part in arr {
            let typ = part["type"].as_str().unwrap_or("");
            if typ == "text" {
                if let Some(t) = part["text"].as_str() {
                    blocks.push(ContentBlock::Text {
                        text: t.to_string(),
                    });
                }
            }
        }
    }
    if let Some(tool_calls) = message["tool_calls"].as_array() {
        for tc in tool_calls {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let input = serde_json::from_str(args_str).unwrap_or(serde_json::Value::Null);
            blocks.push(ContentBlock::ToolUse { id, name, input });
        }
    }
    Ok(blocks)
}

#[async_trait]
impl ModelProvider for OpenAICompatProvider {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_to_openai_user_text() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];
        let (sys, msgs) = messages_to_openai(&messages).unwrap();
        assert!(sys.is_none());
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "Hello");
    }

    #[test]
    fn messages_to_openai_assistant_tool_use() {
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Read foo".to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"path": "foo"}),
                }],
            },
        ];
        let (_, msgs) = messages_to_openai(&messages).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["role"], "assistant");
        assert!(msgs[1]["tool_calls"].is_array());
        assert_eq!(msgs[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(msgs[1]["tool_calls"][0]["function"]["name"], "Read");
    }

    #[test]
    fn messages_to_openai_tool_results() {
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "file content".to_string(),
                    is_error: false,
                }],
            },
        ];
        let (_, msgs) = messages_to_openai(&messages).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "call_1");
        assert_eq!(msgs[1]["content"], "file content");
    }

    #[test]
    fn parse_openai_response_text() {
        let json = serde_json::json!({
            "id": "gen-1",
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello world"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        });
        let r = parse_openai_response(&json).unwrap();
        assert_eq!(r.id, "gen-1");
        assert_eq!(r.model, "gpt-4");
        assert_eq!(r.usage.input_tokens, 10);
        assert_eq!(r.usage.output_tokens, 5);
        assert!(matches!(r.stop_reason, StopReason::EndTurn));
        assert_eq!(r.content.len(), 1);
        if let ContentBlock::Text { text } = &r.content[0] {
            assert_eq!(text, "Hello world");
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn parse_openai_response_tool_calls() {
        let json = serde_json::json!({
            "id": "gen-2",
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "Read",
                            "arguments": "{\"path\": \"src/main.rs\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 2 }
        });
        let r = parse_openai_response(&json).unwrap();
        assert!(matches!(r.stop_reason, StopReason::ToolUse));
        assert_eq!(r.content.len(), 1);
        if let ContentBlock::ToolUse { id, name, input } = &r.content[0] {
            assert_eq!(id, "call_abc");
            assert_eq!(name, "Read");
            assert_eq!(input["path"], "src/main.rs");
        } else {
            panic!("expected tool_use block");
        }
    }
}
