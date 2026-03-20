//! Clido CLI entry point.

mod agent_setup;
mod cli;
mod config;
mod doctor;
mod errors;
mod provider;
mod repl;
mod run;
mod sessions;
mod setup;
mod tui;
mod ui;
mod workflow;

use clap::{CommandFactory, Parser};
use clido_core::config_file_exists;
use std::env;
use std::io::{self, IsTerminal};

use errors::CliError;
use ui::{ansi, cli_use_color};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Restore terminal to cooked mode before printing anything.
///
/// `crossterm::terminal::is_raw_mode_enabled()` only tracks the current process — it returns
/// false even when a previous crashed process left the terminal in raw mode. So we can't rely
/// on it for detection. Instead we always run `stty sane` when stdin is a TTY: it is
/// idempotent (no-op when already in cooked mode) and takes ~1 ms.
fn restore_terminal_if_needed() {
    if !std::io::stdin().is_terminal() {
        return;
    }
    let _ = std::process::Command::new("stty")
        .arg("sane")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
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
        },
        Some(cli::Subcommand::Init) => return setup::run_init().await,
        Some(cli::Subcommand::Doctor) => return doctor::run_doctor().await,
        Some(cli::Subcommand::Config { cmd }) => return config::run_config(cmd).await,
        Some(cli::Subcommand::Workflow { cmd }) => return workflow::run_workflow(cmd).await,
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
