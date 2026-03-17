//! Clido CLI: run agent, sessions, version, init.

mod cli;

use async_trait::async_trait;
use clap::Parser;
use clido_agent::{session_lines_to_messages, AgentLoop, AskUser};
use clido_core::{
    agent_config_from_loaded, load_config, load_pricing, ClidoError, LoadedConfig, PermissionMode,
};
use clido_providers::AnthropicProvider;
use clido_storage::{
    list_sessions, session_dir_for_project, stale_paths, SessionLine, SessionReader, SessionWriter,
};
use clido_tools::default_registry;
use std::env;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// ASCII banner shown when the agent starts (interactive text mode only).
const BANNER: &str = r#"          ▄▄   ▄▄                ▄▄           
        ▀███   ██              ▀███           
          ██                     ██           
 ▄██▀██   ██ ▀███    ██     ▄█▀▀███   ▄██▀██▄ 
██▀  ██   ██   ██    ▀▀   ▄██    ██  ██▀   ▀██
██        ██   ██         ███    ██  ██     ██
██▄    ▄  ██   ██    ▄▄   ▀██    ██  ██▄   ▄██
 █████▀ ▄████▄████▄  ▀█    ▀████▀███▄ ▀█████▀ 
                      ▀                        
                                             
"#;

/// Ask user on stderr/stdin for permission to run a state-changing tool (Default permission mode).
struct StdinAskUser;

