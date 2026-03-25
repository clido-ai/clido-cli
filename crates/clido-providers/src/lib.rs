//! Model providers (Anthropic, OpenRouter, etc.).

use std::sync::Arc;

pub mod anthropic;
pub mod openai;
pub mod provider;

pub use anthropic::AnthropicProvider;
pub use openai::OpenAICompatProvider;
pub use provider::{ModelEntry, ModelProvider, StreamEvent};

use clido_core::{ClidoError, Result};

/// Build a provider from profile name, API key, model, and optional base URL.
/// Used by the CLI after resolving profile and reading API key from env.
pub fn build_provider(
    provider_name: &str,
    api_key: String,
    model: String,
    base_url: Option<&str>,
) -> Result<Arc<dyn ModelProvider>> {
    match provider_name {
        "anthropic" => Ok(Arc::new(AnthropicProvider::new(api_key, model))),
        "openrouter" => Ok(Arc::new(OpenAICompatProvider::new_openrouter(
            api_key, model,
        ))),
        "openai" => Ok(Arc::new(OpenAICompatProvider::new_openai(api_key, model))),
        "mistral" => Ok(Arc::new(OpenAICompatProvider::new_mistral(api_key, model))),
        "minimax" => Ok(Arc::new(OpenAICompatProvider::new_minimax(api_key, model))),
        "local" => {
            let url = base_url.unwrap_or("http://localhost:11434").to_string();
            Ok(Arc::new(OpenAICompatProvider::new(
                api_key,
                model,
                url,
                Vec::new(),
            )))
        }
        "alibabacloud" => {
            let url = base_url
                .unwrap_or("https://dashscope.aliyuncs.com/compatible-mode/v1")
                .to_string();
            Ok(Arc::new(OpenAICompatProvider::new(
                api_key,
                model,
                url,
                Vec::new(),
            )))
        }
        p => Err(ClidoError::Config(format!(
            "Provider '{}' is not supported. Available: anthropic, openrouter, openai, mistral, minimax, local, alibabacloud.",
            p
        ))),
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
) -> Vec<ModelEntry> {
    let Ok(provider) = build_provider(
        provider_name,
        api_key.to_string(),
        "placeholder".to_string(),
        base_url,
    ) else {
        return vec![];
    };
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
}
