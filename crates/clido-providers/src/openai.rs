//! OpenAI-compatible chat completions API (OpenRouter, OpenAI, etc.).

use async_trait::async_trait;
use clido_core::{
    AgentConfig, ContentBlock, Message, ModelResponse, Role, StopReason, ToolSchema, Usage,
};
use clido_core::{ClidoError, Result};
use futures::Stream;
use std::pin::Pin;
use std::time::Duration;

use crate::backoff::{
    network_backoff_secs, parse_retry_after_secs, rate_limit_backoff_secs,
    server_error_backoff_secs, MAX_NETWORK_ATTEMPTS, MAX_RATE_LIMIT_ATTEMPTS,
    MAX_SERVER_ERROR_ATTEMPTS,
};
use tracing::warn;

use crate::provider::{ModelProvider, StreamEvent};

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";

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
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        Self::new_with_user_agent(
            api_key,
            model,
            base_url,
            extra_headers,
            &format!("clido/{}", env!("CARGO_PKG_VERSION")),
        )
    }

    /// Like [`new`] but with an explicit User-Agent header.
    pub fn new_with_user_agent(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
        extra_headers: Vec<(String, String)>,
        user_agent: &str,
    ) -> Self {
        let client = crate::http_client::build_http_client(user_agent);
        Self {
            client,
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.into(),
            extra_headers,
        }
    }

    /// OpenRouter: same API shape, fixed base URL and required headers.
    pub fn new_openrouter(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let extra_headers = vec![
            (
                "HTTP-Referer".to_string(),
                "https://github.com/clido-ai/clido-cli".to_string(),
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

    /// OpenAI: standard base URL, Bearer auth.
    pub fn new_openai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(api_key, model, OPENAI_BASE_URL.to_string(), vec![])
    }

    /// Mistral: OpenAI-compatible API.
    pub fn new_mistral(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(api_key, model, MISTRAL_BASE_URL.to_string(), vec![])
    }

    /// MiniMax: OpenAI-compatible API.
    pub fn new_minimax(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.minimax.io/v1".to_string(),
            vec![],
        )
    }

    /// Kimi (Moonshot AI): OpenAI-compatible API.
    pub fn new_kimi(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.moonshot.ai/v1".to_string(),
            vec![],
        )
    }

    /// Kimi Code: coding-optimised Kimi endpoint.
    pub fn new_kimi_code(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.kimi.com/coding/v1".to_string(),
            vec![],
        )
    }

    /// DeepSeek: OpenAI-compatible API.
    pub fn new_deepseek(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.deepseek.com/v1".to_string(),
            vec![],
        )
    }

    /// Groq: fast inference, OpenAI-compatible API.
    pub fn new_groq(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.groq.com/openai/v1".to_string(),
            vec![],
        )
    }

    /// Cerebras: OpenAI-compatible API.
    pub fn new_cerebras(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.cerebras.ai/v1".to_string(),
            vec![],
        )
    }

    /// Together AI: OpenAI-compatible API.
    pub fn new_togetherai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.together.xyz/v1".to_string(),
            vec![],
        )
    }

    /// Fireworks AI: OpenAI-compatible API.
    pub fn new_fireworks(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.fireworks.ai/inference/v1".to_string(),
            vec![],
        )
    }

    /// xAI (Grok): OpenAI-compatible API.
    pub fn new_xai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(api_key, model, "https://api.x.ai/v1".to_string(), vec![])
    }

    /// Perplexity: OpenAI-compatible API.
    pub fn new_perplexity(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://api.perplexity.ai".to_string(),
            vec![],
        )
    }

    /// Google Gemini: OpenAI-compatible API.
    pub fn new_gemini(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(
            api_key,
            model,
            "https://generativelanguage.googleapis.com/v1beta/openai/".to_string(),
            vec![],
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

        let max_tokens = config.max_output_tokens.unwrap_or(8192);
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "messages": openai_messages
        });
        body["messages"]
            .as_array_mut()
            .expect("messages field must be an array")
            .insert(
                0,
                serde_json::json!({ "role": "system", "content": system_content }),
            );
        if !openai_tools.is_empty() {
            body["tools"] = serde_json::Value::Array(openai_tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        let mut rate_limit_attempts = 0u32;
        let mut server_error_attempts = 0u32;
        let mut network_attempts = 0u32;
        let mut timeout_attempts = 0u32;
        const MAX_TIMEOUT_ATTEMPTS: u32 = 2;
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
                    // Handle timeouts separately with their own retry logic
                    if e.is_timeout() {
                        timeout_attempts += 1;
                        if timeout_attempts < MAX_TIMEOUT_ATTEMPTS {
                            warn!(
                                "Request timeout (attempt {}/{}), retrying immediately...",
                                timeout_attempts, MAX_TIMEOUT_ATTEMPTS
                            );
                            // No delay for timeouts - just retry immediately
                            continue;
                        }
                        return Err(ClidoError::Provider(format!(
                            "Request timed out after {} attempts ({}s each). The API may be experiencing issues.",
                            MAX_TIMEOUT_ATTEMPTS, 420
                        )));
                    }

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
                        if e.is_connect() {
                            "could not connect — check your internet connection".to_string()
                        } else {
                            e.to_string()
                        }
                    )));
                }
            };

            let status = res.status();

            if status.as_u16() == 429 {
                let retry_after_secs = parse_retry_after_secs(res.headers());
                let retry_after = retry_after_secs.map(|s| Duration::from_secs(s.min(300)));
                let body = res.text().await.unwrap_or_default();

                // Subscription/quota limits have long reset times (>5min) or
                // specific error messages about quotas/subscriptions.
                let is_subscription =
                    crate::backoff::is_subscription_limit(retry_after_secs, &body);

                if is_subscription {
                    // Don't retry subscription limits — they won't reset soon.
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
                    tracing::debug!(
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
                    status.as_u16(),
                    MAX_SERVER_ERROR_ATTEMPTS
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
                    let preview: String = text.chars().take(300).collect();
                    format!("API error ({}): {}", status.as_u16(), preview)
                }
            } else {
                let preview: String = text.chars().take(300).collect();
                format!("API error {} (model: {}): {}", status, self.model, preview)
            };
            return Err(ClidoError::Provider(msg));
        }
    }
}