#[async_trait]
impl AskUser for StdinAskUser {
    async fn ask(&self, tool_name: &str, input: &serde_json::Value) -> bool {
        let prompt = format!(
            "Allow {} with input {}? [y/N] ",
            tool_name,
            serde_json::to_string(input).unwrap_or_else(|_| "?".into())
        );
        let result = tokio::task::spawn_blocking(move || {
            eprint!("{}", prompt);
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

/// CLI exit codes per spec: 0 success, 1 runtime, 2 usage/config, 3 soft limit.
/// Doctor: 0 all pass, 1 mandatory failure, 2 warnings only.
#[derive(Error, Debug)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Runtime(String),
    #[error("{0}")]
    SoftLimit(String),
    #[error("{0}")]
    Interrupted(String),
    #[error("{0}")]
    DoctorMandatory(String),
    #[error("{0}")]
    DoctorWarnings(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            CliError::Runtime(_) => 1,
            CliError::SoftLimit(_) => 3,
            CliError::Interrupted(_) => 130,
            CliError::DoctorMandatory(_) => 1,
            CliError::DoctorWarnings(_) => 2,
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();

    let filter = tracing_subscriber::EnvFilter::try_from_env("CLIDO_LOG")
        .or_else(|_| tracing_subscriber::EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| {
            if cli.verbose {
                tracing_subscriber::EnvFilter::new("debug")
            } else {
                tracing_subscriber::EnvFilter::new("info")
            }
        });

    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("clido starting");

    let exit = match run(cli).await {
        Ok(()) => 0,
        Err(e) => {
            let code = if let Some(cli_err) = e.downcast_ref::<CliError>() {
                cli_err.exit_code()
            } else {
                1
            };
            eprintln!("Error: {}", e);
            code
        }
    };
    std::process::exit(exit);
}

async fn run(cli: cli::Cli) -> Result<(), anyhow::Error> {
    match &cli.subcommand {
        Some(cli::Subcommand::Version) => {
            println!("clido {}", VERSION);
            return Ok(());
        }
        Some(cli::Subcommand::ListSessions) => {
            eprintln!(
                "Warning: 'clido list-sessions' is deprecated. Use 'clido sessions list' instead."
            );
            return run_sessions_list().await;
        }
        Some(cli::Subcommand::ShowSession { id }) => {
            eprintln!("Warning: 'clido show-session' is deprecated. Use 'clido sessions show <id>' instead.");
            return run_sessions_show(id).await;
        }
        Some(cli::Subcommand::Sessions { cmd }) => match cmd {
            cli::SessionsCmd::List => return run_sessions_list().await,
            cli::SessionsCmd::Show { id } => return run_sessions_show(id).await,
        },
        Some(cli::Subcommand::Init) => {
            return run_init().await;
        }
        Some(cli::Subcommand::Doctor) => {
            return run_doctor().await;
        }
        None => {}
    }

    // Run agent
    let mut prompt = cli.prompt_str();
    if prompt.is_empty() {
        if cli.print {
            return Err(CliError::Usage(
                "No prompt provided. Pass a prompt as an argument or pipe it via stdin.".into(),
            )
            .into());
        }
        if !is_stdin_tty() {
            let mut stdin = String::new();
            io::stdin().read_to_string(&mut stdin)?;
            prompt = stdin.trim().to_string();
        }
        if prompt.is_empty() {
            return Err(CliError::Usage(
                "Usage: clido [-p] <prompt> or pipe prompt via stdin. Example: clido -p \"list files\""
                    .into(),
            )
            .into());
        }
    }

    // Resolve --resume / --continue (mutually exclusive).
    let resume_id = if let Some(id) = &cli.resume {
        if cli.r#continue {
            return Err(CliError::Usage(
                "Cannot use both --resume and --continue. Use one.".into(),
            )
            .into());
        }
        Some(id.clone())
    } else if cli.r#continue {
        let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let sessions = list_sessions(&cwd)?;
        let id = sessions
            .first()
            .map(|s| s.session_id.clone())
            .ok_or_else(|| {
                CliError::Usage("No session to continue. Run 'clido <prompt>' first.".into())
            })?;
        Some(id)
    } else {
        None
    };

    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    if cli.output_format == "stream-json" {
        return Err(CliError::Usage(
            "stream-json is not yet implemented. Use text or json.".into(),
        )
        .into());
    }

    // If resuming: load session once, run stale-file check, store lines for history reconstruction.
    let resume_lines = if let Some(ref session_id) = resume_id {
        let lines = SessionReader::load(&workspace_root, session_id)
            .map_err(|e| CliError::Usage(format!("Failed to load session: {}", e)))?;
        let records = SessionReader::stale_file_records(&lines);
        let stale = stale_paths(&records);
        if !stale.is_empty() && !cli.resume_ignore_stale {
            let msg = format!(
                "Cannot resume: file(s) modified since session: {} Use --resume-ignore-stale to continue anyway.",
                stale.join(", ")
            );
            if is_stdin_tty() && !cli.print {
                eprintln!("{}", msg);
                eprint!("Continue anyway? [y/N] ");
                let mut buf = String::new();
                if io::stdin().lock().read_line(&mut buf).is_ok()
                    && buf.trim().eq_ignore_ascii_case("y")
                {
                    // continue
                } else {
                    return Err(CliError::Usage(msg).into());
                }
            } else {
                return Err(CliError::Usage(msg).into());
            }
        }
        Some(lines)
    } else {
        None
    };

    let loaded = load_config(&workspace_root).map_err(|e| CliError::Usage(e.to_string()))?;
    let (pricing_table, _) = load_pricing();
    let profile_name = cli
        .profile
        .as_deref()
        .unwrap_or(loaded.default_profile.as_str());
    let profile = loaded
        .get_profile(profile_name)
        .map_err(|e| CliError::Usage(e.to_string()))?;
    LoadedConfig::validate_provider(&profile.provider)
        .map_err(|e| CliError::Usage(e.to_string()))?;

    let provider_name = cli.provider.as_deref().unwrap_or(profile.provider.as_str());
    if provider_name != "anthropic" {
        return Err(CliError::Usage(format!(
            "Unknown or unsupported provider '{}'. In V1 only 'anthropic' is implemented.\n  \
             Use --provider anthropic or omit --provider. Run: clido doctor",
            provider_name
        ))
        .into());
    }

    let api_key_env = profile
        .api_key_env
        .as_deref()
        .unwrap_or("ANTHROPIC_API_KEY");
    let api_key = env::var(api_key_env).map_err(|_| {
        CliError::Usage(format!(
            "API key not found for profile '{}'. Set {} in your environment. Run: clido doctor to check all configuration.",
            profile_name, api_key_env
        ))
    })?;

    let model = cli.model.clone().unwrap_or_else(|| profile.model.clone());
    let provider = Arc::new(AnthropicProvider::new(api_key, model.clone()));

    let mut registry = default_registry(workspace_root.clone());
    let allowed = cli
        .allowed_tools
        .clone()
        .or_else(|| cli.tools.clone())
        .or_else(|| {
            if loaded.tools.allowed.is_empty() {
                None
            } else {
                Some(loaded.tools.allowed.join(","))
            }
        })
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect());
    let disallowed = cli
        .disallowed_tools
        .clone()
        .or_else(|| {
            if loaded.tools.disallowed.is_empty() {
                None
            } else {
                Some(loaded.tools.disallowed.join(","))
            }
        })
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect());
    registry = registry.with_filters(allowed, disallowed);
    if registry.schemas().is_empty() {
        return Err(CliError::Usage(
            "No tools left after --allowed-tools/--disallowed-tools/--tools. Check your filters."
                .into(),
        )
        .into());
    }

    let permission_mode = match cli.permission_mode.as_deref() {
        Some("plan") | Some("plan-only") => PermissionMode::PlanOnly,
        Some("accept-all") => PermissionMode::AcceptAll,
        _ => PermissionMode::Default,
    };

