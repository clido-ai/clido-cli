//! Model providers (Anthropic, OpenRouter, etc.).

use std::sync::Arc;

pub mod anthropic;
pub mod openai;
pub mod provider;

pub use anthropic::AnthropicProvider;
pub use openai::OpenAICompatProvider;
pub use provider::{ModelProvider, StreamEvent};

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
            "Provider '{}' is not yet supported. Available: anthropic, openrouter, local, alibabacloud.",
            p
        ))),
    }
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
        assert!(err.contains("not yet supported"));
        assert!(err.contains("anthropic"));
        assert!(err.contains("openrouter"));
    }
}
