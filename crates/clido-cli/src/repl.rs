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

/// Parse `@agent` mention routing from a prompt.
///
/// Returns `(agent_hint, cleaned_prompt)` where `agent_hint` is `Some("worker")` etc.
/// and `cleaned_prompt` has the `@agent` prefix stripped.
///
/// Supported: `@worker`, `@reviewer`, `@explore`, `@general`.
/// Unknown `@...` tokens are left in the prompt unchanged.
fn parse_agent_mention(prompt: &str) -> (Option<&'static str>, String) {
    let trimmed = prompt.trim_start();
    // Must start with @ and have at least one more char.
    if !trimmed.starts_with('@') {
        return (None, prompt.to_string());
    }
    let rest = &trimmed[1..];
    // Extract the agent name (alpha/underscore/digit until whitespace).
    let name_end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    let name = &rest[..name_end];
    let remainder = rest[name_end..].trim_start().to_string();

    match name.to_lowercase().as_str() {
        "worker" | "w" => (Some("worker"), remainder),
        "reviewer" | "r" => (Some("reviewer"), remainder),
        "explore" | "e" => (Some("explore"), remainder),
        "general" | "g" => (Some("general"), remainder),
        _ => (None, prompt.to_string()),
    }
}

/// Build an agent-routing prefix that tells the main agent to immediately delegate
/// to the specified sub-agent tool.
fn route_to_agent(agent: &str, prompt: &str) -> String {
    match agent {
        "worker" => format!(
            "ROUTING: Call SpawnWorker immediately with the following task. \
             Pass ALL necessary context in the call. Do not do any work yourself first.\n\
             Task: {prompt}"
        ),
        "reviewer" => format!(
            "ROUTING: Call SpawnReviewer immediately to review the following. \
             Include the relevant code as `output` in the call.\n\
             Review request: {prompt}"
        ),
        // explore and general → just prepend a light instruction
        "explore" => {
            format!("[Explore mode] Focus on reading/searching the codebase to answer: {prompt}")
        }
        _ => prompt.to_string(),
    }
}

