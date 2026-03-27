//! Clido CLI entry point.

mod agent_setup;
mod audit_cmd;
mod checkpoint_cmd;
mod cli;
mod commit;
mod config;
mod doctor;
mod errors;
mod git_context;
pub(crate) mod image_input;
mod index_cmd;
mod memory_cmd;
mod models;
mod notify;
mod plan_cmd;
mod pricing_cmd;
mod profiles;
mod provider;
mod repl;
mod run;
mod sessions;
mod setup;
mod spawn_tools;
mod stats;
mod tui;
mod ui;
mod workflow;

use clap::{CommandFactory, Parser};
use clido_core::{config_file_exists, load_config};
use std::env;
use std::io::{self, IsTerminal, Write};

use errors::CliError;
use ui::{ansi, cli_use_color};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Restore terminal to cooked mode before printing anything.
///
/// crossterm's `is_raw_mode_enabled()` only tracks the *current process* — it returns false
/// even when a previous crashed process left the terminal in raw mode. We use libc directly
/// to inspect and fix the real termios state on the stdin fd.
///
/// If ICANON is disabled (raw mode), we re-enable it plus ECHO, OPOST/ONLCR (LF→CRLF
/// translation) and ISIG so that normal CLI output is formatted correctly.
fn restore_terminal_if_needed() {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        use std::os::unix::io::AsRawFd;
        if !std::io::stdin().is_terminal() {
            return;
        }
        let fd = std::io::stdin().as_raw_fd();
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return;
            }
            // ICANON not set → raw mode active from a previous process.
            if t.c_lflag & libc::ICANON != 0 {
                return;
            }
            // Restore the flags that raw mode strips away.
            t.c_iflag |= (libc::ICRNL | libc::IXON) as libc::tcflag_t;
            t.c_oflag |= (libc::OPOST | libc::ONLCR) as libc::tcflag_t;
            t.c_lflag |=
                (libc::ICANON | libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ISIG | libc::IEXTEN)
                    as libc::tcflag_t;
            libc::tcsetattr(fd, libc::TCSAFLUSH, &t);
        }
    }
}

