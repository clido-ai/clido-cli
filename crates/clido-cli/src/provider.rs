//! Provider construction and permission prompting.

use async_trait::async_trait;
use clido_agent::{AskUser, PermGrant, PermRequest};
use clido_core::ProfileEntry;
use clido_providers::{build_provider, ModelProvider, PROVIDER_REGISTRY};
use std::collections::HashMap;
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
        let diff = req.diff.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Show the unified diff before the permission prompt in diff-review mode.
            if let Some(d) = &diff {
                if !d.is_empty() {
                    if cli_use_color() {
                        eprintln!("{}{}{}", ansi::DIM, d, ansi::RESET);
                    } else {
                        eprintln!("{}", d);
                    }
                }
            }
            let prompt = format!("Allow {} with input {}? [y/N/a] ", tool_name, description);
            if cli_use_color() {
                eprint!("{}{}{}", ansi::DIM, prompt, ansi::RESET);
            } else {
                eprint!("{}", prompt);
            }
            let _ = io::stderr().flush();
            let mut line = String::new();
            if io::stdin().read_line(&mut line).is_ok() {
                match line.trim().to_lowercase().as_str() {
                    "y" | "yes" => PermGrant::Allow,
                    "a" | "always" => PermGrant::AllowAll,
                    _ => PermGrant::Deny,
                }
            } else {
                PermGrant::Deny
            }
        })
        .await;
        result.unwrap_or(PermGrant::Deny)
    }
}

/// Return the conventional API key env var for a given provider name.
pub fn default_api_key_env(provider: &str) -> &'static str {
    PROVIDER_REGISTRY
        .iter()
        .find(|d| d.id == provider)
        .map(|d| d.api_key_env)
        .unwrap_or("")
}

/// Derive the clido config directory from `CLIDO_CONFIG` env var or the
/// platform default. Returns `None` if the directory cannot be determined.
fn default_config_dir() -> Option<std::path::PathBuf> {
    if let Ok(p_str) = env::var("CLIDO_CONFIG") {
        std::path::Path::new(&p_str)
            .parent()
            .map(|p| p.to_path_buf())
    } else {
        directories::ProjectDirs::from("", "", "clido").map(|d| d.config_dir().to_path_buf())
    }
}

/// Load API keys from `<config_dir>/credentials` (TOML `[keys]` section).
/// Returns an empty map if the file does not exist or cannot be parsed.
pub fn load_credentials(config_dir: &std::path::Path) -> HashMap<String, String> {
    let creds_path = config_dir.join("credentials");
    if !creds_path.exists() {
        return HashMap::new();
    }
    let content = std::fs::read_to_string(&creds_path).unwrap_or_default();
    let table: toml::Value =
        toml::from_str(&content).unwrap_or(toml::Value::Table(Default::default()));
    let mut map = HashMap::new();
    if let Some(keys) = table.get("keys").and_then(|v| v.as_table()) {
        for (k, v) in keys {
            if let Some(s) = v.as_str() {
                map.insert(k.clone(), s.to_string());
            }
        }
    }
    map
}

/// Build a provider from profile, resolving the API key from the environment.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_api_key_env_kimi() {
        assert_eq!(default_api_key_env("kimi"), "MOONSHOT_API_KEY");
    }

    #[test]
    fn default_api_key_env_kimi_code() {
        assert_eq!(default_api_key_env("kimi-code"), "KIMI_CODE_API_KEY");
    }

    #[test]
    fn make_provider_kimi() {
        let profile = ProfileEntry {
            provider: "kimi".to_string(),
            model: "moonshot-v1-32k".to_string(),
            api_key: Some("sk-kimi-test".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        assert!(make_provider("default", &profile, None, None).is_ok());
    }

    #[test]
    fn make_provider_kimi_code() {
        let profile = ProfileEntry {
            provider: "kimi-code".to_string(),
            model: "kimi-for-coding".to_string(),
            api_key: Some("sk-kimi-code-test".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        assert!(make_provider("default", &profile, None, None).is_ok());
    }

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
            user_agent: None,
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
            user_agent: None,
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
            user_agent: None,
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
            user_agent: None,
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
            user_agent: None,
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
            user_agent: None,
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
            user_agent: None,
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

    let is_local = PROVIDER_REGISTRY
        .iter()
        .find(|d| d.id == provider_name)
        .map(|d| d.is_local)
        .unwrap_or(provider_name == "local");

    let api_key = if is_local {
        // Local/Ollama doesn't require an API key.
        profile.api_key.clone().unwrap_or_default()
    } else {
        // Resolution order:
        // 1. Env var (explicit api_key_env or provider's conventional var)
        // 2. Credentials file (~/.config/clido/credentials)
        // 3. Literal api_key in config.toml (backward compat)
        let env_var = profile
            .api_key_env
            .as_deref()
            .unwrap_or_else(|| default_api_key_env(provider_name));
        let from_env = if !env_var.is_empty() {
            env::var(env_var).ok().filter(|v| !v.is_empty())
        } else {
            None
        };

        if let Some(key) = from_env {
            key
        } else {
            let from_creds = default_config_dir()
                .map(|dir| load_credentials(&dir))
                .and_then(|creds| creds.get(provider_name).cloned())
                .filter(|v| !v.is_empty());

            if let Some(key) = from_creds {
                key
            } else if let Some(key) = &profile.api_key {
                key.clone()
            } else {
                return Err(format!(
                    "No API key configured for profile '{}'. Run 'clido init' to set up your provider, or 'clido doctor' to diagnose.",
                    profile_name
                ));
            }
        }
    };
    let model = model_override.unwrap_or(&profile.model).to_string();
    build_provider(provider_name, api_key, model, profile.base_url.as_deref())
        .map_err(|e| e.to_string())
}