/// Filter for OpenAI chat/completion models — skip embeddings, image, audio, etc.
/// Determine whether an OpenRouter model entry has usable chat endpoints.
///
/// OpenRouter's `/api/v1/models` response includes:
/// - `supported_generation_types`: array of strings (e.g. `["text"]`).
///   If present and empty, or does not contain `"text"`, the model has no
///   chat completion endpoint.
/// - `per_request_limits`: object when the model is accessible; null when
///   it is not available to the current account/key.
///
/// A model passes if either check says it's available. Both being absent
/// means the field schema is not yet known — assume available (safe default).
fn openrouter_model_available(m: &serde_json::Value) -> bool {
    // Check supported_generation_types if present.
    if let Some(types) = m["supported_generation_types"].as_array() {
        return types.iter().any(|t| t.as_str() == Some("text"));
    }
    // Fallback: per_request_limits is null ↔ model is not accessible.
    if !m["per_request_limits"].is_null() {
        return true;
    }
    // Neither field present — assume available.
    true
}

fn is_openai_chat_model(id: &str) -> bool {
    id.starts_with("gpt-")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("chatgpt-")
}

/// Convert Clido messages to OpenAI chat format. Returns (system_content, messages).
/// System content is merged into one string; messages are user/assistant/tool only.
fn messages_to_openai(messages: &[Message]) -> Result<(Option<String>, Vec<serde_json::Value>)> {
    let mut system_parts: Vec<String> = Vec::new();
    let mut openai_messages: Vec<serde_json::Value> = Vec::new();

    // Some providers (e.g. Kimi kimi-for-coding) require `reasoning_content` to be present
    // in *every* assistant message with tool_calls when thinking is active in the session.
    // Detect whether any message in this conversation has a thinking block so we can
    // include the field (even as empty string) where it would otherwise be absent.
    let has_thinking_in_conv = messages.iter().any(|m| {
        m.role == Role::Assistant
            && m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking { .. }))
    });

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
                let thinking_parts: Vec<&str> = m
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Thinking { thinking } = b {
                            Some(thinking.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                let reasoning_content = thinking_parts.join("");
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
                    let mut msg = serde_json::json!({
                        "role": "assistant",
                        "content": content_val,
                        "tool_calls": tool_calls
                    });
                    // Always include reasoning_content when thinking is active in the
                    // conversation — some providers (e.g. Kimi kimi-for-coding) require
                    // the field to be present on every assistant tool_calls message.
                    if !reasoning_content.is_empty() || has_thinking_in_conv {
                        msg["reasoning_content"] = serde_json::Value::String(reasoning_content);
                    }
                    openai_messages.push(msg);
                } else {
                    let mut msg = serde_json::json!({
                        "role": "assistant",
                        "content": text_content
                    });
                    if !reasoning_content.is_empty() || has_thinking_in_conv {
                        msg["reasoning_content"] = serde_json::Value::String(reasoning_content);
                    }
                    openai_messages.push(msg);
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
    // Check for API error response first
    if let Some(error) = json.get("error") {
        let msg = error["message"]
            .as_str()
            .or_else(|| error.as_str())
            .unwrap_or("unknown provider error");
        let error_type = error["type"].as_str().unwrap_or("");
        let code = error["code"].as_str().unwrap_or("");

        let full_msg = if !code.is_empty() {
            format!("{} ({}): {}", error_type, code, msg)
        } else if !error_type.is_empty() {
            format!("{}: {}", error_type, msg)
        } else {
            msg.to_string()
        };
        return Err(ClidoError::Provider(full_msg));
    }

    let id = json["id"].as_str().unwrap_or("").to_string();
    let model = json["model"].as_str().unwrap_or("").to_string();
    let choices = json["choices"].as_array().ok_or_else(|| {
        ClidoError::Provider("missing choices (provider may have returned an error)".into())
    })?;
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
            // Some OpenAI-compatible APIs return reasoning blocks in content
        }
    }
    // Some providers (e.g. Kimi) return reasoning as a separate field
    if let Some(reasoning) = message["reasoning_content"].as_str() {
        if !reasoning.is_empty() {
            blocks.push(ContentBlock::Thinking {
                thinking: reasoning.to_string(),
            });
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
        messages: &[Message],
        tools: &[ToolSchema],
        config: &AgentConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
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

        let max_tokens = config.max_output_tokens.unwrap_or(8192);
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "messages": openai_messages,
            "stream": true,
            "stream_options": {"include_usage": true}
        });
        body["messages"]
            .as_array_mut()
            .expect("messages field must be an array")
            .insert(
                0,
                serde_json::json!({ "role": "system", "content": system_content }),
            );
        if !openai_tools.is_empty() {
            body["tools"] = serde_json::Value::Array(openai_tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        let url = self.request_url();
        let mut req = self
            .client
            .post(&url)
            .header("content-type", "application/json");
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        for (k, v) in &self.extra_headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let response = req
            .json(&body)
            .send()
            .await
            .map_err(|e| ClidoError::Provider(format!("stream request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = crate::backoff::parse_retry_after_secs(response.headers());
            let text = response.text().await.unwrap_or_default();
            let preview: String = text.chars().take(300).collect();
            if status.as_u16() == 429 {
                let lower = preview.to_lowercase();
                let is_sub = crate::backoff::is_subscription_limit(retry_after, &lower);
                return Err(ClidoError::RateLimited {
                    message: format!("429 (model: {}): {}", self.model, preview),
                    retry_after_secs: retry_after,
                    is_subscription_limit: is_sub,
                });
            }
            return Err(ClidoError::Provider(format!(
                "API error {} (model: {}): {}",
                status, self.model, preview
            )));
        }

        let byte_stream = response.bytes_stream();
        Ok(Box::pin(parse_openai_sse(byte_stream)))
    }

    async fn list_models(&self) -> Vec<crate::provider::ModelEntry> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/models", base);
        let mut req = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key));
        for (k, v) in &self.extra_headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = match req.send().await {
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
        let is_openrouter = base.contains("openrouter.ai");
        let is_openai = base.contains("api.openai.com");
        let mut models: Vec<crate::provider::ModelEntry> = json["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| {
                let id = m["id"].as_str()?.to_string();
                if is_openai && !is_openai_chat_model(&id) {
                    return None;
                }
                // For OpenRouter: mark models that have no usable chat endpoints.
                // The API returns `supported_generation_types` (array) — if it
                // exists and does not contain "text", the model cannot generate
                // chat completions. Falls back to checking `per_request_limits`.
                let available = if is_openrouter {
                    openrouter_model_available(m)
                } else {
                    true
                };
                Some(crate::provider::ModelEntry { id, available })
            })
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models
    }
}