    let system_prompt_base = if let Some(ref path) = cli.system_prompt_file {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::Usage(format!("Failed to read system prompt file: {}", e)))?
    } else if let Some(ref s) = cli.system_prompt {
        s.clone()
    } else {
        "You are a helpful coding assistant.".to_string()
    };
    let system_prompt = if let Some(ref append) = cli.append_system_prompt {
        format!("{}\n{}", system_prompt_base, append)
    } else {
        system_prompt_base
    };

    let mut config = agent_config_from_loaded(
        &loaded,
        profile_name,
        Some(cli.max_turns),
        cli.max_budget_usd,
        cli.model.clone(),
        Some(system_prompt),
        Some(permission_mode),
    )
    .map_err(|e| CliError::Usage(e.to_string()))?;

    if config.max_context_tokens.is_none() {
        if let Some(entry) = pricing_table.models.get(&config.model) {
            if let Some(cw) = entry.context_window {
                config.max_context_tokens = Some(cw);
            }
        }
    }

    let ask_user: Option<Arc<dyn AskUser>> =
        if permission_mode == PermissionMode::Default && io::stdin().is_terminal() {
            Some(Arc::new(StdinAskUser))
        } else {
            None
        };

    if cli.output_format == "text" && io::stdout().is_terminal() {
        print!("{}", BANNER);
        let _ = io::stdout().flush();
    }

    let (session_id, mut writer) = match &resume_id {
        Some(id) => (id.clone(), SessionWriter::append(&workspace_root, id)?),
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let w = SessionWriter::create(&workspace_root, &id)?;
            (id, w)
        }
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_handle = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel_handle.store(true, Ordering::Relaxed);
    });

    let start = std::time::Instant::now();
    let (result, num_turns, total_cost_usd) = match &resume_lines {
        Some(lines) => {
            let history = session_lines_to_messages(lines);
            if history.is_empty() {
                let mut loop_ =
                    AgentLoop::new(provider, registry, config.clone(), ask_user.clone());
                let r = loop_
                    .run(
                        &prompt,
                        Some(&mut writer),
                        Some(&pricing_table),
                        Some(cancel.clone()),
                    )
                    .await;
                (r, loop_.turn_count(), loop_.cumulative_cost_usd)
            } else {
                let mut loop_ = AgentLoop::new_with_history(
                    provider,
                    registry,
                    config.clone(),
                    history,
                    ask_user.clone(),
                );
                let r = loop_
                    .run_continue(
                        Some(&mut writer),
                        Some(&pricing_table),
                        Some(cancel.clone()),
                    )
                    .await;
                (r, loop_.turn_count(), loop_.cumulative_cost_usd)
            }
        }
        None => {
            let mut loop_ = AgentLoop::new(provider, registry, config, ask_user);
            let r = loop_
                .run(
                    &prompt,
                    Some(&mut writer),
                    Some(&pricing_table),
                    Some(cancel),
                )
                .await;
            (r, loop_.turn_count(), loop_.cumulative_cost_usd)
        }
    };
    let duration_ms = start.elapsed().as_millis() as u64;

    let exit_status = match &result {
        Ok(_) => "completed".to_string(),
        Err(_) => "error".to_string(),
    };

    if let Err(ref e) = result {
        if let ClidoError::Interrupted = e {
            let _ = writer.flush();
            eprintln!("Interrupted.");
            return Err(CliError::Interrupted("Interrupted by user.".into()).into());
        }
    }

    writer.write_line(&SessionLine::Result {
        exit_status: exit_status.clone(),
        total_cost_usd,
        num_turns,
        duration_ms,
    })?;

    match result {
        Ok(text) => {
            if cli.output_format == "json" {
                let out = serde_json::json!({
                    "schema_version": 1,
                    "type": "result",
                    "exit_status": exit_status.as_str(),
                    "result": text,
                    "session_id": session_id,
                    "num_turns": num_turns,
                    "duration_ms": duration_ms,
                    "total_cost_usd": total_cost_usd,
                    "is_error": false
                });
                println!("{}", serde_json::to_string(&out).unwrap());
            } else {
                println!("{}", text);
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            if cli.output_format == "json" {
                let out = serde_json::json!({
                    "schema_version": 1,
                    "type": "result",
                    "exit_status": "error",
                    "result": msg,
                    "session_id": session_id,
                    "num_turns": num_turns,
                    "duration_ms": duration_ms,
                    "total_cost_usd": total_cost_usd,
                    "is_error": true
                });
                println!("{}", serde_json::to_string(&out).unwrap());
            }
            if matches!(
                &e,
                ClidoError::BudgetExceeded | ClidoError::MaxTurnsExceeded
            ) {
                Err(CliError::SoftLimit(msg).into())
            } else {
                Err(e.into())
            }
        }
    }
}

