//! Provider construction and permission prompting.

use async_trait::async_trait;
use clido_agent::AskUser;
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
    async fn ask(&self, tool_name: &str, input: &serde_json::Value) -> bool {
        let prompt = format!(
            "Allow {} with input {}? [y/N] ",
            tool_name,
            serde_json::to_string(input).unwrap_or_else(|_| "?".into())
        );
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
        result.unwrap_or(false)
    }
}

/// Return the conventional API key env var for a given provider name.
pub fn default_api_key_env(provider: &str) -> &'static str {
    match provider {
        "openrouter" => "OPENROUTER_API_KEY",
        _ => "ANTHROPIC_API_KEY",
    }
}

/// Build a provider from profile, resolving the API key from the environment.
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
                "API key not found for profile '{}'. Set {} in your environment. Run: clido doctor to check all configuration.",
                profile_name, api_key_env
            )
        })?
    };
    let model = model_override.unwrap_or(&profile.model).to_string();
    build_provider(provider_name, api_key, model, profile.base_url.as_deref())
        .map_err(|e| e.to_string())
}