/// Handle a `/`-prefixed REPL slash command.
/// Returns `Some(exit)` if handled locally (exit=true means quit the loop),
/// or `None` if the input should be passed to the agent unchanged.
async fn handle_slash_command(
    cmd: &str,
    total_turns: u32,
    total_cost_usd: f64,
    agent_loop: &mut AgentLoop,
) -> Option<bool> {
    let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
    match parts[0] {
        "/help" => {
            eprintln!("REPL commands (not sent to the agent):");
            eprintln!("  /help              — show this list");
            eprintln!("  /cost              — current session cost and turn count");
            eprintln!("  /compact           — summarise and compact the conversation history");
            eprintln!("  /model <name>      — switch the active model for this session");
            eprintln!("  /sessions          — list recent sessions");
            eprintln!(
                "  /resume <id>       — restart with an existing session (use --resume flag)"
            );
            eprintln!("  /mode plan-only    — reminder: restart with --permission-mode plan-only");
            eprintln!("  /mode agent        — reminder: restart without --permission-mode");
            eprintln!("  /exit  /quit       — end the REPL");
            eprintln!("  /cls               — clear the screen");
            eprintln!("  //...              — send a literal prompt starting with /");
            eprintln!();
            eprintln!("Agent routing shortcuts:");
            eprintln!("  @worker <task>     — route immediately to the SpawnWorker sub-agent");
            eprintln!("  @reviewer <task>   — route immediately to the SpawnReviewer sub-agent");
            eprintln!("  @explore <query>   — hint to explore/search the codebase");
        }
        "/cost" => {
            eprintln!(
                "Session: {} turns, ${:.4} total",
                total_turns, total_cost_usd
            );
        }
        "/compact" => {
            eprint!("Compacting history…");
            let _ = io::stderr().flush();
            match agent_loop.compact_history_now().await {
                Ok((before, after)) => {
                    eprintln!(" done ({before} → {after} messages).");
                }
                Err(e) => {
                    eprintln!(" failed: {e}");
                }
            }
        }
        "/model" => {
            let name = parts.get(1).map(|s| s.trim()).unwrap_or("");
            if name.is_empty() {
                eprintln!("Current model: {}", agent_loop.current_model());
                eprintln!("Usage: /model <model-name>");
            } else {
                agent_loop.set_model(name.to_string());
                eprintln!("Switched to model: {}", agent_loop.current_model());
            }
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
                "plan" | "plan-only" => {
                    eprintln!("To switch to plan-only mode, restart with: clido --permission-mode plan-only")
                }
                "accept-all" => eprintln!(
                    "To enable accept-all mode, restart with: clido --permission-mode accept-all"
                ),
                "diff-review" => eprintln!(
                    "To enable diff-review mode, restart with: clido --permission-mode diff-review"
                ),
                "agent" | "default" => eprintln!(
                    "Currently in agent mode. Restart without --permission-mode to use default."
                ),
                _ => eprintln!("Usage: /mode [plan-only | accept-all | diff-review | default]"),
            }
        }
        "/exit" | "/quit" => return Some(true),
        "/cls" | "/clear-screen" => {
            // Clear the terminal screen (ANSI escape: erase + move to top).
            eprint!("\x1b[2J\x1b[H");
            let _ = io::stderr().flush();
        }
        // Unknown: pass to the agent unchanged.
        _ => return None,
    }
    Some(false)
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
        let prompt: String = if line.starts_with("//") {
            line[1..].to_string()
        } else if line.starts_with('/') {
            match handle_slash_command(line, total_turns, total_cost_usd, &mut loop_).await {
                Some(true) => break,      // /exit or /quit
                Some(false) => continue,  // handled locally, nothing to send
                None => line.to_string(), // unknown — pass to agent as-is
            }
        } else {
            // @agent mention routing: rewrite the prompt if applicable.
            let (agent_hint, cleaned) = parse_agent_mention(line);
            if let Some(agent) = agent_hint {
                route_to_agent(agent, &cleaned)
            } else {
                line.to_string()
            }
        };

        let result = if first_turn {
            loop_
                .run(
                    &prompt,
                    Some(&mut writer),
                    Some(&setup.pricing_table),
                    Some(cancel.clone()),
                )
                .await
        } else {
            loop_
                .run_next_turn(
                    &prompt,
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
                    println!(
                        "{}",
                        serde_json::to_string(&out)
                            .unwrap_or_else(|_| r#"{"error":"serialization_failed"}"#.to_string())
                    );
                } else {
                    println!("{}", text);
                }
            }
            Err(ClidoError::DoomLoop { tool, error }) => {
                let msg = format!(
                    "Stuck: tool '{}' failed with the same error 3 times in a row.\n\
                     Error: {}\n\
                     The agent has been stopped. Try rephrasing your request, \
                     checking the tool configuration, or running with --permission-mode accept-all.",
                    tool, error
                );
                if cli.output_format == "json" {
                    let out = serde_json::json!({
                        "schema_version": 1,
                        "type": "repl_turn",
                        "is_error": true,
                        "error_kind": "doom_loop",
                        "result": msg,
                    });
                    println!(
                        "{}",
                        serde_json::to_string(&out)
                            .unwrap_or_else(|_| r#"{"error":"serialization_failed"}"#.to_string())
                    );
                } else {
                    eprintln!("⚠ {}", msg);
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
                    println!(
                        "{}",
                        serde_json::to_string(&out)
                            .unwrap_or_else(|_| r#"{"error":"serialization_failed"}"#.to_string())
                    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agent_mention_worker() {
        let (agent, prompt) = parse_agent_mention("@worker refactor the auth module");
        assert_eq!(agent, Some("worker"));
        assert_eq!(prompt, "refactor the auth module");
    }

    #[test]
    fn parse_agent_mention_reviewer() {
        let (agent, prompt) = parse_agent_mention("@reviewer check this PR");
        assert_eq!(agent, Some("reviewer"));
        assert_eq!(prompt, "check this PR");
    }

    #[test]
    fn parse_agent_mention_explore() {
        let (agent, prompt) = parse_agent_mention("@explore find all API endpoints");
        assert_eq!(agent, Some("explore"));
        assert_eq!(prompt, "find all API endpoints");
    }

    #[test]
    fn parse_agent_mention_general() {
        let (agent, prompt) = parse_agent_mention("@general do X");
        assert_eq!(agent, Some("general"));
        assert_eq!(prompt, "do X");
    }

    #[test]
    fn parse_agent_mention_no_mention() {
        let (agent, prompt) = parse_agent_mention("fix the bug in auth.rs");
        assert_eq!(agent, None);
        assert_eq!(prompt, "fix the bug in auth.rs");
    }

    #[test]
    fn parse_agent_mention_unknown_at() {
        // Unknown @name should be passed through unchanged.
        let (agent, prompt) = parse_agent_mention("@unknown something");
        assert_eq!(agent, None);
        assert!(prompt.contains("@unknown"));
    }

    #[test]
    fn parse_agent_mention_shorthand_w() {
        let (agent, _) = parse_agent_mention("@w refactor");
        assert_eq!(agent, Some("worker"));
    }

    #[test]
    fn route_to_agent_worker_contains_spawn() {
        let out = route_to_agent("worker", "write tests");
        assert!(out.contains("SpawnWorker"), "expected SpawnWorker: {out}");
        assert!(out.contains("write tests"), "expected original task: {out}");
    }

    #[test]
    fn route_to_agent_reviewer_contains_spawn() {
        let out = route_to_agent("reviewer", "review this code");
        assert!(
            out.contains("SpawnReviewer"),
            "expected SpawnReviewer: {out}"
        );
    }

    #[test]
    fn route_to_agent_explore_passthrough() {
        let out = route_to_agent("explore", "find the auth module");
        assert!(out.contains("find the auth module"));
    }
}
