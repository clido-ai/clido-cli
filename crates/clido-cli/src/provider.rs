//! Provider construction and permission prompting.

use async_trait::async_trait;
use clido_agent::{AskUser, PermGrant, PermRequest};
use clido_core::ProfileEntry;
use clido_providers::{build_provider, ModelProvider};
use std::env;
use std::io::{self, Write};
use std::sync::Arc;

use crate::ui::{ansi, cli_use_color};

/// Ask the user on stderr/stdin for permission to run a state-changing tool.
pub struct StdinAskUser;

#[async_trait]
impl AskUser for StdinAskUser {
    async fn ask(&self, req: PermRequest) -> PermGrant {
        let tool_name = req.tool_name.clone();
        let description = req.description.clone();
        let prompt = format!("Allow {} with input {}? [y/N] ", tool_name, description);
        let result = tokio::task::spawn_blocking(move || {
            if cli_use_color() {
                eprint!("{}{}{}", ansi::DIM, prompt, ansi::RESET);
            } else {
                eprint!("{}", prompt);
            }
            let _ = io::stderr().flush();
            let mut line = String::new();
            if io::stdin().read_line(&mut line).is_ok() {
                let t = line.trim();
                t.eq_ignore_ascii_case("y") || t.eq_ignore_ascii_case("yes")
            } else {
                false
            }
        })
        .await;
        if result.unwrap_or(false) {
            PermGrant::Allow
        } else {
            PermGrant::Deny
        }
    }
}

/// Return the conventional API key env var for a given provider name.
pub fn default_api_key_env(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "minimax" => "MINIMAX_API_KEY",
        "alibabacloud" => "DASHSCOPE_API_KEY",
        _ => "",
    }
}

/// Build a provider from profile, resolving the API key from the environment.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_api_key_env_openrouter() {
        assert_eq!(default_api_key_env("openrouter"), "OPENROUTER_API_KEY");
    }

    #[test]
    fn default_api_key_env_alibabacloud() {
        assert_eq!(default_api_key_env("alibabacloud"), "DASHSCOPE_API_KEY");
    }

    #[test]
    fn default_api_key_env_anthropic() {
        assert_eq!(default_api_key_env("anthropic"), "ANTHROPIC_API_KEY");
    }

    #[test]
    fn default_api_key_env_openai() {
        assert_eq!(default_api_key_env("openai"), "OPENAI_API_KEY");
    }

    #[test]
    fn default_api_key_env_mistral() {
        assert_eq!(default_api_key_env("mistral"), "MISTRAL_API_KEY");
    }

    #[test]
    fn default_api_key_env_minimax() {
        assert_eq!(default_api_key_env("minimax"), "MINIMAX_API_KEY");
    }

    #[test]
    fn default_api_key_env_unknown_returns_empty() {
        assert_eq!(default_api_key_env("local"), "");
        assert_eq!(default_api_key_env("unknown"), "");
        assert_eq!(default_api_key_env(""), "");
    }

    // ── make_provider ──────────────────────────────────────────────────────

    #[test]
    fn make_provider_with_api_key_in_profile() {
        let profile = ProfileEntry {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            api_key: Some("sk-ant-test".to_string()),
            api_key_env: None,
            base_url: None,
            worker: None,
            reviewer: None,
        };
        let result = make_provider("default", &profile, None, None);
        assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
    }

    #[test]
    fn make_provider_local_no_key_needed() {
        let profile = ProfileEntry {
            provider: "local".to_string(),
            model: "llama3.2".to_string(),
            api_key: None,
            api_key_env: None,
            base_url: Some("http://localhost:11434".to_string()),
            worker: None,
            reviewer: None,
        };
        let result = make_provider("default", &profile, None, None);
        assert!(result.is_ok(), "local provider should not need API key");
    }

    #[test]
    fn make_provider_missing_key_returns_error() {
        // Use a provider that requires a key, with no key in profile or env
        // Set a unique env var name that won't be set
        let profile = ProfileEntry {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            api_key: None,
            api_key_env: Some("CLIDO_TEST_NONEXISTENT_KEY_XYZ".to_string()),
            base_url: None,
            worker: None,
            reviewer: None,
        };
        // Ensure env var is not set
        env::remove_var("CLIDO_TEST_NONEXISTENT_KEY_XYZ");
        let result = make_provider("default", &profile, None, None);
        assert!(result.is_err(), "should fail when key env var not set");
        let msg = result.err().unwrap();
        assert!(msg.contains("No API key") || msg.contains("default"));
    }

    #[test]
    fn make_provider_with_model_override() {
        let profile = ProfileEntry {
            provider: "openrouter".to_string(),
            model: "anthropic/claude-3-5-sonnet".to_string(),
            api_key: Some("sk-or-test".to_string()),
            api_key_env: None,
            base_url: None,
            worker: None,
            reviewer: None,
        };
        let result = make_provider("default", &profile, None, Some("gpt-4o"));
        assert!(result.is_ok());
    }

    #[test]
    fn make_provider_with_provider_override() {
        let profile = ProfileEntry {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            api_key: Some("key".to_string()),
            api_key_env: None,
            base_url: None,
            worker: None,
            reviewer: None,
        };
        // Override to local (no key needed)
        let result = make_provider("default", &profile, Some("local"), None);
        assert!(result.is_ok());
    }

    #[test]
    fn make_provider_minimax() {
        let profile = ProfileEntry {
            provider: "minimax".to_string(),
            model: "MiniMax-M2.7".to_string(),
            api_key: Some("sk-minimax-test".to_string()),
            api_key_env: None,
            base_url: None,
            worker: None,
            reviewer: None,
        };
        let result = make_provider("default", &profile, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn make_provider_alibabacloud() {
        let profile = ProfileEntry {
            provider: "alibabacloud".to_string(),
            model: "qwen-plus".to_string(),
            api_key: Some("sk-alibaba-test".to_string()),
            api_key_env: None,
            base_url: None,
            worker: None,
            reviewer: None,
        };
        let result = make_provider("default", &profile, None, None);
        assert!(result.is_ok());
    }
}

pub fn make_provider(
    profile_name: &str,
    profile: &ProfileEntry,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<Arc<dyn ModelProvider>, String> {
    let provider_name = provider_override.unwrap_or(profile.provider.as_str());
    let api_key = if provider_name == "local" {
        // Local/Ollama doesn't require an API key.
        profile.api_key.clone().unwrap_or_default()
    } else if let Some(key) = &profile.api_key {
        key.clone()
    } else {
        let api_key_env = profile
            .api_key_env
            .as_deref()
            .unwrap_or_else(|| default_api_key_env(provider_name));
        env::var(api_key_env).map_err(|_| {
            format!(
                "No API key configured for profile '{}'. Run 'clido init' to set up your provider, or 'clido doctor' to diagnose.",
                profile_name
            )
        })?
    };
    let model = model_override.unwrap_or(&profile.model).to_string();
    build_provider(provider_name, api_key, model, profile.base_url.as_deref())
        .map_err(|e| e.to_string())
}
