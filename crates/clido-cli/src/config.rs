//! `clido config show` and `clido config set <key> <value>`.

use clido_core::load_config;
use std::env;
use std::path::PathBuf;

use crate::cli::ConfigCmd;
use crate::errors::CliError;
use crate::setup::anonymize_key;
use crate::ui::{ansi, cli_use_color};

pub async fn run_config(cmd: &ConfigCmd) -> Result<(), anyhow::Error> {
    match cmd {
        ConfigCmd::Show => show_config(),
        ConfigCmd::Set { key, value } => set_config(key, value),
    }
}

fn show_config() -> Result<(), anyhow::Error> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let loaded = load_config(&cwd).map_err(|e| CliError::Config(e.to_string()))?;
    let profile_name = loaded.default_profile.as_str();
    let profile = loaded
        .get_profile(profile_name)
        .map_err(|e| CliError::Config(e.to_string()))?;
    let use_color = cli_use_color();

    let key_display = if let Some(k) = &profile.api_key {
        anonymize_key(k)
    } else if let Some(env_name) = &profile.api_key_env {
        format!("${} (env)", env_name)
    } else {
        "not set".to_string()
    };

    if use_color {
        println!("{}profile{} {}", ansi::BOLD, ansi::RESET, profile_name);
        println!(
            "  {}provider{}   {}",
            ansi::DIM,
            ansi::RESET,
            profile.provider
        );
        println!("  {}model{}      {}", ansi::DIM, ansi::RESET, profile.model);
        println!("  {}api-key{}    {}", ansi::DIM, ansi::RESET, key_display);
        if let Some(url) = &profile.base_url {
            println!("  {}base-url{}   {}", ansi::DIM, ansi::RESET, url);
        }
    } else {
        println!("profile {}", profile_name);
        println!("  provider   {}", profile.provider);
        println!("  model      {}", profile.model);
        println!("  api-key    {}", key_display);
        if let Some(url) = &profile.base_url {
            println!("  base-url   {}", url);
        }
    }
    Ok(())
}

fn set_config(key: &str, value: &str) -> Result<(), anyhow::Error> {
    let config_path = config_path()?;
    if !config_path.exists() {
        return Err(
            CliError::Config("No config file found. Run 'clido init' first.".into()).into(),
        );
    }

    let text = std::fs::read_to_string(&config_path)
        .map_err(|e| CliError::Config(format!("Cannot read config: {}", e)))?;

    let new_text = match key {
        "model" => replace_or_add(&text, "model", value),
        "provider" => replace_or_add(&text, "provider", value),
        "api-key" => {
            // Remove api_key_env if present, set api_key.
            let without_env = remove_line(&text, "api_key_env");
            let without_old_key = remove_line(&without_env, "api_key");
            add_after_profile_header(
                &without_old_key,
                "# api_key is stored in plain text — keep this file private (chmod 600).",
                &format!("api_key = \"{}\"", value),
            )
        }
        _ => {
            return Err(CliError::Usage(format!(
                "Unknown config key '{}'. Valid keys: model, provider, api-key.",
                key
            ))
            .into())
        }
    };

    std::fs::write(&config_path, &new_text)
        .map_err(|e| CliError::Config(format!("Cannot write config: {}", e)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
    }

    let use_color = cli_use_color();
    let display = if key == "api-key" {
        anonymize_key(value)
    } else {
        value.to_string()
    };
    if use_color {
        println!("{}✓{} {} = {}", ansi::GREEN, ansi::RESET, key, display);
    } else {
        println!("✓ {} = {}", key, display);
    }
    Ok(())
}

/// Replace `key = "..."` line in the [profile.default] section, or append it before the next section.
fn replace_or_add(text: &str, key: &str, value: &str) -> String {
    let needle = format!("{} = ", key);
    if text.lines().any(|l| l.trim_start().starts_with(&needle)) {
        text.lines()
            .map(|l| {
                if l.trim_start().starts_with(&needle) {
                    format!("{} = \"{}\"", key, value)
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + if text.ends_with('\n') { "\n" } else { "" }
    } else {
        add_after_profile_header(text, "", &format!("{} = \"{}\"", key, value))
    }
}

/// Remove all lines whose trimmed form starts with `key`.
fn remove_line(text: &str, key: &str) -> String {
    let needle = format!("{} ", key);
    let needle_eq = format!("{}=", key);
    let mut out: Vec<&str> = text
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with(&needle) && !t.starts_with(&needle_eq)
        })
        .collect();
    // Remove trailing blank comment lines left over.
    while out.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        out.pop();
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Insert `comment` (if non-empty) and `line` right after the `[profile.default]` header.
fn add_after_profile_header(text: &str, comment: &str, line: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let insert_at = lines
        .iter()
        .position(|l| l.trim() == "[profile.default]")
        .map(|i| i + 1)
        .unwrap_or(lines.len());
    if comment.is_empty() {
        lines.insert(insert_at, line.to_string());
    } else {
        lines.insert(insert_at, line.to_string());
        lines.insert(insert_at, comment.to_string());
    }
    let mut s = lines.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn config_path() -> Result<PathBuf, anyhow::Error> {
    if let Ok(p) = env::var("CLIDO_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    directories::ProjectDirs::from("", "", "clido")
        .map(|d| d.config_dir().join("config.toml"))
        .ok_or_else(|| CliError::Config("Could not determine config directory.".into()).into())
}
