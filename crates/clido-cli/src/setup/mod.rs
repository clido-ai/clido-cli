//! Interactive setup flow: first-run, `clido init`, provider + model selection.
//!
//! Flow: choose provider → enter API key / base URL → fetch models from API → choose model.
//!
//! TTY  → full-screen ratatui TUI.
//! No TTY → plain stdin/stdout (CI, pipes).

mod config;
mod event_loop;
mod render;
mod types;

use std::env;
use std::io::{self, BufRead};
use std::path::PathBuf;

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, style::Color, Terminal};

use crate::errors::CliError;
use crate::ui::{setup_use_color, setup_use_rich_ui, SETUP_BANNER_ASCII};

use clido_providers::registry::PROVIDER_REGISTRY;

pub use types::SetupPreFill;
use types::{build_saved_key_catalog, SetupOutcome};

pub(crate) use config::read_credential;
pub(crate) use config::upsert_credential;

use config::{
    build_full_config_toml, collect_credentials_from_state, credentials_path,
    state_to_profile_entry, write_credentials_file,
};
use event_loop::setup_event_loop;

/// Result of a completed setup: (config_path, toml_content, credentials).
type SetupResult = (PathBuf, String, Vec<(String, String)>);

/// Border accent for setup text inputs — matches main TUI soft blue (`tui.rs` `TUI_SOFT_ACCENT`).
const SETUP_INPUT_ACCENT: Color = Color::Rgb(150, 200, 255);

const PROFILE_NAME_PREFIX: &str = "  Profile name: ";

/// Options shown on the sub-agent intro screen.
const FAST_PROVIDER_OPTIONS: &[(&str, &str)] = &[
    (
        "Configure fast provider",
        "cheaper model handles titles, summaries, utility tasks",
    ),
    ("Skip for now", "can add a fast provider later via /profile"),
];

// ── TUI entry point ───────────────────────────────────────────────────────────

fn run_tui_setup_blocking(pre_fill: Option<SetupPreFill>) -> Result<SetupOutcome, anyhow::Error> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = setup_event_loop(&mut terminal, pre_fill);

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
}

async fn run_tui_setup(pre_fill: Option<SetupPreFill>) -> Result<SetupOutcome, anyhow::Error> {
    tokio::task::spawn_blocking(move || run_tui_setup_blocking(pre_fill))
        .await
        .map_err(|e| anyhow::anyhow!("setup join: {}", e))?
}

// ── Plain-text fallback (non-TTY) ─────────────────────────────────────────────