async fn run_doctor() -> Result<(), anyhow::Error> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(&cwd).map_err(|e| CliError::DoctorMandatory(e.to_string()))?;
    let profile_name = loaded.default_profile.as_str();
    let profile = loaded
        .get_profile(profile_name)
        .map_err(|e| CliError::DoctorMandatory(e.to_string()))?;

    let mut mandatory = Vec::new();
    let mut warnings = Vec::new();

    let api_key_env = profile
        .api_key_env
        .as_deref()
        .unwrap_or("ANTHROPIC_API_KEY");
    if profile.provider != "local" {
        if env::var(api_key_env).is_err() {
            mandatory.push(format!(
                "API key not set for profile '{}' (set {}).",
                profile_name, api_key_env
            ));
        } else {
            println!(
                "✓ API key ({}) set for profile '{}'",
                api_key_env, profile_name
            );
        }
    }

    match session_dir_for_project(&cwd) {
        Ok(dir) => {
            if !dir.exists() {
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    mandatory.push(format!("Session dir not writable: {}", e));
                } else {
                    println!("✓ Session dir created and writable: {}", dir.display());
                }
            } else {
                let test_file = dir.join(".clido_doctor_write_test");
                if std::fs::write(&test_file, b"").is_ok() {
                    let _ = std::fs::remove_file(&test_file);
                    println!("✓ Session dir writable: {}", dir.display());
                } else {
                    mandatory.push(format!("Session dir not writable: {}", dir.display()));
                }
            }
        }
        Err(e) => {
            mandatory.push(format!("Session dir: {}", e));
        }
    }

    let (pricing_table, pricing_path) = load_pricing();
    if let Some(path) = &pricing_path {
        println!("✓ pricing.toml present: {}", path.display());
        if pricing_table.models.is_empty() {
            warnings.push(
                "pricing.toml is empty or invalid; using default cost estimates.".to_string(),
            );
        }
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                if let (Ok(now), Ok(mod_dur)) = (
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH),
                    modified.duration_since(std::time::UNIX_EPOCH),
                ) {
                    let age_secs = now.as_secs().saturating_sub(mod_dur.as_secs());
                    if age_secs > 90 * 86400 {
                        warnings.push(
                            "pricing.toml is older than 90 days; consider updating.".to_string(),
                        );
                    }
                }
            }
        }
    } else {
        warnings.push("pricing.toml not found; using default cost estimates.".to_string());
    }

    if !mandatory.is_empty() {
        for m in &mandatory {
            eprintln!("✗ {}", m);
        }
        return Err(CliError::DoctorMandatory(mandatory.join(" ")).into());
    }
    if !warnings.is_empty() {
        for w in &warnings {
            eprintln!("⚠ {}", w);
        }
        return Err(CliError::DoctorWarnings(warnings.join(" ")).into());
    }
    Ok(())
}

async fn run_init() -> Result<(), anyhow::Error> {
    let dir = directories::ProjectDirs::from("", "", "clido")
        .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;
    let config_dir = dir.config_dir();
    std::fs::create_dir_all(config_dir)?;
    let config_path = config_dir.join("config.toml");
    let default_toml = r#"
default_profile = "default"

[profile.default]
provider = "anthropic"
model = "claude-3-5-sonnet-20241022"
api_key_env = "ANTHROPIC_API_KEY"
"#;
    std::fs::write(&config_path, default_toml.trim_start())?;
    println!(
        "Created {}. Set ANTHROPIC_API_KEY and run clido doctor.",
        config_path.display()
    );
    Ok(())
}

async fn run_sessions_list() -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let sessions = list_sessions(&cwd)?;
    if sessions.is_empty() {
        println!("No sessions yet. Run 'clido <prompt>' to start one.");
        return Ok(());
    }
    for s in sessions {
        let (head, tail) = if s.session_id.len() > 12 {
            (&s.session_id[..8], &s.session_id[s.session_id.len() - 4..])
        } else {
            (s.session_id.as_str(), "")
        };
        let short_id = format!("{}...{}", head, tail);
        println!(
            "{}  {}  turns: {}  cost: ${:.4}  {}",
            short_id, s.start_time, s.num_turns, s.total_cost_usd, s.preview
        );
    }
    Ok(())
}

fn is_stdin_tty() -> bool {
    io::stdin().is_terminal()
}

async fn run_sessions_show(id: &str) -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let lines = SessionReader::load(&cwd, id)?;
    for line in lines {
        println!("{}", serde_json::to_string(&line)?);
    }
    Ok(())
}
