//! `clido list-models` / `clido fetch-models`: fetch available models from the configured provider.

use crate::provider::{default_api_key_env, make_provider};
use clido_core::load_config;
use std::env;

pub async fn run_list_models(provider_filter: Option<&str>, json: bool) -> anyhow::Result<()> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let Ok(loaded) = load_config(&workspace_root) else {
        if json {
            println!("[]");
        } else {
            eprintln!("No configuration found. Run 'clido init' to set up a provider.");
        }
        return Ok(());
    };

    let profile_name = loaded.default_profile.as_str();
    let Ok(profile) = loaded.get_profile(profile_name) else {
        if json {
            println!("[]");
        } else {
            eprintln!("Profile '{}' not found in config.", profile_name);
        }
        return Ok(());
    };

    let effective_provider = provider_filter.unwrap_or(profile.provider.as_str());

    // Resolve API key for the effective provider.
    let api_key = if profile.provider == effective_provider {
        // Use the configured profile's key resolution.
        match make_provider(profile_name, profile, Some(effective_provider), None) {
            Ok(_) => {
                // Key resolved — extract it properly.
                if let Some(k) = &profile.api_key {
                    k.clone()
                } else {
                    let env_var = profile
                        .api_key_env
                        .as_deref()
                        .unwrap_or_else(|| default_api_key_env(effective_provider));
                    env::var(env_var).unwrap_or_default()
                }
            }
            Err(e) => {
                if json {
                    println!("[]");
                } else {
                    eprintln!("Cannot list models: {}", e);
                }
                return Ok(());
            }
        }
    } else {
        // Provider filter differs from config — try env var for that provider.
        let env_var = default_api_key_env(effective_provider);
        if env_var.is_empty() {
            String::new()
        } else {
            env::var(env_var).unwrap_or_default()
        }
    };

    let base_url = if effective_provider == "local" {
        profile.base_url.as_deref()
    } else {
        None
    };

    let models = clido_providers::fetch_provider_models(effective_provider, &api_key, base_url)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Error fetching models: {}", e);
            vec![]
        });

    if json {
        let arr: serde_json::Value =
            serde_json::Value::Array(models.iter().map(|m| serde_json::json!(m)).collect());
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else if models.is_empty() {
        eprintln!(
            "No models returned for provider '{}'. Check your API key or network connection.",
            effective_provider
        );
    } else {
        for m in &models {
            println!("{}", m);
        }
    }
    Ok(())
}