/// Non-TTY setup: plain stdin/stdout prompts (CI, pipes).
pub fn run_interactive_setup_blocking(
    _init_subline: Option<&str>,
) -> Result<SetupResult, anyhow::Error> {
    let config_path = if let Ok(p) = env::var("CLIDO_CONFIG") {
        PathBuf::from(p)
    } else {
        let dir = directories::ProjectDirs::from("", "", "clido")
            .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;
        dir.config_dir().join("config.toml")
    };

    let mut line = String::new();
    let mut stdin = io::stdin().lock();
    eprintln!("{}", SETUP_BANNER_ASCII);

    // Step 1: Provider
    eprintln!("  Provider:");
    for (i, def) in PROVIDER_REGISTRY.iter().enumerate() {
        eprintln!("    {}) {:<16}  {}", i + 1, def.name, def.description);
    }
    eprintln!("  Enter 1–{}: ", PROVIDER_REGISTRY.len());
    line.clear();
    stdin
        .read_line(&mut line)
        .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
    let pidx: usize = line.trim().parse::<usize>().unwrap_or(0);
    if pidx < 1 || pidx > PROVIDER_REGISTRY.len() {
        return Err(anyhow::anyhow!(
            "Invalid choice. Run 'clido init' again and enter 1–{}.",
            PROVIDER_REGISTRY.len()
        ));
    }
    let pidx = pidx - 1;
    let provider = PROVIDER_REGISTRY[pidx].id;
    let is_local = PROVIDER_REGISTRY[pidx].is_local;

    // Step 2: Credential (API key or base URL)
    let credential = if is_local {
        eprintln!("  Base URL (Enter for http://localhost:11434):");
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let url = line.trim();
        if url.is_empty() {
            "http://localhost:11434".to_string()
        } else {
            url.to_string()
        }
    } else {
        let key_env = PROVIDER_REGISTRY[pidx].api_key_env;
        eprintln!("  {} (paste and press Enter):", key_env);
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let api_key = line.trim();
        if api_key.is_empty() {
            return Err(anyhow::anyhow!(
                "No API key entered. Run 'clido init' again and paste your key."
            ));
        }
        api_key.to_string()
    };

    // Step 3: Fetch models from API, then ask user to pick
    let (api_key_for_fetch, base_url_for_fetch): (&str, Option<&str>) = if is_local {
        ("", Some(credential.as_str()))
    } else {
        (credential.as_str(), None)
    };
    let handle = tokio::runtime::Handle::current();
    let fetched = handle
        .block_on(clido_providers::fetch_provider_models(
            provider,
            api_key_for_fetch,
            base_url_for_fetch,
        ))
        .unwrap_or_default();

    let model = if fetched.is_empty() {
        eprintln!("  (Couldn't fetch model list — enter model ID manually)");
        eprintln!("  Model ID:");
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let m = line.trim();
        if m.is_empty() {
            return Err(anyhow::anyhow!(
                "No model entered. Run 'clido init' again and type a model ID."
            ));
        }
        m.to_string()
    } else {
        eprintln!("  Model:");
        for (i, m) in fetched.iter().enumerate() {
            let avail_note = if !m.available { "  [no endpoints]" } else { "" };
            eprintln!("    {}) {}{}", i + 1, m.id, avail_note);
        }
        eprintln!("  Enter 1–{} (or type a custom ID): ", fetched.len());
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let choice = line.trim();
        if let Ok(midx) = choice.parse::<usize>() {
            if midx >= 1 && midx <= fetched.len() {
                fetched[midx - 1].id.clone()
            } else {
                return Err(anyhow::anyhow!("Invalid choice. Run 'clido init' again."));
            }
        } else if !choice.is_empty() {
            choice.to_string()
        } else {
            return Err(anyhow::anyhow!(
                "No model entered. Run 'clido init' again and type a model ID."
            ));
        }
    };

    let credentials: Vec<(String, String)> = if is_local {
        vec![]
    } else {
        vec![(provider.to_string(), credential.clone())]
    };
    let toml = if is_local {
        format!(
            "default_profile = \"default\"\n\n[profile.default]\nprovider = \"local\"\nmodel = \"{}\"\nbase_url = \"{}\"\n",
            model, credential
        )
    } else {
        format!(
            "default_profile = \"default\"\n\n[profile.default]\nprovider = \"{}\"\nmodel = \"{}\"\n# API keys are stored in the credentials file (same directory as this file).\n",
            provider, model
        )
    };

    Ok((config_path, toml, credentials))
}

// ── Anonymize helper ──────────────────────────────────────────────────────────

/// Show first 4 + `···` + last 4 chars of a key.
pub fn anonymize_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "···".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{}···{}", head, tail)
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Detect the first API key present in the environment and return `(provider_id, env_var_name)`.
/// Used during first-run to pre-select the provider and skip the credential entry step.
pub fn detect_provider_from_env() -> Option<(&'static str, &'static str)> {
    const CHECKS: &[(&str, &str)] = &[
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("openrouter", "OPENROUTER_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"),
        ("groq", "GROQ_API_KEY"),
        ("gemini", "GEMINI_API_KEY"),
        ("xai", "XAI_API_KEY"),
        ("mistral", "MISTRAL_API_KEY"),
        ("togetherai", "TOGETHER_API_KEY"),
        ("perplexity", "PERPLEXITY_API_KEY"),
    ];
    for &(provider_id, env_var) in CHECKS {
        if std::env::var(env_var)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        {
            return Some((provider_id, env_var));
        }
    }
    None
}

/// First-run: no config and TTY → run TUI setup, write config, continue.
/// If an API key is already in the environment, pre-select that provider and
/// skip the credential entry step so the user only needs to pick a model.
pub async fn run_first_run_setup() -> Result<(), anyhow::Error> {
    let pre_fill = detect_provider_from_env().map(|(provider_id, env_var)| SetupPreFill {
        provider: provider_id.to_string(),
        api_key: std::env::var(env_var).unwrap_or_default(),
        model: String::new(),
        profile_name: String::new(),
        is_new_profile: false,
        saved_api_keys: Vec::new(),
    });
    write_setup_config(false, pre_fill).await
}

/// `clido init` subcommand.
pub async fn run_init() -> Result<(), anyhow::Error> {
    write_setup_config(true, None).await
}

