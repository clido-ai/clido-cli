//! `clido list-models` / `clido refresh-models`: fetch and display available models.

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
        match make_provider(profile_name, profile, Some(effective_provider), None) {
            Ok(_) => {
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

    let provider = match clido_providers::build_provider(
        effective_provider,
        api_key.clone(),
        "placeholder".to_string(),
        base_url,
    ) {
        Ok(p) => p,
        Err(e) => {
            if json {
                println!("[]");
            } else {
                eprintln!("Cannot create provider: {}", e);
            }
            return Ok(());
        }
    };

    let models = provider.list_models_metadata().await.unwrap_or_else(|e| {
        if !json {
            eprintln!("Error fetching models: {}", e);
        }
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
            let name = m.name.as_deref().unwrap_or(&m.id);
            let mut line = name.to_string();
            if m.name.is_some() {
                line.push_str(&format!(" ({})", m.id));
            }
            if let Some(ctx) = m.context_window {
                line.push_str(&format!("  {}K ctx", ctx / 1000));
            }
            if let Some(ref p) = m.pricing {
                line.push_str(&format!("  ${:.2}/${:.2}", p.input_per_mtok, p.output_per_mtok));
            }
            if !m.available {
                line.push_str("  [unavailable]");
            }
            println!("{}", line);
        }
    }
    Ok(())
}

/// Refresh the local models cache from models.dev.
pub async fn run_refresh_models() -> anyhow::Result<()> {
    let config_dir = clido_core::global_config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".clido"));
    let fetcher = clido_providers::ModelFetcher::new(&config_dir);
    eprintln!("Fetching latest models from models.dev...");
    fetcher.refresh().await;
    let snapshot = fetcher.load().await;
    let provider_count = snapshot.providers.len();
    let model_count: usize = snapshot.providers.values().map(|p| p.models.len()).sum();
    println!("Cached {} models across {} providers.", model_count, provider_count);
    Ok(())
}
