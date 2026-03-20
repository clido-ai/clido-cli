//! Single-shot agent execution.

use clido_agent::{session_lines_to_messages, AgentLoop};
use clido_core::ClidoError;
use clido_storage::{list_sessions, stale_paths, SessionLine, SessionReader, SessionWriter};
use std::env;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::agent_setup::AgentSetup;
use crate::cli::Cli;
use crate::errors::CliError;
use crate::ui::{ansi, cli_use_color};

pub async fn run_agent(cli: Cli) -> Result<(), anyhow::Error> {
    if cli.output_format == "stream-json" {
        return Err(CliError::Usage(
            "stream-json is not yet implemented. Use text or json.".into(),
        )
        .into());
    }

    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let resume_id = resolve_resume_id(&cli, &workspace_root)?;
    let resume_lines = load_resume_lines(&cli, &resume_id, &workspace_root)?;

    let setup = AgentSetup::build(&cli, &workspace_root)?;
    let (session_id, mut writer) = match &resume_id {
        Some(id) => (id.clone(), SessionWriter::append(&workspace_root, id)?),
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let w = SessionWriter::create(&workspace_root, &id)?;
            (id, w)
        }
    };

    let cancel = make_cancel_token();
    let start = std::time::Instant::now();

    let (result, num_turns, total_cost_usd) = match &resume_lines {
        Some(lines) => {
            let history = session_lines_to_messages(lines);
            if history.is_empty() {
                let mut loop_ =
                    AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user);
                let r = loop_
                    .run(
                        &cli.prompt_str(),
                        Some(&mut writer),
                        Some(&setup.pricing_table),
                        Some(cancel),
                    )
                    .await;
                (r, loop_.turn_count(), loop_.cumulative_cost_usd)
            } else {
                let mut loop_ = AgentLoop::new_with_history(
                    setup.provider,
                    setup.registry,
                    setup.config,
                    history,
                    setup.ask_user,
                );
                let r = loop_
                    .run_continue(Some(&mut writer), Some(&setup.pricing_table), Some(cancel))
                    .await;
                (r, loop_.turn_count(), loop_.cumulative_cost_usd)
            }
        }
        None => {
            let mut loop_ =
                AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user);
            let r = loop_
                .run(
                    &cli.prompt_str(),
                    Some(&mut writer),
                    Some(&setup.pricing_table),
                    Some(cancel),
                )
                .await;
            (r, loop_.turn_count(), loop_.cumulative_cost_usd)
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    let exit_status = if result.is_ok() { "completed" } else { "error" };

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
        exit_status: exit_status.to_string(),
        total_cost_usd,
        num_turns,
        duration_ms,
    })?;

    emit_result(
        result,
        &cli.output_format,
        &session_id,
        num_turns,
        duration_ms,
        total_cost_usd,
    )
}

fn resolve_resume_id(
    cli: &Cli,
    workspace_root: &std::path::Path,
) -> Result<Option<String>, anyhow::Error> {
    if let Some(id) = &cli.resume {
        if cli.r#continue {
            return Err(CliError::Usage(
                "Cannot use both --resume and --continue. Use one.".into(),
            )
            .into());
        }
        return Ok(Some(id.clone()));
    }
    if cli.r#continue {
        let sessions = list_sessions(workspace_root)?;
        let id = sessions
            .first()
            .map(|s| s.session_id.clone())
            .ok_or_else(|| {
                CliError::Usage("No session to continue. Run 'clido <prompt>' first.".into())
            })?;
        return Ok(Some(id));
    }
    Ok(None)
}

fn load_resume_lines(
    cli: &Cli,
    resume_id: &Option<String>,
    workspace_root: &std::path::Path,
) -> Result<Option<Vec<clido_storage::SessionLine>>, anyhow::Error> {
    let Some(ref session_id) = resume_id else {
        return Ok(None);
    };
    let lines = SessionReader::load(workspace_root, session_id)
        .map_err(|e| CliError::Usage(format!("Failed to load session: {}", e)))?;
    let records = SessionReader::stale_file_records(&lines);
    let stale = stale_paths(&records);
    if !stale.is_empty() && !cli.resume_ignore_stale {
        let msg = format!(
            "Cannot resume: file(s) modified since session: {} Use --resume-ignore-stale to continue anyway.",
            stale.join(", ")
        );
        if crate::sessions::is_stdin_tty() && !cli.print {
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
    Ok(Some(lines))
}

fn make_cancel_token() -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let handle = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        handle.store(true, Ordering::Relaxed);
    });
    cancel
}

fn emit_result(
    result: clido_core::Result<String>,
    output_format: &str,
    session_id: &str,
    num_turns: u32,
    duration_ms: u64,
    total_cost_usd: f64,
) -> Result<(), anyhow::Error> {
    match result {
        Ok(text) => {
            if output_format == "json" {
                let out = serde_json::json!({
                    "schema_version": 1,
                    "type": "result",
                    "exit_status": "completed",
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
            if output_format == "json" {
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