/// Re-run setup from within the TUI (/init command), pre-filling with current config values.
pub async fn run_reinit(pre_fill: SetupPreFill) -> Result<(), anyhow::Error> {
    write_setup_config(true, Some(pre_fill)).await
}

/// Create a new named profile via the guided wizard.
pub async fn run_create_profile(initial_name: Option<String>) -> Result<(), anyhow::Error> {
    let config_path = clido_core::global_config_path()
        .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;

    // Load config from global path so we see all profiles and credentials
    let loaded = clido_core::load_config(&config_path)
        .or_else(|_| clido_core::load_config(&PathBuf::from(".")))
        .ok();

    let saved_api_keys = loaded
        .as_ref()
        .map(|c| {
            let ex = initial_name
                .as_deref()
                .filter(|n| c.profiles.contains_key(*n));
            build_saved_key_catalog(c, &config_path, ex)
        })
        .unwrap_or_default();

    // Detect env vars to pre-select provider (same as first-run)
    let pre_fill = if let Some((provider_id, env_var)) = detect_provider_from_env() {
        SetupPreFill {
            provider: provider_id.to_string(),
            api_key: std::env::var(env_var).unwrap_or_default(),
            model: String::new(),
            profile_name: initial_name.clone().unwrap_or_default(),
            is_new_profile: initial_name.is_none(),
            saved_api_keys,
        }
    } else {
        SetupPreFill {
            provider: String::new(),
            api_key: String::new(),
            model: String::new(),
            profile_name: initial_name.clone().unwrap_or_default(),
            is_new_profile: initial_name.is_none(), // show ProfileName step if no name given
            saved_api_keys,
        }
    };

    let state = if setup_use_rich_ui() {
        match tokio::task::spawn_blocking(move || run_tui_setup_state_blocking(Some(pre_fill)))
            .await
            .map_err(|e| anyhow::anyhow!("setup join: {}", e))??
        {
            SetupOutcome::Cancelled => return Ok(()),
            SetupOutcome::Finished(s) => *s,
        }
    } else {
        return Err(anyhow::anyhow!(
            "Profile creation requires an interactive terminal. Run in a TTY."
        ));
    };

    // Determine profile name from state (either from ProfileName step or from initial_name)
    let pname = if state.profile_name.is_empty() {
        return Err(anyhow::anyhow!("No profile name provided."));
    } else {
        state.profile_name.clone()
    };

    let mut entry = state_to_profile_entry(&state);

    let parent = config_path
        .parent()
        .ok_or_else(|| CliError::Usage("Invalid config path.".into()))?;
    std::fs::create_dir_all(parent)?;

    // Save API key(s) to the credentials file (same as first-run setup),
    // then strip them from the ProfileEntry so config.toml stays clean.
    let credentials = collect_credentials_from_state(&state);
    for (provider_id, api_key) in &credentials {
        let _ = config::upsert_credential(&config_path, provider_id, api_key);
    }
    if !credentials.is_empty() {
        entry.api_key = None;
        if let Some(ref mut fast) = entry.fast {
            fast.api_key = None;
        }
    }

    clido_core::upsert_profile_in_config(&config_path, &pname, &entry)
        .map_err(|e| CliError::Config(e.to_string()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&config_path, perms);
    }

    let use_color = setup_use_color();
    if use_color {
        println!(
            "\x1b[32m  Profile '{}' created. Run 'clido profile switch {}' to activate.\x1b[0m",
            pname, pname
        );
    } else {
        println!(
            "  Profile '{}' created. Run 'clido profile switch {}' to activate.",
            pname, pname
        );
    }
    Ok(())
}