#[tokio::main]
async fn main() {
    // Must run before Cli::parse() — clap may print help/version and exit.
    restore_terminal_if_needed();
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

    // In TUI mode the alternate screen owns the terminal; writing to stderr
    // corrupts the display. Redirect logs to a file instead.
    let tui_mode = !cli.print
        && cli.prompt_str().is_empty()
        && sessions::is_stdin_tty()
        && cli.output_format == "text"
        && io::stdout().is_terminal();
    if tui_mode {
        if let Some(log_path) = directories::ProjectDirs::from("", "", "clido")
            .map(|d| d.config_dir().join("clido.log"))
        {
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .init();
            } else {
                tracing_subscriber::fmt()
                    .with_env_filter(tracing_subscriber::EnvFilter::new("off"))
                    .init();
            }
        } else {
            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new("off"))
                .init();
        }
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let exit = match dispatch(cli).await {
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

/// Returns true if a config exists AND the default profile's provider is fully usable
/// (API key is present or resolvable from environment). Returns false if either the
/// config is missing, has no profiles, or the API key cannot be resolved — all of
/// which mean the user should go through first-run setup.
fn config_ready_for_use(cli: &cli::Cli, workspace_root: &std::path::Path) -> bool {
    if !config_file_exists(workspace_root) {
        return false;
    }
    let Ok(loaded) = load_config(workspace_root) else {
        return false;
    };
    let profile_name = cli
        .profile
        .as_deref()
        .unwrap_or(loaded.default_profile.as_str());
    let Ok(profile) = loaded.get_profile(profile_name) else {
        return false;
    };
    provider::make_provider(
        profile_name,
        profile,
        cli.provider.as_deref(),
        cli.model.as_deref(),
    )
    .is_ok()
}

async fn dispatch(cli: cli::Cli) -> Result<(), anyhow::Error> {
    // --input-format stream-json is a V2 feature (SDK subprocess integration).
    // Error early so users get a clear message instead of silent ignore.
    if cli.input_format == "stream-json" {
        return Err(CliError::Usage(
            "--input-format stream-json is not yet supported in V1. Use V2 or later, or pipe a plain-text prompt via stdin.".into(),
        )
        .into());
    }

    match &cli.subcommand {
        Some(cli::Subcommand::Version) => {
            println!("clido {}", VERSION);
            return Ok(());
        }
        Some(cli::Subcommand::Sessions { cmd }) => match cmd {
            cli::SessionsCmd::List => return sessions::run_sessions_list().await,
            cli::SessionsCmd::Show { id } => return sessions::run_sessions_show(id).await,
            cli::SessionsCmd::Fork { id } => return sessions::run_sessions_fork(id).await,
        },
        Some(cli::Subcommand::Init) => return setup::run_init().await,
        Some(cli::Subcommand::Doctor) => return doctor::run_doctor().await,
        Some(cli::Subcommand::Config { cmd }) => return config::run_config(cmd).await,
        Some(cli::Subcommand::Workflow { cmd }) => return workflow::run_workflow(&cli, cmd).await,
        Some(cli::Subcommand::ListModels { provider, json }) => {
            return models::run_list_models(provider.as_deref(), *json).await;
        }
        Some(cli::Subcommand::UpdatePricing) => {
            pricing_cmd::run_update_pricing();
            return Ok(());
        }
        Some(cli::Subcommand::Completions { shell }) => {
            use clap_complete::{generate, shells};
            let mut cmd = cli::Cli::command();
            let shell_enum = shell.to_lowercase();
            match shell_enum.as_str() {
                "bash" => generate(shells::Bash, &mut cmd, "clido", &mut io::stdout()),
                "zsh" => generate(shells::Zsh, &mut cmd, "clido", &mut io::stdout()),
                "fish" => generate(shells::Fish, &mut cmd, "clido", &mut io::stdout()),
                "powershell" | "ps" => {
                    generate(shells::PowerShell, &mut cmd, "clido", &mut io::stdout())
                }
                "elvish" => generate(shells::Elvish, &mut cmd, "clido", &mut io::stdout()),
                _ => {
                    eprintln!(
                        "Unknown shell: {}. Use: bash, zsh, fish, powershell, elvish",
                        shell
                    );
                    std::process::exit(2);
                }
            }
            return Ok(());
        }
        Some(cli::Subcommand::Man) => {
            let cmd = cli::Cli::command();
            let man = clap_mangen::Man::new(cmd);
            let mut buf = Vec::new();
            man.render(&mut buf)?;
            io::stdout().write_all(&buf)?;
            return Ok(());
        }
        Some(cli::Subcommand::Stats { session, json }) => {
            return stats::run_stats(session.as_deref(), *json);
        }
        Some(cli::Subcommand::Audit {
            tail,
            session,
            tool,
            since,
            json,
        }) => {
            return audit_cmd::run_audit(
                *tail,
                session.as_deref(),
                tool.as_deref(),
                since.as_deref(),
                *json,
            );
        }
        Some(cli::Subcommand::Memory { cmd }) => {
            return memory_cmd::run_memory(cmd);
        }
        Some(cli::Subcommand::FetchModels { provider, json }) => {
            return models::run_list_models(provider.as_deref(), *json).await;
        }
        Some(cli::Subcommand::Index { cmd }) => {
            return index_cmd::run_index(cmd).await;
        }
        Some(cli::Subcommand::Checkpoint { cmd }) => {
            return checkpoint_cmd::run_checkpoint(cmd, &cli).await;
        }
        Some(cli::Subcommand::Rollback { id, session, yes }) => {
            return checkpoint_cmd::run_rollback(id.as_deref(), session.as_deref(), *yes, &cli)
                .await;
        }
        Some(cli::Subcommand::Plan { cmd }) => {
            return plan_cmd::run_plan(cmd, &cli).await;
        }
        Some(cli::Subcommand::Profile { cmd }) => {
            return profiles::run_profile(cmd).await;
        }
        Some(cli::Subcommand::ListSessions) => {
            eprintln!(
                "Warning: 'clido list-sessions' is deprecated. Use 'clido sessions list' instead."
            );
            return sessions::run_sessions_list().await;
        }
        Some(cli::Subcommand::ShowSession { id }) => {
            eprintln!(
                "Warning: 'clido show-session' is deprecated. Use 'clido sessions show' instead."
            );
            return sessions::run_sessions_show(id).await;
        }
        Some(cli::Subcommand::Commit { yes, dry_run }) => {
            let workspace_root = cli.workdir.clone().unwrap_or_else(|| {
                env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
            return commit::run_commit(&workspace_root, *yes, *dry_run, &cli).await;
        }
        Some(cli::Subcommand::Run { prompt }) => {
            let prompt_str = prompt.join(" ").trim().to_string();
            if prompt_str.is_empty() {
                return Err(CliError::Usage(
                    "run requires a prompt. Usage: clido run <prompt>".into(),
                )
                .into());
            }
            let mut run_cli = cli.clone();
            run_cli.subcommand = None;
            run_cli.prompt = prompt.clone();
            return run::run_agent(run_cli).await;
        }
        None => {}
    }

    // Check for "help" before touching config — trailing_var_arg captures it as a prompt word.
    if cli.prompt_str().eq_ignore_ascii_case("help") {
        cli::Cli::command().print_help()?;
        println!();
        return Ok(());
    }

    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));

    if !config_ready_for_use(&cli, &workspace_root) {
        if sessions::is_stdin_tty() {
            setup::run_first_run_setup().await?;
        } else {
            return Err(CliError::Config(
                "No configuration found. Run 'clido init' to set up Clido.".into(),
            )
            .into());
        }
    }

    let mut prompt = cli.prompt_str();
    if prompt.is_empty() {
        if cli.print {
            return Err(CliError::Usage(
                "No prompt provided. Pass a prompt as an argument or pipe it via stdin.".into(),
            )
            .into());
        }
        if sessions::is_stdin_tty() {
            if cli.output_format == "text" && io::stdout().is_terminal() {
                return tui::run_tui(cli).await;
            }
            return repl::run_repl(cli).await;
        }
        use std::io::Read;
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

    // Re-create cli with the resolved prompt for run_agent
    run::run_agent(cli).await
}