/// Parse OpenAI-compatible SSE byte stream into a stream of `StreamEvent`.
///
/// OpenAI SSE format: each chunk is `data: <json>\n\n`, terminated by `data: [DONE]\n\n`.
/// Each JSON chunk has `choices[0].delta` with optional `content` (text) and `tool_calls`.
fn parse_openai_sse(
    byte_stream: impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static,
) -> impl futures::Stream<Item = Result<StreamEvent>> + Send {
    use futures::channel::mpsc;

    /// Per-stream state for assembling OpenAI tool-call deltas.
    struct OpenAISseState {
        /// tool_call index → (id, name, partial_json)
        tool_calls: std::collections::HashMap<u64, (String, String, String)>,
    }

    fn process_line(
        line: &str,
        tx: &mut mpsc::UnboundedSender<Result<StreamEvent>>,
        state: &mut OpenAISseState,
    ) -> bool {
        let Some(data) = line.strip_prefix("data:") else {
            return true;
        };
        let data = data.trim();
        if data == "[DONE]" {
            for (id, _, _) in state.tool_calls.values() {
                let _ = tx.unbounded_send(Ok(StreamEvent::ToolUseEnd { id: id.clone() }));
            }
            return false;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
            return true;
        };

        // Usage chunk (stream_options.include_usage)
        if let Some(usage_obj) = json.get("usage").filter(|u| !u.is_null()) {
            let usage = Usage {
                input_tokens: usage_obj["prompt_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage_obj["completion_tokens"].as_u64().unwrap_or(0),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            };
            let stop_reason = match json["choices"]
                .as_array()
                .and_then(|c| c.first())
                .and_then(|c| c["finish_reason"].as_str())
            {
                Some("tool_calls") => StopReason::ToolUse,
                Some("length") => StopReason::MaxTokens,
                Some("stop_sequence") => StopReason::StopSequence,
                _ => StopReason::EndTurn,
            };
            let _ = tx.unbounded_send(Ok(StreamEvent::MessageDelta { stop_reason, usage }));
            return true;
        }

        let Some(choices) = json["choices"].as_array() else {
            return true;
        };
        let Some(choice) = choices.first() else {
            return true;
        };
        let delta = &choice["delta"];

        // Text content delta
        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                let _ = tx.unbounded_send(Ok(StreamEvent::TextDelta(text.to_string())));
            }
        }

        // Tool call deltas
        if let Some(tc_arr) = delta["tool_calls"].as_array() {
            for tc in tc_arr {
                let idx = tc["index"].as_u64().unwrap_or(0);
                let entry = state
                    .tool_calls
                    .entry(idx)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = tc["id"].as_str() {
                    if entry.0.is_empty() {
                        entry.0 = id.to_string();
                    }
                }
                if let Some(name) = tc["function"]["name"].as_str() {
                    if entry.1.is_empty() {
                        entry.1 = name.to_string();
                        let _ = tx.unbounded_send(Ok(StreamEvent::ToolUseStart {
                            id: entry.0.clone(),
                            name: name.to_string(),
                        }));
                    }
                }
                if let Some(partial) = tc["function"]["arguments"].as_str() {
                    entry.2.push_str(partial);
                    if !partial.is_empty() {
                        let _ = tx.unbounded_send(Ok(StreamEvent::ToolUseDelta {
                            id: entry.0.clone(),
                            partial_json: partial.to_string(),
                        }));
                    }
                }
            }
        }

        // Emit finish event
        if let Some(reason) = choice["finish_reason"].as_str() {
            if !reason.is_empty() && reason != "null" {
                for (id, _, _) in state.tool_calls.values() {
                    let _ = tx.unbounded_send(Ok(StreamEvent::ToolUseEnd { id: id.clone() }));
                }
                state.tool_calls.clear();
            }
        }
        true
    }

    let state = OpenAISseState {
        tool_calls: std::collections::HashMap::new(),
    };
    crate::sse::parse_sse_stream(byte_stream, state, process_line)
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

    // ── request_url ────────────────────────────────────────────────────────

    #[test]
    fn request_url_strips_trailing_slash() {
        let p = OpenAICompatProvider::new("key", "model", "https://example.com/v1/", vec![]);
        assert_eq!(p.request_url(), "https://example.com/v1/chat/completions");
    }

    #[test]
    fn request_url_no_trailing_slash() {
        let p = OpenAICompatProvider::new("key", "model", "https://example.com/v1", vec![]);
        assert_eq!(p.request_url(), "https://example.com/v1/chat/completions");
    }

    // ── new_openrouter ─────────────────────────────────────────────────────

    #[test]
    fn new_openrouter_uses_openrouter_url() {
        let p = OpenAICompatProvider::new_openrouter("sk-or-key", "model");
        assert!(p.request_url().contains("openrouter.ai"));
    }

    // ── messages_to_openai system messages ────────────────────────────────

    #[test]
    fn messages_to_openai_system_message_extracted() {
        let messages = vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "You are a bot.".to_string(),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            },
        ];
        let (sys, msgs) = messages_to_openai(&messages).unwrap();
        assert_eq!(sys, Some("You are a bot.".to_string()));
        assert_eq!(msgs.len(), 1); // system not in messages list
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn messages_to_openai_multiple_system_messages_joined() {
        let messages = vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "part1".to_string(),
                }],
            },
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "part2".to_string(),
                }],
            },
        ];
        let (sys, msgs) = messages_to_openai(&messages).unwrap();
        assert_eq!(sys, Some("part1\npart2".to_string()));
        assert!(msgs.is_empty());
    }

    // ── messages_to_openai assistant with text only ────────────────────────

    #[test]
    fn messages_to_openai_assistant_text_only() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "answer".to_string(),
            }],
        }];
        let (_, msgs) = messages_to_openai(&messages).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["content"], "answer");
        assert!(!msgs[0]["tool_calls"].is_array());
    }

    // ── messages_to_openai assistant with both text and tool calls ─────────

    #[test]
    fn messages_to_openai_assistant_with_text_and_tool_calls() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "thinking...".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "call_x".to_string(),
                    name: "Write".to_string(),
                    input: serde_json::json!({"file_path": "f.txt", "content": "hi"}),
                },
            ],
        }];
        let (_, msgs) = messages_to_openai(&messages).unwrap();
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["content"], "thinking...");
        assert!(msgs[0]["tool_calls"].is_array());
    }

    #[test]
    fn messages_to_openai_assistant_reasoning_content_preserved() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: "let me think...".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"path": "/tmp/x"}),
                },
            ],
        }];
        let (_, msgs) = messages_to_openai(&messages).unwrap();
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["reasoning_content"], "let me think...");
        assert!(msgs[0]["tool_calls"].is_array());
    }

    #[test]
    fn messages_to_openai_assistant_no_reasoning_no_field() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        }];
        let (_, msgs) = messages_to_openai(&messages).unwrap();
        assert!(msgs[0]["reasoning_content"].is_null());
    }

    // ── parse_openai_response edge cases ──────────────────────────────────

    #[test]
    fn parse_openai_response_length_stop() {
        let json = serde_json::json!({
            "id": "x",
            "model": "gpt-4",
            "choices": [{
                "message": {"role": "assistant", "content": "partial"},
                "finish_reason": "length"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        });
        let r = parse_openai_response(&json).unwrap();
        assert!(matches!(r.stop_reason, StopReason::MaxTokens));
    }

    #[test]
    fn parse_openai_response_empty_choices_error() {
        let json = serde_json::json!({
            "id": "x",
            "model": "gpt-4",
            "choices": [],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        });
        let result = parse_openai_response(&json);
        assert!(result.is_err());
    }

    #[test]
    fn parse_openai_response_missing_choices_error() {
        let json = serde_json::json!({
            "id": "x",
            "model": "gpt-4",
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        });
        let result = parse_openai_response(&json);
        assert!(result.is_err());
    }

    // ── message_to_content_blocks array content ────────────────────────────

    #[test]
    fn message_to_content_blocks_array_content() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "text", "text": "world"}
            ]
        });
        let blocks = message_to_content_blocks(&message).unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "hello"));
        assert!(matches!(&blocks[1], ContentBlock::Text { text } if text == "world"));
    }

    #[test]
    fn message_to_content_blocks_empty_text_not_added() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": ""
        });
        let blocks = message_to_content_blocks(&message).unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn message_to_content_blocks_tool_calls_parsed() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "Bash",
                        "arguments": "{\"command\":\"echo hi\"}"
                    }
                }
            ]
        });
        let blocks = message_to_content_blocks(&message).unwrap();
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::ToolUse { id, name, input } = &blocks[0] {
            assert_eq!(id, "call_1");
            assert_eq!(name, "Bash");
            assert_eq!(input["command"], "echo hi");
        } else {
            panic!("expected ToolUse");
        }
    }
}
