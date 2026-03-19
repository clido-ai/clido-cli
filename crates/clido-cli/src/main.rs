//! Clido CLI: run agent, sessions, version, init.

mod cli;

use async_trait::async_trait;
use clap::Parser;
use clido_agent::{session_lines_to_messages, AgentLoop, AskUser};
use clido_core::{
    agent_config_from_loaded, config_file_exists, load_config, load_pricing, ClidoError,
    LoadedConfig, PermissionMode, ProfileEntry, Result as CoreResult,
};
use clido_providers::{build_provider, ModelProvider};
use clido_storage::{
    list_sessions, session_dir_for_project, stale_paths, workflow_run_path, SessionLine,
    SessionReader, SessionWriter,
};
use clido_tools::default_registry;
use clido_workflows::{
    load as load_workflow, run_workflow as run_workflow_exec, validate as validate_workflow,
    StepRunRequest, StepRunResult, WorkflowContext, WorkflowStepRunner,
};
use inquire::{Confirm, Select, Text};
use std::env;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Inner width of the setup box (so right border aligns).
const SETUP_BOX_WIDTH: usize = 59;

/// Build the rich setup box with exactly SETUP_BOX_WIDTH chars per line so borders align (ux-requirements §2.2).
fn setup_banner_rich() -> String {
    let pad = |s: &str| {
        let n = s.chars().count();
        format!("{}{}", s, " ".repeat(SETUP_BOX_WIDTH.saturating_sub(n)))
    };
    let wrap = |s: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut cur_len = 0usize;
        for word in s.split_whitespace() {
            let wl = word.chars().count();
            let sep = if cur.is_empty() { 0 } else { 1 };
            if cur_len + sep + wl > SETUP_BOX_WIDTH {
                out.push(cur);
                cur = word.to_string();
                cur_len = wl;
            } else {
                if !cur.is_empty() {
                    cur.push(' ');
                    cur_len += 1;
                }
                cur.push_str(word);
                cur_len += wl;
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    };
    let mut lines = vec![
        "  Clido setup".to_string(),
        "  Choose a provider and where to store your API key.".to_string(),
    ];
    lines.extend(wrap(
        "  Answer the questions below; use arrow keys or type, then Enter.",
    ));
    lines.push("  Defaults are in brackets.".to_string());
    let body = lines
        .iter()
        .map(|l| format!("║{}║", pad(l)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "╔{}╗\n{}\n╚{}╝",
        "═".repeat(SETUP_BOX_WIDTH),
        body,
        "═".repeat(SETUP_BOX_WIDTH),
    )
}

/// ASCII fallback when not a TTY or narrow (ux-requirements §2.2).
const SETUP_BANNER_ASCII: &str = "  --- Clido setup ---\n  Answer each question: type your choice, then press Enter. Defaults in [brackets].";

/// True if we should show the rich setup UI (box, welcome line). Use stdin OR stderr TTY so the banner shows whenever the user is at a terminal.
fn setup_use_rich_ui() -> bool {
    io::stdin().is_terminal() || io::stderr().is_terminal()
}

/// Use color for CLI output when any standard stream is a TTY and NO_COLOR is not set (CLI spec §5, ux-requirements §7.3).
/// Used for agent banner, REPL prompt, doctor, errors, first-run, permission prompt, deprecation warnings.
fn cli_use_color() -> bool {
    (io::stdin().is_terminal() || io::stderr().is_terminal() || io::stdout().is_terminal())
        && env::var("NO_COLOR").is_err()
}

/// Use color only in setup flow (alias for consistency with ux-requirements: stdin or stderr TTY).
fn setup_use_color() -> bool {
    (io::stdin().is_terminal() || io::stderr().is_terminal()) && env::var("NO_COLOR").is_err()
}

/// ANSI codes for CLI UI (only when cli_use_color() or setup_use_color()). ux-requirements §7.3: color supports, does not replace, text.
mod ansi {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m"; // hints, prompt
    pub const CYAN: &str = "\x1b[36m"; // box / accent / banner
    pub const BRIGHT_CYAN: &str = "\x1b[96m"; // title
    pub const GREEN: &str = "\x1b[32m"; // success
    pub const YELLOW: &str = "\x1b[33m"; // warnings
    pub const RED: &str = "\x1b[31m"; // errors
}

/// Build provider from profile; resolves API key from env and calls build_provider.
fn make_provider(
    profile_name: &str,
    profile: &ProfileEntry,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<Arc<dyn ModelProvider>, String> {
    let provider_name = provider_override.unwrap_or(profile.provider.as_str());
    let api_key_env = profile
        .api_key_env
        .as_deref()
        .unwrap_or("ANTHROPIC_API_KEY");
    let api_key = env::var(api_key_env).map_err(|_| {
        format!(
            "API key not found for profile '{}'. Set {} in your environment. Run: clido doctor to check all configuration.",
            profile_name, api_key_env
        )
    })?;
    let model = model_override.unwrap_or(&profile.model).to_string();
    build_provider(provider_name, api_key, model, profile.base_url.as_deref())
        .map_err(|e| e.to_string())
}

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

/// CLI exit codes per spec: 0 success, 1 runtime, 2 usage/config, 3 soft limit.
/// Doctor: 0 all pass, 1 mandatory failure, 2 warnings only.
#[derive(Error, Debug)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    /// Config-related error; display with "Error [Config]: {}" per CLI spec.
    #[error("{0}")]
    Config(String),
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
            CliError::Config(_) => 2,
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

    // Global startup banner: shown on every interactive text startup, regardless of subcommand.
    if cli.output_format == "text" && io::stdout().is_terminal() {
        if cli_use_color() {
            print!("{}", ansi::CYAN);
        }
        print!("{}", BANNER);
        if cli_use_color() {
            print!("{}", ansi::RESET);
        }
        let _ = io::stdout().flush();
    }

    let exit = match run(cli).await {
        Ok(()) => 0,
        Err(e) => {
            if let Some(cli_err) = e.downcast_ref::<CliError>() {
                let code = cli_err.exit_code();
                if cli_use_color() {
                    match cli_err {
                        CliError::Config(msg) => {
                            eprintln!("{}Error [Config]: {}{}", ansi::RED, msg, ansi::RESET)
                        }
                        _ => eprintln!("{}Error: {}{}", ansi::RED, e, ansi::RESET),
                    }
                } else {
                    match cli_err {
                        CliError::Config(msg) => eprintln!("Error [Config]: {}", msg),
                        _ => eprintln!("Error: {}", e),
                    }
                }
                code
            } else {
                if cli_use_color() {
                    eprintln!("{}Error: {}{}", ansi::RED, e, ansi::RESET);
                } else {
                    eprintln!("Error: {}", e);
                }
                1
            }
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
            if cli_use_color() {
                eprintln!(
                    "{}Warning: 'clido list-sessions' is deprecated. Use 'clido sessions list' instead.{}",
                    ansi::YELLOW, ansi::RESET
                );
            } else {
                eprintln!(
                    "Warning: 'clido list-sessions' is deprecated. Use 'clido sessions list' instead."
                );
            }
            return run_sessions_list().await;
        }
        Some(cli::Subcommand::ShowSession { id }) => {
            if cli_use_color() {
                eprintln!("{}Warning: 'clido show-session' is deprecated. Use 'clido sessions show <id>' instead.{}", ansi::YELLOW, ansi::RESET);
            } else {
                eprintln!("Warning: 'clido show-session' is deprecated. Use 'clido sessions show <id>' instead.");
            }
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
        Some(cli::Subcommand::Workflow { cmd }) => {
            return run_workflow(cmd).await;
        }
        None => {}
    }

    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // First-run: no config file and we are about to run the agent (not init/doctor/version/sessions).
    if !config_file_exists(&workspace_root) {
        if is_stdin_tty() {
            run_first_run_setup().await?;
        } else {
            return Err(CliError::Config(
                "No configuration found. Run 'clido init' to set up Clido.".into(),
            )
            .into());
        }
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
        if is_stdin_tty() {
            return run_repl(cli).await;
        }
        let mut stdin = String::new();
        io::stdin().read_to_string(&mut stdin)?;
        prompt = stdin.trim().to_string();
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

    let provider = make_provider(
        profile_name,
        profile,
        cli.provider.as_deref(),
        cli.model.as_deref(),
    )
    .map_err(CliError::Usage)?;

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

    if let Err(ClidoError::Interrupted) = &result {
        let _ = writer.flush();
        if cli_use_color() {
            eprintln!("{}Interrupted.{}", ansi::DIM, ansi::RESET);
        } else {
            eprintln!("Interrupted.");
        }
        return Err(CliError::Interrupted("Interrupted by user.".into()).into());
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

/// Interactive REPL: no prompt, TTY → loop with "clido> " prompt; exit on empty, "exit", "quit", or Ctrl-C.
async fn run_repl(cli: cli::Cli) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

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

    let provider = make_provider(
        profile_name,
        profile,
        cli.provider.as_deref(),
        cli.model.as_deref(),
    )
    .map_err(CliError::Usage)?;

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
        return Err(CliError::Usage("No tools left after filters.".into()).into());
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

    let session_id = uuid::Uuid::new_v4().to_string();
    let mut writer = SessionWriter::create(&workspace_root, &session_id)?;

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_handle = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel_handle.store(true, Ordering::Relaxed);
    });

    let mut loop_ = AgentLoop::new(provider, registry, config, ask_user);
    let mut first_turn = true;
    let mut total_turns: u32 = 0;
    let mut total_cost_usd: f64 = 0.0;

    loop {
        if cli_use_color() {
            eprint!("{}clido> {}", ansi::DIM, ansi::RESET);
        } else {
            eprint!("clido> ");
        }
        let _ = io::stderr().flush();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            break;
        }
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("exit") || line.eq_ignore_ascii_case("quit")
        {
            break;
        }

        let result = if first_turn {
            loop_
                .run(
                    line,
                    Some(&mut writer),
                    Some(&pricing_table),
                    Some(cancel.clone()),
                )
                .await
        } else {
            loop_
                .run_next_turn(
                    line,
                    Some(&mut writer),
                    Some(&pricing_table),
                    Some(cancel.clone()),
                )
                .await
        };

        total_turns += loop_.turn_count();
        total_cost_usd += loop_.cumulative_cost_usd;

        if let Err(ClidoError::Interrupted) = &result {
            let _ = writer.flush();
            eprintln!("Interrupted.");
            return Err(CliError::Interrupted("Interrupted by user.".into()).into());
        }

        match result {
            Ok(text) => {
                if cli.output_format == "json" {
                    let out = serde_json::json!({
                        "schema_version": 1,
                        "type": "repl_turn",
                        "result": text,
                        "session_id": session_id,
                    });
                    println!("{}", serde_json::to_string(&out).unwrap());
                } else {
                    println!("{}", text);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if cli.output_format == "json" {
                    let out = serde_json::json!({
                        "schema_version": 1,
                        "type": "repl_turn",
                        "is_error": true,
                        "result": msg,
                    });
                    println!("{}", serde_json::to_string(&out).unwrap());
                } else {
                    eprintln!("Error: {}", msg);
                }
            }
        }

        first_turn = false;
    }

    let _ = writer.write_line(&SessionLine::Result {
        exit_status: "completed".to_string(),
        total_cost_usd,
        num_turns: total_turns,
        duration_ms: 0,
    });
    let _ = writer.flush();
    Ok(())
}

async fn run_workflow(cmd: &cli::WorkflowCmd) -> Result<(), anyhow::Error> {
    match cmd {
        cli::WorkflowCmd::Run {
            workflow,
            input,
            dry_run,
            yes: _,
        } => run_workflow_run(workflow, input, *dry_run).await,
        cli::WorkflowCmd::Validate { path } => run_workflow_validate(path).await,
        cli::WorkflowCmd::Inspect { path } => run_workflow_inspect(path).await,
        cli::WorkflowCmd::List => run_workflow_list().await,
    }
}

/// Step runner that uses AgentLoop (load config, build provider/registry per step).
struct CliWorkflowRunner {
    workspace_root: std::path::PathBuf,
    run_id: String,
}

#[async_trait]
impl WorkflowStepRunner for CliWorkflowRunner {
    async fn run_step(&self, request: StepRunRequest) -> CoreResult<StepRunResult> {
        let loaded =
            load_config(&self.workspace_root).map_err(|e| ClidoError::Workflow(e.to_string()))?;
        let (pricing_table, _) = load_pricing();
        let profile_name = request
            .profile
            .as_deref()
            .unwrap_or(loaded.default_profile.as_str());
        let profile = loaded
            .get_profile(profile_name)
            .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        LoadedConfig::validate_provider(&profile.provider)
            .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        let provider =
            make_provider(profile_name, profile, None, None).map_err(ClidoError::Workflow)?;
        let model = profile.model.clone();
        let mut registry = default_registry(self.workspace_root.clone());
        registry = registry.with_filters(request.tools, None);
        if registry.schemas().is_empty() {
            return Err(ClidoError::Workflow(
                "No tools available for step".to_string(),
            ));
        }
        let system_prompt = request
            .system_prompt_override
            .unwrap_or_else(|| "You are a helpful coding assistant.".to_string());
        let permission_mode = PermissionMode::Default;
        let mut config = agent_config_from_loaded(
            &loaded,
            profile_name,
            request.max_turns_override,
            None,
            Some(model),
            Some(system_prompt),
            Some(permission_mode),
        )
        .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        if config.max_context_tokens.is_none() {
            if let Some(entry) = pricing_table.models.get(&config.model) {
                if let Some(cw) = entry.context_window {
                    config.max_context_tokens = Some(cw);
                }
            }
        }
        let session_id = format!("{}_{}", self.run_id, request.step_id);
        let mut writer = SessionWriter::create(&self.workspace_root, &session_id)
            .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        let mut loop_ = AgentLoop::new(provider, registry, config, None);
        let start = std::time::Instant::now();
        let result = loop_
            .run(
                &request.rendered_prompt,
                Some(&mut writer),
                Some(&pricing_table),
                None,
            )
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;
        let _ = writer.flush();
        match result {
            Ok(text) => Ok(StepRunResult {
                output_text: text,
                cost_usd: loop_.cumulative_cost_usd,
                duration_ms,
                error: None,
            }),
            Err(e) => Ok(StepRunResult {
                output_text: String::new(),
                cost_usd: loop_.cumulative_cost_usd,
                duration_ms,
                error: Some(e.to_string()),
            }),
        }
    }
}

async fn run_workflow_run(
    workflow: &str,
    input: &[String],
    dry_run: bool,
) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(&workspace_root).map_err(|e| CliError::Usage(e.to_string()))?;
    let path = resolve_workflow_path(workflow, &workspace_root, &loaded.workflows.directory)?;
    let def = load_workflow(&path).map_err(|e| CliError::Usage(e.to_string()))?;
    validate_workflow(&def).map_err(|e| CliError::Usage(e.to_string()))?;
    let overrides: Vec<(String, serde_json::Value)> = input
        .iter()
        .filter_map(|s| {
            let (k, v) = s.split_once('=')?;
            Some((
                k.trim().to_string(),
                serde_json::Value::String(v.trim().to_string()),
            ))
        })
        .collect();
    let inputs = WorkflowContext::resolve_inputs(&def, &overrides)
        .map_err(|e| CliError::Usage(e.to_string()))?;
    let mut context = WorkflowContext::new(inputs);
    if dry_run {
        for step in &def.steps {
            let prompt = clido_workflows::render(&step.prompt, &context)
                .map_err(|e| CliError::Usage(e.to_string()))?;
            println!("Step {}: rendered prompt ({} chars)", step.id, prompt.len());
            println!("---\n{}\n---", prompt);
        }
        return Ok(());
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    let runner = CliWorkflowRunner {
        workspace_root: workspace_root.clone(),
        run_id: run_id.clone(),
    };
    let audit_path = workflow_run_path(&def.name, &run_id).ok();
    let summary = run_workflow_exec(&def, &mut context, &runner, audit_path.as_deref()).await?;
    if cli_use_color() {
        println!(
            "{}Workflow completed: {} steps, ${:.4} total, {} ms{}",
            ansi::GREEN,
            summary.step_count,
            summary.total_cost_usd,
            summary.total_duration_ms,
            ansi::RESET
        );
    } else {
        println!(
            "Workflow completed: {} steps, ${:.4} total, {} ms",
            summary.step_count, summary.total_cost_usd, summary.total_duration_ms
        );
    }
    Ok(())
}

fn resolve_workflow_path(
    workflow: &str,
    workspace_root: &Path,
    project_workflow_dir: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let p = Path::new(workflow);
    if p.is_absolute() || p.exists() {
        return Ok(p.to_path_buf());
    }
    let project_base = workspace_root.join(project_workflow_dir);
    for candidate in [
        workflow,
        &format!("{}.yaml", workflow),
        &format!("{}.yml", workflow),
    ] {
        let path = project_base.join(candidate);
        if path.exists() {
            return Ok(path);
        }
    }
    if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") {
        let global_base = dirs.config_dir().join("workflows");
        for candidate in [
            workflow,
            &format!("{}.yaml", workflow),
            &format!("{}.yml", workflow),
        ] {
            let path = global_base.join(candidate);
            if path.exists() {
                return Ok(path);
            }
        }
    }
    Err(anyhow::anyhow!(
        "Workflow not found: {} (tried current path, {}, ~/.config/clido/workflows/)",
        workflow,
        project_workflow_dir
    ))
}

async fn run_workflow_validate(path: &Path) -> Result<(), anyhow::Error> {
    let def = load_workflow(path).map_err(|e| CliError::Usage(e.to_string()))?;
    validate_workflow(&def).map_err(|e| CliError::Usage(e.to_string()))?;
    println!("Valid: {}", path.display());
    Ok(())
}

async fn run_workflow_inspect(path: &Path) -> Result<(), anyhow::Error> {
    let def = load_workflow(path).map_err(|e| CliError::Usage(e.to_string()))?;
    println!("Workflow: {} (version {})", def.name, def.version);
    for (i, step) in def.steps.iter().enumerate() {
        let parallel = if step.parallel { " [parallel]" } else { "" };
        println!("  {}. {}{}", i + 1, step.id, parallel);
    }
    Ok(())
}

async fn run_workflow_list() -> Result<(), anyhow::Error> {
    let mut found = Vec::new();
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let workflow_dir = load_config(&cwd)
        .map(|l| l.workflows.directory)
        .unwrap_or_else(|_| ".clido/workflows".to_string());
    let project_dir = cwd.join(&workflow_dir);
    if project_dir.exists() {
        if let Ok(rd) = std::fs::read_dir(&project_dir) {
            for e in rd.flatten() {
                let path = e.path();
                if path.extension().map(|x| x == "yaml").unwrap_or(false)
                    || path.extension().map(|x| x == "yml").unwrap_or(false)
                {
                    found.push(path);
                }
            }
        }
    }
    if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") {
        let global = dirs.config_dir().join("workflows");
        if global.exists() {
            if let Ok(rd) = std::fs::read_dir(&global) {
                for e in rd.flatten() {
                    let path = e.path();
                    if path.extension().map(|x| x == "yaml").unwrap_or(false)
                        || path.extension().map(|x| x == "yml").unwrap_or(false)
                    {
                        found.push(path);
                    }
                }
            }
        }
    }
    for path in found {
        if let Ok(def) = load_workflow(&path) {
            println!("  {}  {}", def.name, path.display());
        }
    }
    Ok(())
}

/// Interactive setup flow (CLI spec §4): ask provider, API key/env or base URL, write config.
/// Returns (config_path, toml_content). Runs in blocking thread for stdin reads.
/// When stderr is a TTY, prints rich banner (ux-requirements §2.2); otherwise ASCII.
fn run_interactive_setup_blocking(
    init_subline: Option<&str>,
) -> Result<(std::path::PathBuf, String), anyhow::Error> {
    let config_path = if let Ok(p) = env::var("CLIDO_CONFIG") {
        std::path::PathBuf::from(p)
    } else {
        let dir = directories::ProjectDirs::from("", "", "clido")
            .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;
        dir.config_dir().join("config.toml")
    };

    let mut stdin = io::stdin().lock();
    let mut line = String::new();

    let use_color = setup_use_color();
    let rich = setup_use_rich_ui();

    if rich {
        // Production-grade setup UI: welcome line, spacing, bordered box, then prompts.
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

    let choice: u8 = if rich {
        // Arrow-key selection (inquire) for production-grade UX.
        let options: Vec<&str> = vec![
            "Anthropic (cloud) — requires API key",
            "Local (Ollama) — no key; use http://localhost:11434",
        ];
        let selected: &str = Select::new("Provider:", options)
            .with_starting_cursor(0)
            .prompt()
            .map_err(|e| anyhow::anyhow!("prompt: {}", e))?;
        if selected.contains("Local") {
            2
        } else {
            1
        }
    } else {
        eprintln!("  Provider:");
        eprintln!("    1) Anthropic (cloud) — requires API key");
        eprintln!("    2) Local (Ollama)    — no key; use http://localhost:11434");
        eprintln!("  Type 1 or 2, then press Enter [default: 1]:");
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        line.trim().parse().unwrap_or(1)
    };

    let toml = if choice == 2 {
        let base_url = if rich {
            Text::new("Ollama base URL")
                .with_default("http://localhost:11434")
                .prompt()
                .map_err(|e| anyhow::anyhow!("prompt: {}", e))?
                .trim()
                .to_string()
        } else {
            eprintln!("  Ollama base URL (press Enter for http://localhost:11434):");
            line.clear();
            stdin
                .read_line(&mut line)
                .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
            let base = line.trim();
            if base.is_empty() {
                "http://localhost:11434"
            } else {
                base
            }
            .to_string()
        };
        let base_url = if base_url.is_empty() {
            "http://localhost:11434"
        } else {
            base_url.as_str()
        };
        format!(
            r#"default_profile = "default"

[profile.default]
provider = "local"
model = "codellama"
base_url = "{}"
"#,
            base_url
        )
    } else {
        let use_env = if rich {
            Confirm::new("Use existing ANTHROPIC_API_KEY from your environment?")
                .with_default(true)
                .prompt()
                .map_err(|e| anyhow::anyhow!("prompt: {}", e))?
        } else {
            eprintln!("  Use existing ANTHROPIC_API_KEY from your environment? [Y/n]:");
            line.clear();
            stdin
                .read_line(&mut line)
                .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
            line.trim().is_empty()
                || line.trim().eq_ignore_ascii_case("y")
                || line.trim().eq_ignore_ascii_case("yes")
        };
        if !use_env {
            if use_color {
                eprintln!(
                    "{}  Set your key in the environment, then run Clido again:{}",
                    ansi::DIM,
                    ansi::RESET
                );
                eprintln!(
                    "{}    export ANTHROPIC_API_KEY='your-key-here'{}",
                    ansi::DIM,
                    ansi::RESET
                );
                eprintln!(
                    "{}  Or add it to your shell profile. Then run: clido doctor{}",
                    ansi::DIM,
                    ansi::RESET
                );
            } else {
                eprintln!("  Set your key in the environment, then run Clido again:");
                eprintln!("    export ANTHROPIC_API_KEY='your-key-here'");
                eprintln!("  Or add it to your shell profile. Then run: clido doctor");
            }
        }
        r#"default_profile = "default"

[profile.default]
provider = "anthropic"
model = "claude-3-5-sonnet-20241022"
api_key_env = "ANTHROPIC_API_KEY"
"#
        .to_string()
    };

    Ok((config_path, toml))
}

/// First-run interactive setup: no config file and TTY → run interactive setup, write config, then continue.
async fn run_first_run_setup() -> Result<(), anyhow::Error> {
    if cli_use_color() {
        eprintln!(
            "{}No configuration found. Running first-time setup.{}",
            ansi::DIM,
            ansi::RESET
        );
    } else {
        eprintln!("No configuration found. Running first-time setup.");
    }
    let (config_path, toml) = tokio::task::spawn_blocking(|| run_interactive_setup_blocking(None))
        .await
        .map_err(|e| anyhow::anyhow!("setup: {}", e))??;
    let parent = config_path
        .parent()
        .ok_or_else(|| CliError::Usage("Invalid config path.".into()))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&config_path, toml.trim_start())?;
    if setup_use_color() {
        eprintln!(
            "{}  Created {}. Run 'clido doctor' to verify.{}",
            ansi::GREEN,
            config_path.display(),
            ansi::RESET
        );
    } else {
        eprintln!(
            "  Created {}. Run 'clido doctor' to verify.",
            config_path.display()
        );
    }
    Ok(())
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
    let use_color = cli_use_color();
    if profile.provider != "local" {
        if env::var(api_key_env).is_err() {
            mandatory.push(format!(
                "API key not set for profile '{}' (set {}).",
                profile_name, api_key_env
            ));
        } else {
            if use_color {
                println!(
                    "{}✓ API key ({}) set for profile '{}'{}",
                    ansi::GREEN,
                    api_key_env,
                    profile_name,
                    ansi::RESET
                );
            } else {
                println!(
                    "✓ API key ({}) set for profile '{}'",
                    api_key_env, profile_name
                );
            }
        }
    }

    match session_dir_for_project(&cwd) {
        Ok(dir) => {
            if !dir.exists() {
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    mandatory.push(format!("Session dir not writable: {}", e));
                } else {
                    if use_color {
                        println!(
                            "{}✓ Session dir created and writable: {}{}",
                            ansi::GREEN,
                            dir.display(),
                            ansi::RESET
                        );
                    } else {
                        println!("✓ Session dir created and writable: {}", dir.display());
                    }
                }
            } else {
                let test_file = dir.join(".clido_doctor_write_test");
                if std::fs::write(&test_file, b"").is_ok() {
                    let _ = std::fs::remove_file(&test_file);
                    if use_color {
                        println!(
                            "{}✓ Session dir writable: {}{}",
                            ansi::GREEN,
                            dir.display(),
                            ansi::RESET
                        );
                    } else {
                        println!("✓ Session dir writable: {}", dir.display());
                    }
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
        if use_color {
            println!(
                "{}✓ pricing.toml present: {}{}",
                ansi::GREEN,
                path.display(),
                ansi::RESET
            );
        } else {
            println!("✓ pricing.toml present: {}", path.display());
        }
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
            if use_color {
                eprintln!("{}✗ {}{}", ansi::RED, m, ansi::RESET);
            } else {
                eprintln!("✗ {}", m);
            }
        }
        return Err(CliError::DoctorMandatory(mandatory.join(" ")).into());
    }
    if !warnings.is_empty() {
        for w in &warnings {
            if use_color {
                eprintln!("{}⚠ {}{}", ansi::YELLOW, w, ansi::RESET);
            } else {
                eprintln!("⚠ {}", w);
            }
        }
        return Err(CliError::DoctorWarnings(warnings.join(" ")).into());
    }
    Ok(())
}

async fn run_init() -> Result<(), anyhow::Error> {
    let (config_path, toml) = tokio::task::spawn_blocking(|| {
        run_interactive_setup_blocking(Some(
            "  Re-run 'clido init' anytime to change provider or reset config.",
        ))
    })
    .await
    .map_err(|e| anyhow::anyhow!("init: {}", e))??;
    let parent = config_path
        .parent()
        .ok_or_else(|| CliError::Usage("Invalid config path.".into()))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&config_path, toml.trim_start())?;
    if setup_use_color() {
        println!(
            "{}  Created {}. Run 'clido doctor' to verify.{}",
            ansi::GREEN,
            config_path.display(),
            ansi::RESET
        );
    } else {
        println!(
            "  Created {}. Run 'clido doctor' to verify.",
            config_path.display()
        );
    }
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
