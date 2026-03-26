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
use crate::sessions;
use crate::ui::{ansi, cli_use_color};

/// Handle a `/`-prefixed REPL slash command. Returns `true` if the loop should exit.
async fn handle_slash_command(cmd: &str, total_turns: u32, total_cost_usd: f64) -> bool {
    let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
    match parts[0] {
        "/help" => {
            eprintln!("REPL commands (not sent to the agent):");
            eprintln!("  /help              — show this list");
            eprintln!("  /cost              — current session cost and turn count");
            eprintln!("  /sessions          — list recent sessions");
            eprintln!(
                "  /resume <id>       — restart with an existing session (use --resume flag)"
            );
            eprintln!("  /mode plan         — reminder: restart with --permission-mode plan");
            eprintln!("  /mode agent        — reminder: restart without --permission-mode");
            eprintln!("  /exit  /quit       — end the REPL");
            eprintln!("  //...              — send a literal prompt starting with /");
        }
        "/cost" => {
            eprintln!(
                "Session: {} turns, ${:.4} total",
                total_turns, total_cost_usd
            );
        }
        "/sessions" => {
            if let Err(e) = sessions::run_sessions_list().await {
                eprintln!("Error listing sessions: {}", e);
            }
        }
        "/resume" => {
            let id = parts.get(1).map(|s| s.trim()).unwrap_or("");
            if id.is_empty() {
                eprintln!("Usage: /resume <session-id>");
                eprintln!("       Or restart with: clido --resume {}", id);
            } else {
                eprintln!(
                    "To resume session {}, restart the REPL with: clido --resume {}",
                    id, id
                );
            }
        }
        "/mode" => {
            let mode = parts.get(1).map(|s| s.trim()).unwrap_or("");
            match mode {
                "plan" => {
                    eprintln!("To switch to plan mode, restart with: clido --permission-mode plan")
                }
                "agent" => eprintln!(
                    "Currently in agent mode. Restart without --permission-mode to reset."
                ),
                _ => eprintln!("Usage: /mode plan | /mode agent"),
            }
        }
        "/exit" | "/quit" => return true,
        other => {
            eprintln!("Unknown REPL command: {}. Type /help for a list.", other);
        }
    }
    false
}

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

        // `//...` sends a literal prompt starting with `/`.
        let prompt = if line.starts_with("//") {
            &line[1..]
        } else if line.starts_with('/') {
            // Slash command — handle locally, do not send to agent.
            let exit = handle_slash_command(line, total_turns, total_cost_usd).await;
            if exit {
                break;
            }
            continue;
        } else {
            line
        };

        let result = if first_turn {
            loop_
                .run(
                    prompt,
                    Some(&mut writer),
                    Some(&setup.pricing_table),
                    Some(cancel.clone()),
                )
                .await
        } else {
            loop_
                .run_next_turn(
                    prompt,
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
