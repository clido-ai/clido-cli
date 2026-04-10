//! Model providers (Anthropic, OpenRouter, etc.).

use std::sync::Arc;

pub mod anthropic;
pub mod backoff;
pub mod fallback;
pub mod http_client;
pub mod openai;
pub mod provider;
pub mod rate_limit;
pub mod registry;
pub mod retry;
pub(crate) mod sse;

pub use anthropic::AnthropicProvider;
pub use fallback::FallbackProvider;
pub use openai::OpenAICompatProvider;
pub use provider::{ModelEntry, ModelProvider, StreamEvent};
pub use rate_limit::{
    RateLimitConfig, RateLimitError, RateLimiter, RateLimitPermit, RateLimiterRegistry,
    RateLimiterStats,
};
pub use registry::{is_subscription_provider, ProviderDef, PROVIDER_REGISTRY};
pub use retry::RetryProvider;

use clido_core::{ClidoError, Result};

/// Resolve common model aliases to canonical model IDs.
/// Returns the original string unchanged if no alias matches.
pub fn resolve_model_alias(model: &str) -> &str {
    match model {
        "sonnet" => "claude-sonnet-4-5",
        "opus" => "claude-opus-4-6",
        "haiku" => "claude-haiku-4-5",
        "4o" => "gpt-4o",
        "4o-mini" => "gpt-4o-mini",
        "flash" => "gemini-2.5-flash",
        "deepseek" => "deepseek-chat",
        "r1" => "deepseek-reasoner",
        "grok" => "grok-3-beta",
        "sonar" => "sonar-pro",
        other => other,
    }
}

/// Build a provider from profile name, API key, model, and optional base URL.
/// Used by the CLI after resolving profile and reading API key from env.
pub fn build_provider(
    provider_name: &str,
    api_key: String,
    model: String,
    base_url: Option<&str>,
) -> Result<Arc<dyn ModelProvider>> {
    build_provider_with_ua(provider_name, api_key, model, base_url, None)
}

/// Like [`build_provider`] but allows overriding the HTTP `User-Agent` header.
///
/// Resolution order (first non-empty wins):
///   1. `user_agent` argument — explicit override from profile/slot config
///   2. `CLIDO_USER_AGENT` environment variable — process-wide override (e.g. set in test scripts)
///   3. `"clido/<version>"` — default
pub fn build_provider_with_ua(
    provider_name: &str,
    api_key: String,
    model: String,
    base_url: Option<&str>,
    user_agent: Option<String>,
) -> Result<Arc<dyn ModelProvider>> {
    let model = resolve_model_alias(&model).to_string();
    let ua = user_agent
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("CLIDO_USER_AGENT")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| {
            // Provider-specific default user agents
            if provider_name == "kimi-code" {
                "RooCode/3.0.0".to_string()
            } else {
                format!("clido/{}", env!("CARGO_PKG_VERSION"))
            }
        });

    if let Some(def) = PROVIDER_REGISTRY.iter().find(|d| d.id == provider_name) {
        if def.is_anthropic {
            return Ok(Arc::new(AnthropicProvider::new_with_user_agent(
                api_key, model, &ua,
            )));
        }
        let url = if def.is_local || def.id == "alibabacloud" {
            base_url.unwrap_or(def.base_url).to_string()
        } else {
            def.base_url.to_string()
        };
        let headers: Vec<(String, String)> = def
            .extra_headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Ok(Arc::new(OpenAICompatProvider::new_with_user_agent(
            api_key, model, url, headers, &ua,
        )))
    } else {
        let valid: Vec<&str> = PROVIDER_REGISTRY.iter().map(|d| d.id).collect();
        Err(ClidoError::Config(format!(
            "Provider '{}' is not supported. Valid: {}.",
            provider_name,
            valid.join(", ")
        )))
    }
}

