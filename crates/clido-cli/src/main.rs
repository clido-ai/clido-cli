//! Clido CLI entry point.

mod agent_setup;
mod audit_cmd;
mod cli;
mod config;
mod doctor;
mod errors;
mod index_cmd;
mod memory_cmd;
mod models;
mod pricing_cmd;
mod provider;
mod repl;
mod run;
mod sessions;
mod setup;
mod stats;
mod tui;
mod ui;
mod workflow;

use clap::{CommandFactory, Parser};
use clido_core::config_file_exists;
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
    tracing_subscriber::fmt().with_env_filter(filter).init();

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

async fn dispatch(cli: cli::Cli) -> Result<(), anyhow::Error> {
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
        Some(cli::Subcommand::Workflow { cmd }) => return workflow::run_workflow(cmd).await,
        Some(cli::Subcommand::ListModels { provider, json }) => {
            models::run_list_models(provider.as_deref(), *json);
            return Ok(());
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
            return audit_cmd::run_audit(*tail, session.as_deref(), tool.as_deref(), since.as_deref(), *json);
        }
        Some(cli::Subcommand::Memory { cmd }) => {
            return memory_cmd::run_memory(cmd);
        }
        Some(cli::Subcommand::FetchModels { provider, json }) => {
            models::run_list_models(provider.as_deref(), *json);
            return Ok(());
        }
        Some(cli::Subcommand::Index { cmd }) => {
            return index_cmd::run_index(cmd).await;
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

    if !config_file_exists(&workspace_root) {
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
