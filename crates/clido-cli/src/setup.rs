//! Interactive setup flow: first-run, `clido init`, model selection.

use inquire::{Confirm, Select, Text};
use std::env;
use std::io::{self, BufRead, Write};

use crate::errors::CliError;
use crate::ui::{ansi, setup_banner_rich, setup_use_color, setup_use_rich_ui, SETUP_BANNER_ASCII};

const ANTHROPIC_MODELS: &[&str] = &[
    "claude-sonnet-4-5",
    "claude-opus-4-5",
    "claude-haiku-4-5-20251001",
    "claude-3-5-sonnet-20241022",
    "claude-3-5-haiku-20241022",
    "Custom...",
];

const OPENROUTER_MODELS: &[&str] = &[
    "anthropic/claude-3-5-sonnet",
    "anthropic/claude-haiku-3-5",
    "openai/gpt-4o",
    "openai/gpt-4o-mini",
    "google/gemini-2.0-flash",
    "Custom...",
];

/// Interactive setup flow (CLI spec §4): ask provider, model, API key/base URL, write config.
/// Returns (config_path, toml_content). Runs in a blocking thread for stdin reads.
pub fn run_interactive_setup_blocking(
    init_subline: Option<&str>,
) -> Result<(std::path::PathBuf, String), anyhow::Error> {
    let config_path = if let Ok(p) = env::var("CLIDO_CONFIG") {
        std::path::PathBuf::from(p)
    } else {
        let dir = directories::ProjectDirs::from("", "", "clido")
            .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;
        dir.config_dir().join("config.toml")
    };

    let mut line = String::new();
    let use_color = setup_use_color();
    let rich = setup_use_rich_ui();

    // Read the existing stored api_key (if any) so the credential prompt can show it.
    let existing_stored_key: Option<String> = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| {
                    let t = l.trim_start();
                    // Match `api_key = "..."` but NOT `api_key_env = "..."`
                    t.starts_with("api_key") && !t.starts_with("api_key_env")
                })
                .and_then(|l| l.split_once('=').map(|x| x.1))
                .map(|v| v.trim().trim_matches('"').to_string())
        })
        .filter(|s| !s.is_empty());

    print_banner(rich, use_color, init_subline);

    // Use a scope so stdin lock is released before each prompt call can re-acquire it.
    // Each helper takes &mut impl BufRead so we pass a fresh lock each time.
    let provider = {
        let mut stdin = io::stdin().lock();
        prompt_provider(rich, &mut stdin, &mut line)?
    };
    let model = {
        let mut stdin = io::stdin().lock();
        prompt_model(provider, rich, &mut stdin, &mut line)?
    };
    let toml = {
        let mut stdin = io::stdin().lock();
        prompt_credentials(
            provider,
            &model,
            rich,
            use_color,
            existing_stored_key.as_deref(),
            &mut stdin,
            &mut line,
        )?
    };

    Ok((config_path, toml))
}

fn print_banner(rich: bool, use_color: bool, init_subline: Option<&str>) {
    if rich {
        if use_color {
            eprintln!(
                "{}{}  Welcome to Clido.{} {}Let's set up your environment.{}",
                ansi::BOLD,
                ansi::BRIGHT_CYAN,
                ansi::RESET,
                ansi::DIM,
                ansi::RESET
            );
        } else {
            eprintln!("  Welcome to Clido. Let's set up your environment.");
        }
        eprintln!();
        if use_color {
            eprint!("{}", ansi::CYAN);
        }
        eprintln!("{}", setup_banner_rich());
        if use_color {
            eprint!("{}", ansi::RESET);
        }
        eprintln!();
        if let Some(s) = init_subline {
            if use_color {
                eprintln!("{}{}{}", ansi::DIM, s, ansi::RESET);
            } else {
                eprintln!("{}", s);
            }
        }
        let _ = io::stderr().flush();
    } else {
        eprintln!("{}", SETUP_BANNER_ASCII);
        if let Some(s) = init_subline {
            eprintln!("{}", s);
        }
    }
}