/// Fetch models from a provider's API using the given credentials.
/// Returns an empty vec on any error (bad key, network failure, unsupported provider).
/// Used during setup to populate the model selection list dynamically.
/// Entries with `available = false` have no usable chat endpoints (shown greyed-out).
pub async fn fetch_provider_models(
    provider_name: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> std::result::Result<Vec<ModelEntry>, String> {
    let provider = build_provider(
        provider_name,
        api_key.to_string(),
        "placeholder".to_string(),
        base_url,
    )
    .map_err(|e| e.to_string())?;
    provider.list_models().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_provider_openrouter_returns_ok() {
        let p = build_provider(
            "openrouter",
            "sk-fake".to_string(),
            "anthropic/claude-3-haiku".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_unsupported_returns_config_error() {
        let res = build_provider("unsupported", "key".to_string(), "model".to_string(), None);
        let err = match &res {
            Ok(_) => panic!("expected Err"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("not supported"));
        assert!(err.contains("anthropic"));
        assert!(err.contains("openrouter"));
    }

    #[test]
    fn build_provider_anthropic_returns_ok() {
        let p = build_provider(
            "anthropic",
            "sk-ant-fake".to_string(),
            "claude-sonnet-4-5".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_local_with_default_url() {
        let p = build_provider("local", "".to_string(), "llama3.2".to_string(), None).unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_local_with_custom_url() {
        let p = build_provider(
            "local",
            "".to_string(),
            "mistral".to_string(),
            Some("http://127.0.0.1:8080"),
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_minimax_returns_ok() {
        let p = build_provider(
            "minimax",
            "sk-minimax-fake".to_string(),
            "MiniMax-M2.7".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_kimi_returns_ok() {
        let p = build_provider(
            "kimi",
            "sk-kimi-fake".to_string(),
            "moonshot-v1-32k".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_kimi_code_returns_ok() {
        let p = build_provider(
            "kimi-code",
            "sk-kimi-fake".to_string(),
            "kimi-for-coding".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_alibabacloud_default_url() {
        let p = build_provider(
            "alibabacloud",
            "sk-fake".to_string(),
            "qwen-plus".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_alibabacloud_custom_url() {
        let p = build_provider(
            "alibabacloud",
            "sk-fake".to_string(),
            "qwen-turbo".to_string(),
            Some("https://custom.dashscope.com/v1"),
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_deepseek_returns_ok() {
        let p = build_provider(
            "deepseek",
            "sk-ds-fake".to_string(),
            "deepseek-chat".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_groq_returns_ok() {
        let p = build_provider(
            "groq",
            "gsk_fake".to_string(),
            "llama-3.3-70b-versatile".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_cerebras_returns_ok() {
        let p = build_provider(
            "cerebras",
            "csk-fake".to_string(),
            "llama-3.3-70b".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_togetherai_returns_ok() {
        let p = build_provider(
            "togetherai",
            "tok-fake".to_string(),
            "meta-llama/Llama-3-70b-chat-hf".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_fireworks_returns_ok() {
        let p = build_provider(
            "fireworks",
            "fw-fake".to_string(),
            "accounts/fireworks/models/llama-v3p1-70b-instruct".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_xai_returns_ok() {
        let p = build_provider(
            "xai",
            "xai-fake".to_string(),
            "grok-3-beta".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_perplexity_returns_ok() {
        let p = build_provider(
            "perplexity",
            "pplx-fake".to_string(),
            "sonar-pro".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn build_provider_gemini_returns_ok() {
        let p = build_provider(
            "gemini",
            "AIzaSy-fake".to_string(),
            "gemini-2.5-pro-exp-03-25".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }

    #[test]
    fn resolve_model_alias_known() {
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-5");
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5");
        assert_eq!(resolve_model_alias("4o"), "gpt-4o");
        assert_eq!(resolve_model_alias("4o-mini"), "gpt-4o-mini");
        assert_eq!(resolve_model_alias("flash"), "gemini-2.5-flash");
        assert_eq!(resolve_model_alias("deepseek"), "deepseek-chat");
        assert_eq!(resolve_model_alias("r1"), "deepseek-reasoner");
        assert_eq!(resolve_model_alias("grok"), "grok-3-beta");
        assert_eq!(resolve_model_alias("sonar"), "sonar-pro");
    }

    #[test]
    fn resolve_model_alias_passthrough() {
        assert_eq!(
            resolve_model_alias("gpt-4o-2024-11-20"),
            "gpt-4o-2024-11-20"
        );
        assert_eq!(resolve_model_alias("claude-opus-4-6"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias(""), "");
    }

    #[test]
    fn build_provider_alias_is_resolved() {
        // "sonnet" alias should produce a working provider (not error)
        let p = build_provider(
            "anthropic",
            "sk-ant-fake".to_string(),
            "sonnet".to_string(),
            None,
        )
        .unwrap();
        assert!(Arc::strong_count(&p) >= 1);
    }
}
