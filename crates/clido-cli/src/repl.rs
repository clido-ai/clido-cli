//! Interactive REPL: no prompt + TTY → multi-turn conversation loop.

use clido_agent::AgentLoop;
use clido_core::ClidoError;
use clido_storage::{SessionLine, SessionWriter};
use std::env;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::agent_setup::AgentSetup;
use crate::cli::Cli;
use crate::errors::CliError;
use crate::ui::{ansi, cli_use_color};

pub async fn run_repl(cli: Cli) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let setup = AgentSetup::build(&cli, &workspace_root)?;

    let session_id = uuid::Uuid::new_v4().to_string();
    let mut writer = SessionWriter::create(&workspace_root, &session_id)?;

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_handle = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel_handle.store(true, Ordering::Relaxed);
    });

    let mut loop_ = AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user);
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
                    Some(&setup.pricing_table),
                    Some(cancel.clone()),
                )
                .await
        } else {
            loop_
                .run_next_turn(
                    line,
                    Some(&mut writer),
                    Some(&setup.pricing_table),
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