/// Edit an existing profile via the guided wizard (pre-filled with current values).
pub async fn run_edit_profile(
    name: String,
    entry: clido_core::ProfileEntry,
) -> Result<(), anyhow::Error> {
    let config_path = clido_core::global_config_path()
        .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;

    // Resolve the API key from credentials file, env var, then inline (same lookup as `build_saved_key_catalog`)
    let api_key = crate::setup::read_credential(&config_path, &entry.provider)
        .or_else(|| {
            entry
                .api_key_env
                .as_ref()
                .and_then(|e| std::env::var(e).ok())
        })
        .or_else(|| entry.api_key.clone())
        .unwrap_or_default();

    // Load config from global path so we see all profiles
    let loaded = clido_core::load_config(&config_path)
        .or_else(|_| clido_core::load_config(&PathBuf::from(".")))
        .ok();
    let saved_api_keys = loaded
        .as_ref()
        .map(|c| build_saved_key_catalog(c, &config_path, Some(name.as_str())))
        .unwrap_or_default();

    let pre_fill = SetupPreFill {
        provider: entry.provider.clone(),
        api_key,
        model: entry.model.clone(),
        profile_name: name.clone(),
        is_new_profile: false,
        saved_api_keys,
    };

    let config_path = clido_core::global_config_path()
        .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;

    let state = if setup_use_rich_ui() {
        match tokio::task::spawn_blocking(move || run_tui_setup_state_blocking(Some(pre_fill)))
            .await
            .map_err(|e| anyhow::anyhow!("setup join: {}", e))??
        {
            SetupOutcome::Cancelled => return Ok(()),
            SetupOutcome::Finished(s) => *s,
        }
    } else {
        return Err(anyhow::anyhow!(
            "Profile editing requires an interactive terminal. Run in a TTY."
        ));
    };

    let mut updated_entry = state_to_profile_entry(&state);

    // Save API key(s) to the credentials file (same as first-run setup),
    // then strip them from the ProfileEntry so config.toml stays clean.
    let credentials = collect_credentials_from_state(&state);
    for (provider_id, api_key) in &credentials {
        let _ = config::upsert_credential(&config_path, provider_id, api_key);
    }
    if !credentials.is_empty() {
        updated_entry.api_key = None;
        if let Some(ref mut fast) = updated_entry.fast {
            fast.api_key = None;
        }
    }

    clido_core::upsert_profile_in_config(&config_path, &name, &updated_entry)
        .map_err(|e| CliError::Config(e.to_string()))?;

    let use_color = setup_use_color();
    if use_color {
        println!("\x1b[32m  Profile '{}' updated.\x1b[0m", name);
    } else {
        println!("  Profile '{}' updated.", name);
    }
    Ok(())
}

/// Run the TUI setup and return finished state or cancellation (profile create/edit).
fn run_tui_setup_state_blocking(
    pre_fill: Option<SetupPreFill>,
) -> Result<SetupOutcome, anyhow::Error> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = setup_event_loop(&mut terminal, pre_fill);

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
}