fn prompt_provider(
    rich: bool,
    stdin: &mut impl BufRead,
    line: &mut String,
) -> Result<u8, anyhow::Error> {
    if rich {
        let options: Vec<&str> = vec![
            "Anthropic (cloud) — requires API key",
            "OpenRouter (cloud) — requires API key",
            "Local (Ollama) — no key; use http://localhost:11434",
        ];
        let selected = Select::new("Provider:", options)
            .with_starting_cursor(0)
            .prompt()
            .map_err(|e| anyhow::anyhow!("prompt: {}", e))?;
        Ok(if selected.contains("Local") {
            3
        } else if selected.contains("OpenRouter") {
            2
        } else {
            1
        })
    } else {
        eprintln!("  Provider:");
        eprintln!("    1) Anthropic (cloud)  — requires API key");
        eprintln!("    2) OpenRouter (cloud) — requires API key");
        eprintln!("    3) Local (Ollama)     — no key; use http://localhost:11434");
        eprintln!("  Type 1, 2, or 3, then press Enter [default: 1]:");
        line.clear();
        stdin
            .read_line(line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        Ok(line.trim().parse().unwrap_or(1))
    }
}

fn prompt_model(
    provider: u8,
    rich: bool,
    stdin: &mut impl BufRead,
    line: &mut String,
) -> Result<String, anyhow::Error> {
    match provider {
        1 => prompt_model_from_list(ANTHROPIC_MODELS, "claude-sonnet-4-5", rich, stdin, line),
        2 => prompt_model_from_list(
            OPENROUTER_MODELS,
            "anthropic/claude-3-5-sonnet",
            rich,
            stdin,
            line,
        ),
        _ => {
            // Local: free text
            if rich {
                Text::new("Model:")
                    .with_default("llama3.2")
                    .prompt()
                    .map_err(|e| anyhow::anyhow!("prompt: {}", e))
            } else {
                eprintln!("  Model (press Enter for llama3.2):");
                line.clear();
                stdin
                    .read_line(line)
                    .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
                let m = line.trim();
                Ok(if m.is_empty() {
                    "llama3.2".to_string()
                } else {
                    m.to_string()
                })
            }
        }
    }
}

fn prompt_model_from_list(
    models: &[&str],
    default: &str,
    rich: bool,
    stdin: &mut impl BufRead,
    line: &mut String,
) -> Result<String, anyhow::Error> {
    if rich {
        let selected = Select::new("Model:", models.to_vec())
            .with_starting_cursor(0)
            .prompt()
            .map_err(|e| anyhow::anyhow!("prompt: {}", e))?;
        if selected == "Custom..." {
            Text::new("Model ID:")
                .with_default(default)
                .prompt()
                .map_err(|e| anyhow::anyhow!("prompt: {}", e))
        } else {
            Ok(selected.to_string())
        }
    } else {
        eprintln!("  Model (press Enter for {}):", default);
        line.clear();
        stdin
            .read_line(line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let m = line.trim();
        Ok(if m.is_empty() {
            default.to_string()
        } else {
            m.to_string()
        })
    }
}

fn prompt_credentials(
    provider: u8,
    model: &str,
    rich: bool,
    use_color: bool,
    stored_key: Option<&str>,
    stdin: &mut impl BufRead,
    line: &mut String,
) -> Result<String, anyhow::Error> {
    match provider {
        3 => {
            // Local/Ollama: ask base URL
            let base_url = if rich {
                Text::new("Ollama base URL")
                    .with_default("http://localhost:11434")
                    .prompt()
                    .map_err(|e| anyhow::anyhow!("prompt: {}", e))?
            } else {
                eprintln!("  Ollama base URL (press Enter for http://localhost:11434):");
                line.clear();
                stdin
                    .read_line(line)
                    .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
                let b = line.trim();
                if b.is_empty() {
                    "http://localhost:11434".to_string()
                } else {
                    b.to_string()
                }
            };
            let base_url = if base_url.is_empty() {
                "http://localhost:11434"
            } else {
                base_url.as_str()
            };
            Ok(format!(
                r#"default_profile = "default"

[profile.default]
provider = "local"
model = "{}"
base_url = "{}"
"#,
                model, base_url
            ))
        }
        2 => cloud_key_toml(
            "openrouter",
            model,
            "OPENROUTER_API_KEY",
            stored_key,
            rich,
            use_color,
            stdin,
            line,
        ),
        _ => cloud_key_toml(
            "anthropic",
            model,
            "ANTHROPIC_API_KEY",
            stored_key,
            rich,
            use_color,
            stdin,
            line,
        ),
    }
}

/// Anonymize an API key for display: show first 4 chars, dots, last 4 chars.
/// e.g. "sk-or-v1-abc...xyz" → "sk-o···xyz"
pub fn anonymize_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "···".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{}···{}", head, tail)
}

#[allow(clippy::too_many_arguments)]
fn cloud_key_toml(
    provider: &str,
    model: &str,
    key_env: &str,
    stored_key: Option<&str>,
    rich: bool,
    use_color: bool,
    stdin: &mut impl BufRead,
    line: &mut String,
) -> Result<String, anyhow::Error> {
    // Show whichever key is available (stored in config takes priority, then env var).
    let env_key = std::env::var(key_env).ok();
    let display_key: Option<String> = stored_key
        .map(anonymize_key)
        .or_else(|| env_key.as_ref().map(|k| anonymize_key(k)));

    let has_existing = display_key.is_some();
    let prompt_msg = match &display_key {
        Some(d) => format!("Use this key? ({})", d),
        None => format!("Enter your {} API key", key_env),
    };
    let ascii_msg = match &display_key {
        Some(d) => format!("  Use this key? ({}) [Y/n]:", d),
        None => format!("  Enter your {} API key [Y/n]:", key_env),
    };

    let use_env = if rich {
        Confirm::new(&prompt_msg)
            .with_default(has_existing)
            .prompt()
            .map_err(|e| anyhow::anyhow!("prompt: {}", e))?
    } else {
        eprintln!("{}", ascii_msg);
        line.clear();
        stdin
            .read_line(line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        line.trim().is_empty()
            || line.trim().eq_ignore_ascii_case("y")
            || line.trim().eq_ignore_ascii_case("yes")
    };

    if use_env {
        // Resolve the actual key value so it is stored directly — not as an env var reference.
        // This avoids "key works at setup time but not later" when the env var isn't exported.
        let resolved = stored_key
            .map(|s| s.to_string())
            .or_else(|| env_key.clone());
        if let Some(key) = resolved {
            eprintln!("  ✓ Key stored: {}", anonymize_key(&key));
            return Ok(format!(
                r#"default_profile = "default"

[profile.default]
provider = "{}"
model = "{}"
# api_key is stored in plain text — keep this file private (chmod 600).
api_key = "{}"
"#,
                provider, model, key
            ));
        }
        // Nothing available — fall through to manual entry.
    }

    // User wants to store the key directly in config.
    let api_key = if rich {
        Text::new("API key:")
            .with_help_message("Paste your key and press Enter")
            .prompt()
            .map_err(|e| anyhow::anyhow!("prompt: {}", e))?
    } else {
        eprintln!("  Enter your API key (paste and press Enter — will be stored in config):");
        line.clear();
        stdin
            .read_line(line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        line.trim().to_string()
    };

    if !api_key.is_empty() {
        eprintln!("  ✓ Key stored: {}", anonymize_key(&api_key));
    }

    if api_key.is_empty() {
        // Nothing entered — fall back to env var hint.
        if use_color {
            eprintln!(
                "{}  No key entered. Set {} in your environment and re-run 'clido init'.{}",
                ansi::DIM,
                key_env,
                ansi::RESET
            );
        } else {
            eprintln!(
                "  No key entered. Set {} in your environment and re-run 'clido init'.",
                key_env
            );
        }
        return Ok(format!(
            r#"default_profile = "default"

[profile.default]
provider = "{}"
model = "{}"
api_key_env = "{}"
"#,
            provider, model, key_env
        ));
    }

    Ok(format!(
        r#"default_profile = "default"

[profile.default]
provider = "{}"
model = "{}"
# api_key is stored in plain text — keep this file private (chmod 600).
api_key = "{}"
"#,
        provider, model, api_key
    ))
}

/// First-run: no config and TTY → run setup, write config, continue.
pub async fn run_first_run_setup() -> Result<(), anyhow::Error> {
    let use_color = crate::ui::cli_use_color();
    if use_color {
        eprintln!(
            "{}No configuration found. Running first-time setup.{}",
            ansi::DIM,
            ansi::RESET
        );
    } else {
        eprintln!("No configuration found. Running first-time setup.");
    }
    write_setup_config(None, false).await
}

/// `clido init` subcommand.
pub async fn run_init() -> Result<(), anyhow::Error> {
    write_setup_config(
        Some("  Re-run 'clido init' anytime to change provider or reset config."),
        true,
    )
    .await
}

/// `use_stdout`: true for `init` (output to stdout), false for first-run (output to stderr so
/// it doesn't mix with the agent's subsequent stdout).
async fn write_setup_config(
    subline: Option<&'static str>,
    use_stdout: bool,
) -> Result<(), anyhow::Error> {
    let (config_path, toml) =
        tokio::task::spawn_blocking(move || run_interactive_setup_blocking(subline))
            .await
            .map_err(|e| anyhow::anyhow!("setup: {}", e))??;
    let parent = config_path
        .parent()
        .ok_or_else(|| CliError::Usage("Invalid config path.".into()))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&config_path, toml.trim_start())?;
    // Restrict config file to owner-only read/write (chmod 600) so API keys stay private.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&config_path, perms);
    }
    let msg = format!(
        "  Created {}. Run 'clido doctor' to verify.",
        config_path.display()
    );
    let use_color = setup_use_color();
    if use_stdout {
        if use_color {
            println!("{}{}{}", ansi::GREEN, msg, ansi::RESET);
        } else {
            println!("{}", msg);
        }
    } else if use_color {
        eprintln!("{}{}{}", ansi::GREEN, msg, ansi::RESET);
    } else {
        eprintln!("{}", msg);
    }
    Ok(())
}