async fn write_setup_config(
    use_stdout: bool,
    pre_fill: Option<SetupPreFill>,
) -> Result<(), anyhow::Error> {
    let (config_path, toml, credentials) = if setup_use_rich_ui() {
        match run_tui_setup(pre_fill).await? {
            SetupOutcome::Cancelled => return Ok(()),
            SetupOutcome::Finished(state) => {
                let path = clido_core::global_config_path().ok_or_else(|| {
                    CliError::Usage("Could not determine config directory.".into())
                })?;
                let credentials = collect_credentials_from_state(&state);
                let toml = build_full_config_toml(&state);
                (path, toml, credentials)
            }
        }
    } else {
        tokio::task::spawn_blocking(|| run_interactive_setup_blocking(None))
            .await
            .map_err(|e| anyhow::anyhow!("setup: {}", e))??
    };

    let parent = config_path
        .parent()
        .ok_or_else(|| CliError::Usage("Invalid config path.".into()))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&config_path, toml.trim_start())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&config_path, perms);
    }

    // Write credentials file alongside config
    if !credentials.is_empty() {
        let creds_path = credentials_path(&config_path);
        let _ = write_credentials_file(&creds_path, &credentials);
    }

    let msg = format!(
        "  Created {}. Run 'clido doctor' to verify.",
        config_path.display()
    );
    let use_color = setup_use_color();
    if use_stdout {
        if use_color {
            println!("\x1b[32m{}\x1b[0m", msg);
        } else {
            println!("{}", msg);
        }
    } else if use_color {
        eprintln!("\x1b[32m{}\x1b[0m", msg);
    } else {
        eprintln!("{}", msg);
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_input::TextInput;
    use clido_providers::ModelMetadata;
    use config::{build_toml, collect_credentials_from_state, write_credentials_file};
    use types::{SetupState, SetupStep};

    #[test]
    fn profile_name_prefix_matches_draw_constant() {
        assert_eq!(PROFILE_NAME_PREFIX, "  Profile name: ");
    }

    #[test]
    fn text_input_insert_and_delete_utf8() {
        let mut ti = TextInput::new();
        ti.set_text("aéb");
        ti.cursor = 1; // after 'a'
        ti.insert_char('x');
        assert_eq!(ti.text, "axéb");
        assert_eq!(ti.cursor, 2);
        ti.delete_back();
        assert_eq!(ti.text, "aéb");
        assert_eq!(ti.cursor, 1);
        ti.cursor = 2; // after 'é'
        ti.delete_forward();
        assert_eq!(ti.text, "aé");
        assert_eq!(ti.cursor, 2);
    }

    #[test]
    fn anonymize_key_short_returns_dots() {
        assert_eq!(anonymize_key("short"), "···");
        assert_eq!(anonymize_key("12345678"), "···");
    }

    #[test]
    fn anonymize_key_long_shows_head_and_tail() {
        let key = "sk-ant-api03-longkeysomethinghere12345";
        let anon = anonymize_key(key);
        assert!(anon.starts_with("sk-a"));
        assert!(anon.ends_with("2345"));
        assert!(anon.contains("···"));
    }

    #[test]
    fn anonymize_key_exactly_nine_chars() {
        let key = "123456789";
        let anon = anonymize_key(key);
        assert!(anon.starts_with("1234"));
        assert!(anon.ends_with("6789"));
    }

    #[test]
    fn build_toml_local_provider() {
        let mut s = SetupState::new();
        s.provider = 17; // Local / Ollama
        s.model = "llama3.2".to_string();
        s.credential.clear();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"local\""));
        assert!(toml.contains("model = \"llama3.2\""));
        assert!(toml.contains("base_url = \"http://localhost:11434\""));
        assert!(!toml.contains("api_key"));
    }

    #[test]
    fn build_toml_local_provider_custom_url() {
        let mut s = SetupState::new();
        s.provider = 17; // Local / Ollama
        s.model = "mistral".to_string();
        s.credential = "http://127.0.0.1:8080".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("base_url = \"http://127.0.0.1:8080\""));
    }

    #[test]
    fn build_toml_anthropic_provider() {
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-api03-secret".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"anthropic\""));
        assert!(toml.contains("model = \"claude-sonnet-4-5\""));
        assert!(
            !toml.contains("api_key ="),
            "api_key must not appear in config.toml"
        );
    }

    #[test]
    fn build_toml_openrouter_provider() {
        let mut s = SetupState::new();
        s.provider = 0; // OpenRouter
        s.model = "anthropic/claude-3-5-sonnet".to_string();
        s.credential = "sk-or-test-key".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"openrouter\""));
        assert!(
            !toml.contains("api_key ="),
            "api_key must not appear in config.toml"
        );
    }

    #[test]
    fn build_toml_openai_provider() {
        let mut s = SetupState::new();
        s.provider = 2; // OpenAI
        s.model = "gpt-4o".to_string();
        s.credential = "sk-openai-test".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"openai\""));
        assert!(toml.contains("model = \"gpt-4o\""));
        assert!(
            !toml.contains("api_key ="),
            "api_key must not appear in config.toml"
        );
    }

    #[test]
    fn build_toml_mistral_provider() {
        let mut s = SetupState::new();
        s.provider = 3; // Mistral
        s.model = "mistral-large-latest".to_string();
        s.credential = "mk-test-key".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"mistral\""));
        assert!(toml.contains("model = \"mistral-large-latest\""));
    }

    #[test]
    fn build_toml_minimax_provider() {
        let mut s = SetupState::new();
        s.provider = 4; // MiniMax
        s.model = "MiniMax-M2.7".to_string();
        s.credential = "sk-minimax-test".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"minimax\""));
        assert!(toml.contains("model = \"MiniMax-M2.7\""));
        assert!(
            !toml.contains("api_key ="),
            "api_key must not appear in config.toml"
        );
    }

    #[test]
    fn setup_state_new_defaults() {
        let s = SetupState::new();
        assert_eq!(s.step, SetupStep::Provider);
        assert_eq!(s.provider_picker.selected, 0);
        assert_eq!(s.model_picker.selected, 0);
        assert!(!s.custom_model);
        assert!(s.model.is_empty());
        assert!(s.text_input.text.is_empty());
        assert!(s.error.is_none());
        assert!(s.fetched_models.is_empty());
    }

    #[test]
    fn setup_state_is_local() {
        let mut s = SetupState::new();
        s.provider = 17; // Local / Ollama is index 17
        assert!(s.is_local());
        s.provider = 0;
        assert!(!s.is_local());
    }

    #[test]
    fn setup_state_model_list_mode() {
        let mut s = SetupState::new();
        assert!(!s.model_list_mode()); // no fetched models
        s.fetched_models = vec![ModelMetadata::available("gpt-4o")];
        assert!(s.model_list_mode()); // has models, not in custom mode
        s.custom_model = true;
        assert!(!s.model_list_mode()); // custom mode overrides
    }

    #[test]
    fn provider_registry_consistency() {
        use clido_providers::registry::PROVIDER_REGISTRY;
        assert_eq!(PROVIDER_REGISTRY.len(), 18);
        assert!(PROVIDER_REGISTRY.last().unwrap().is_local);
        assert_eq!(PROVIDER_REGISTRY.last().unwrap().api_key_env, "");
        for def in PROVIDER_REGISTRY {
            assert!(!def.id.is_empty());
        }
    }

    // ── build_toml agents section tests ────────────────────────────────────

    #[test]
    fn build_toml_profile_does_not_embed_api_key() {
        // Regression: [profile.default] must NOT store api_key inline (goes to credentials file)
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-secret-key".to_string();
        let toml = build_toml(&s);
        assert!(
            toml.contains("[profile.default]"),
            "profile.default section missing"
        );
        assert!(
            !toml.contains("api_key ="),
            "api_key must not appear in config.toml (goes to credentials file)"
        );
        assert!(
            !toml.contains("api_key_env"),
            "profile must not use api_key_env when credential is known"
        );
    }

    #[test]
    fn build_toml_with_fast_provider_configured() {
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic (main)
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-main-key".to_string();
        s.configure_fast = true;
        s.fast_provider_idx = 2; // OpenAI
        s.fast_model = "gpt-4o-mini".to_string();
        s.fast_credential = "sk-openai-fast-key".to_string();
        let toml = build_toml(&s);
        assert!(
            toml.contains("[profile.default.fast]"),
            "fast provider section missing"
        );
        assert!(
            toml.contains("provider = \"openai\""),
            "fast provider missing"
        );
        assert!(
            toml.contains("model = \"gpt-4o-mini\""),
            "fast model missing"
        );
    }

    #[test]
    fn build_toml_local_fast_provider_uses_base_url() {
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic (main)
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-key".to_string();
        s.configure_fast = true;
        s.fast_provider_idx = 17; // Local
        s.fast_model = "llama3.2".to_string();
        s.fast_credential = "http://127.0.0.1:8080".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("[profile.default.fast]"));
        assert!(toml.contains("base_url = \"http://127.0.0.1:8080\""));
        let fast_section = &toml[toml.find("[profile.default.fast]").unwrap()..];
        assert!(
            !fast_section.contains("api_key"),
            "local fast provider should not have api_key"
        );
    }

    #[test]
    fn build_toml_no_credential_no_key_line() {
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic
        s.model = "claude-sonnet-4-5".to_string();
        s.credential.clear();
        let toml = build_toml(&s);
        assert!(toml.contains("[profile.default]"));
        assert!(
            !toml.contains("api_key ="),
            "should not have api_key when credential is empty"
        );
    }

    #[test]
    fn build_toml_no_fast_provider_by_default() {
        let mut s = SetupState::new();
        s.provider = 1;
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-key".to_string();
        let toml = build_toml(&s);
        assert!(
            !toml.contains("[profile.default.fast]"),
            "fast provider section should not appear when not configured"
        );
    }

    #[test]
    fn collect_credentials_includes_main_key() {
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-api03-secret".to_string();
        let creds = collect_credentials_from_state(&s);
        assert!(
            creds
                .iter()
                .any(|(id, key)| id == "anthropic" && key == "sk-ant-api03-secret"),
            "credentials should include main agent key"
        );
    }

    #[test]
    fn collect_credentials_local_provider_excluded() {
        let mut s = SetupState::new();
        s.provider = 17; // Local
        s.model = "llama3.2".to_string();
        s.credential = "http://localhost:11434".to_string();
        let creds = collect_credentials_from_state(&s);
        assert!(
            creds.is_empty(),
            "local provider should not produce credentials"
        );
    }

    #[test]
    fn write_credentials_file_creates_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials");
        let entries = vec![
            ("anthropic".to_string(), "sk-ant-test".to_string()),
            ("openai".to_string(), "sk-openai-test".to_string()),
        ];
        write_credentials_file(&path, &entries).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[keys]"));
        assert!(content.contains("anthropic = \"sk-ant-test\""));
        assert!(content.contains("openai = \"sk-openai-test\""));
    }
}
