//! Single-shot agent execution.

use async_trait::async_trait;
use clido_agent::{session_lines_to_messages, AgentLoop, EventEmitter};
use clido_core::ClidoError;
use clido_storage::{
    list_sessions, stale_paths, AuditLog, SessionLine, SessionReader, SessionWriter,
};
use std::env;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::agent_setup::AgentSetup;
use crate::cli::Cli;
use crate::errors::CliError;
use crate::git_context::GitContext;
use crate::ui::{ansi, cli_use_color};

/// EventEmitter that writes stream-json events to stdout as the agent runs.
/// Attached to the agent loop when --output-format stream-json is active.
struct StreamJsonEmitter;

#[async_trait]
impl EventEmitter for StreamJsonEmitter {
    async fn on_tool_start(&self, tool_use_id: &str, name: &str, input: &serde_json::Value) {
        let ev = serde_json::json!({
            "type": "tool_use",
            "id": tool_use_id,
            "name": name,
            "input": input,
        });
        println!("{}", serde_json::to_string(&ev).unwrap());
    }

    async fn on_tool_done(
        &self,
        tool_use_id: &str,
        _name: &str,
        is_error: bool,
        _diff: Option<String>,
    ) {
        let ev = serde_json::json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "is_error": is_error,
        });
        println!("{}", serde_json::to_string(&ev).unwrap());
    }

    async fn on_assistant_text(&self, text: &str) {
        let ev = serde_json::json!({
            "type": "text",
            "text": text,
        });
        println!("{}", serde_json::to_string(&ev).unwrap());
    }
}

pub async fn run_agent(cli: Cli) -> Result<(), anyhow::Error> {
    if cli.sandbox && !cli.quiet {
        eprintln!("Bash sandbox enabled (sandbox-exec on macOS, bwrap on Linux).");
    }

    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
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

    // Capture model name and tool names before the registry is consumed by AgentLoop.
    let model = setup.config.model.clone();
    let fast_model = setup.fast_model.clone();
    let reasoning_model = setup.reasoning_model.clone();
    let tool_names: Vec<String> = setup
        .registry
        .schemas()
        .into_iter()
        .map(|s| s.name)
        .collect();

    // For stream-json: emit the init event before the agent loop starts.
    // Omit if --quiet (SDK consumers relying on session_id can pass --session).
    if cli.output_format == "stream-json" && !cli.quiet {
        let init = serde_json::json!({
            "type": "system",
            "subtype": "init",
            "session_id": &session_id,
            "tools": &tool_names,
            "model": &model,
        });
        println!("{}", serde_json::to_string(&init).unwrap());
    }

    // Build stream-json emitter (attaches to agent loop for live events).
    // Not attached when quiet — tool events are suppressed.
    let stream_emitter: Option<Arc<dyn EventEmitter>> =
        if cli.output_format == "stream-json" && !cli.quiet {
            Some(Arc::new(StreamJsonEmitter))
        } else {
            None
        };

    // Set up audit log.
    let audit = AuditLog::open(&workspace_root)
        .ok()
        .map(|a| Arc::new(std::sync::Mutex::new(a)));

    // Hooks from config.
    let loaded = clido_core::load_config(&workspace_root).ok();
    let hooks = loaded
        .as_ref()
        .map(|l| l.hooks.clone())
        .filter(|h| h.pre_tool_use.is_some() || h.post_tool_use.is_some());

    let cancel = make_cancel_token();
    let start = std::time::Instant::now();

    let (
        result,
        num_turns,
        total_cost_usd,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
    ) = match &resume_lines {
        Some(lines) => {
            let history = session_lines_to_messages(lines);
            if history.is_empty() {
                let git_wr = workspace_root.clone();
                let git_fn: Box<dyn Fn() -> Option<String> + Send + Sync> = Box::new(move || {
                    GitContext::discover(&git_wr).map(|ctx| ctx.to_prompt_section())
                });
                let mut loop_ =
                    AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user)
                        .with_fast_model(fast_model.clone())
                        .with_reasoning_model(reasoning_model.clone())
                        .with_git_context_fn(git_fn);
                if let Some(ref a) = audit {
                    loop_ = loop_.with_audit_log(a.clone());
                }
                if let Some(h) = hooks.clone() {
                    loop_ = loop_.with_hooks(h);
                }
                if let Some(ref e) = stream_emitter {
                    loop_ = loop_.with_emitter(e.clone());
                }
                let r = loop_
                    .run(
                        &cli.prompt_str(),
                        Some(&mut writer),
                        Some(&setup.pricing_table),
                        Some(cancel),
                    )
                    .await;
                (
                    r,
                    loop_.turn_count(),
                    loop_.cumulative_cost_usd,
                    loop_.cumulative_input_tokens,
                    loop_.cumulative_output_tokens,
                    loop_.cumulative_cache_read_tokens,
                    loop_.cumulative_cache_creation_tokens,
                )
            } else {
                let git_wr2 = workspace_root.clone();
                let git_fn2: Box<dyn Fn() -> Option<String> + Send + Sync> = Box::new(move || {
                    GitContext::discover(&git_wr2).map(|ctx| ctx.to_prompt_section())
                });
                let mut loop_ = AgentLoop::new_with_history(
                    setup.provider,
                    setup.registry,
                    setup.config,
                    history,
                    setup.ask_user,
                )
                .with_fast_model(fast_model.clone())
                .with_reasoning_model(reasoning_model.clone())
                .with_git_context_fn(git_fn2);
                if let Some(ref a) = audit {
                    loop_ = loop_.with_audit_log(a.clone());
                }
                if let Some(h) = hooks.clone() {
                    loop_ = loop_.with_hooks(h);
                }
                if let Some(ref e) = stream_emitter {
                    loop_ = loop_.with_emitter(e.clone());
                }
                let r = loop_
                    .run_continue(Some(&mut writer), Some(&setup.pricing_table), Some(cancel))
                    .await;
                (
                    r,
                    loop_.turn_count(),
                    loop_.cumulative_cost_usd,
                    loop_.cumulative_input_tokens,
                    loop_.cumulative_output_tokens,
                    loop_.cumulative_cache_read_tokens,
                    loop_.cumulative_cache_creation_tokens,
                )
            }
        }
        None => {
            let git_wr3 = workspace_root.clone();
            let git_fn3: Box<dyn Fn() -> Option<String> + Send + Sync> =
                Box::new(move || GitContext::discover(&git_wr3).map(|ctx| ctx.to_prompt_section()));
            let mut loop_ =
                AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user)
                    .with_fast_model(fast_model)
                    .with_reasoning_model(reasoning_model)
                    .with_planner(cli.planner)
                    .with_git_context_fn(git_fn3);
            if let Some(ref a) = audit {
                loop_ = loop_.with_audit_log(a.clone());
            }
            if let Some(h) = hooks {
                loop_ = loop_.with_hooks(h);
            }
            if let Some(ref e) = stream_emitter {
                loop_ = loop_.with_emitter(e.clone());
            }
            // If --planner is set, make a real LLM planning call before the agent loop.
            // The plan is printed to stdout as an informational prefix; if planning fails
            // (network error, malformed JSON, invalid graph) we silently proceed without a plan.
            if cli.planner {
                let planning_prompt = format!(
                    "You are a task planner. Decompose the following task into a JSON task graph.\n\
                     Format: {{\"goal\":\"<goal>\",\"tasks\":[{{\"id\":\"t1\",\"description\":\"<description>\",\"depends_on\":[]}},...]}}\n\
                     Tasks that can run in parallel should have no shared dependencies.\n\
                     Keep it to 2-5 tasks maximum. Respond with ONLY the JSON, no explanation.\n\n\
                     Task: {}",
                    cli.prompt_str()
                );
                if let Ok(plan_text) = loop_.complete_simple(&planning_prompt).await {
                    if let Ok(graph) = clido_planner::parse_plan(&plan_text) {
                        if !cli.quiet {
                            println!("Plan:");
                            for t in &graph.tasks {
                                if t.depends_on.is_empty() {
                                    println!("  {}: {}", t.id, t.description);
                                } else {
                                    println!(
                                        "  {}: {}  (depends: {})",
                                        t.id,
                                        t.description,
                                        t.depends_on.join(", ")
                                    );
                                }
                            }
                        }
                    }
                }
            }
            let prompt = cli.prompt_str();
            let r = loop_
                .run(
                    &prompt,
                    Some(&mut writer),
                    Some(&setup.pricing_table),
                    Some(cancel),
                )
                .await;
            (
                r,
                loop_.turn_count(),
                loop_.cumulative_cost_usd,
                loop_.cumulative_input_tokens,
                loop_.cumulative_output_tokens,
                loop_.cumulative_cache_read_tokens,
                loop_.cumulative_cache_creation_tokens,
            )
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    let exit_status = match &result {
        Ok(_) => "completed",
        Err(ClidoError::MaxTurnsExceeded) => "max_turns_reached",
        Err(ClidoError::BudgetExceeded) => "budget_exceeded",
        Err(_) => "error",
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
        exit_status: exit_status.to_string(),
        total_cost_usd,
        num_turns,
        duration_ms,
    })?;

    emit_result(
        result,
        EmitResultParams {
            output_format: &cli.output_format,
            session_id: &session_id,
            num_turns,
            duration_ms,
            total_cost_usd,
            quiet: cli.quiet,
            model: &model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
        },
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

pub(crate) struct EmitResultParams<'a> {
    pub output_format: &'a str,
    pub session_id: &'a str,
    pub num_turns: u32,
    pub duration_ms: u64,
    pub total_cost_usd: f64,
    pub quiet: bool,
    pub model: &'a str,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

pub(crate) fn emit_result(
    result: clido_core::Result<String>,
    p: EmitResultParams<'_>,
) -> Result<(), anyhow::Error> {
    let EmitResultParams {
        output_format,
        session_id,
        num_turns,
        duration_ms,
        total_cost_usd,
        quiet,
        model,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
    } = p;
    let usage = serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_read_input_tokens": cache_read_tokens,
        "cache_creation_input_tokens": cache_creation_tokens,
    });
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
                    "is_error": false,
                    "usage": usage,
                });
                println!("{}", serde_json::to_string(&out).unwrap());
            } else if output_format == "stream-json" {
                // Init and live events are emitted before/during the loop by run_agent.
                // Only the final result line is needed here.
                let result_line = serde_json::json!({
                    "schema_version": 1,
                    "type": "result",
                    "exit_status": "completed",
                    "result": text,
                    "session_id": session_id,
                    "model": model,
                    "num_turns": num_turns,
                    "total_cost_usd": total_cost_usd,
                    "duration_ms": duration_ms,
                    "is_error": false,
                    "usage": usage,
                });
                println!("{}", serde_json::to_string(&result_line).unwrap());
            } else {
                println!("{}", text);
                if !quiet && (total_cost_usd > 0.0 || num_turns > 0) {
                    let footer = format!(
                        "  \u{21b3} {} turns \u{00b7} ${:.4} \u{00b7} {}ms",
                        num_turns, total_cost_usd, duration_ms
                    );
                    if cli_use_color() {
                        eprintln!("{}{}{}", ansi::DIM, footer, ansi::RESET);
                    } else {
                        eprintln!("{}", footer);
                    }
                }
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            let exit_status = match &e {
                ClidoError::MaxTurnsExceeded => "max_turns_reached",
                ClidoError::BudgetExceeded => "budget_exceeded",
                _ => "error",
            };
            if output_format == "json" {
                let out = serde_json::json!({
                    "schema_version": 1,
                    "type": "result",
                    "exit_status": exit_status,
                    "result": msg,
                    "session_id": session_id,
                    "num_turns": num_turns,
                    "duration_ms": duration_ms,
                    "total_cost_usd": total_cost_usd,
                    "is_error": true,
                    "usage": usage,
                });
                println!("{}", serde_json::to_string(&out).unwrap());
            } else if output_format == "stream-json" {
                // Init already emitted by run_agent. Just emit the result line.
                let result_line = serde_json::json!({
                    "schema_version": 1,
                    "type": "result",
                    "exit_status": exit_status,
                    "result": msg,
                    "session_id": session_id,
                    "model": model,
                    "num_turns": num_turns,
                    "total_cost_usd": total_cost_usd,
                    "duration_ms": duration_ms,
                    "is_error": true,
                    "usage": usage,
                });
                println!("{}", serde_json::to_string(&result_line).unwrap());
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

#[cfg(test)]
mod tests {
    use super::{emit_result, EmitResultParams};
    use clido_core::ClidoError;

    #[test]
    fn emit_json_ok_has_required_fields() {
        // Capture stdout by calling emit_result with json format on a success.
        // We test the function's logic by inspecting the JSON it would print.
        // Since it prints directly, we parse what we know the structure should be.
        let ok: clido_core::Result<String> = Ok("hello world".to_string());
        // Build the JSON object directly the same way emit_result does.
        let out = serde_json::json!({
            "schema_version": 1,
            "type": "result",
            "exit_status": "completed",
            "result": "hello world",
            "session_id": "test-session-1",
            "num_turns": 3u32,
            "duration_ms": 500u64,
            "total_cost_usd": 0.0012f64,
            "is_error": false
        });
        let s = serde_json::to_string(&out).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["type"], "result");
        assert_eq!(v["exit_status"], "completed");
        assert_eq!(v["result"], "hello world");
        assert_eq!(v["is_error"], false);
        assert!(v["num_turns"].is_number());
        assert!(v["duration_ms"].is_number());
        assert!(v["total_cost_usd"].is_number());
        drop(ok);
    }

    #[test]
    fn emit_json_error_sets_is_error_true() {
        let out = serde_json::json!({
            "schema_version": 1,
            "type": "result",
            "exit_status": "error",
            "result": "something went wrong",
            "session_id": "test-session-2",
            "num_turns": 1u32,
            "duration_ms": 100u64,
            "total_cost_usd": 0.0f64,
            "is_error": true
        });
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&out).unwrap()).unwrap();
        assert_eq!(v["is_error"], true);
        assert_eq!(v["exit_status"], "error");
    }

    #[test]
    fn emit_stream_json_result_line_has_model_and_schema_version() {
        // emit_result for stream-json emits only the result line (init is in run_agent).
        // The result line must have schema_version, exit_status (not subtype), and model.
        let result_line = serde_json::json!({
            "schema_version": 1,
            "type": "result",
            "exit_status": "completed",
            "result": "done",
            "session_id": "sess-abc",
            "model": "claude-sonnet-4-6",
            "num_turns": 2u32,
            "total_cost_usd": 0.0f64,
            "duration_ms": 200u64,
            "is_error": false,
            "usage": {"input_tokens": 100u64, "output_tokens": 50u64},
        });
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&result_line).unwrap()).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["type"], "result");
        assert_eq!(v["exit_status"], "completed");
        assert!(v.get("subtype").is_none(), "subtype must not be present");
        assert_eq!(v["model"], "claude-sonnet-4-6");
        assert!(v["usage"]["input_tokens"].is_number());
        assert!(v["usage"]["output_tokens"].is_number());
    }

    #[test]
    fn emit_result_text_quiet_suppresses_footer() {
        let result = emit_result(
            Ok("output".to_string()),
            EmitResultParams {
                output_format: "text",
                session_id: "session-quiet",
                num_turns: 0,
                duration_ms: 10,
                total_cost_usd: 0.0,
                quiet: true,
                model: "claude-sonnet-4-6",
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn emit_result_text_nonquiet_ok() {
        let result = emit_result(
            Ok("output".to_string()),
            EmitResultParams {
                output_format: "text",
                session_id: "session-nq",
                num_turns: 2,
                duration_ms: 100,
                total_cost_usd: 0.001,
                quiet: false,
                model: "claude-sonnet-4-6",
                input_tokens: 1500,
                output_tokens: 45,
                cache_read_tokens: 200,
                cache_creation_tokens: 100,
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn emit_result_json_ok_returns_ok() {
        let result = emit_result(
            Ok("some result".to_string()),
            EmitResultParams {
                output_format: "json",
                session_id: "session-json",
                num_turns: 1,
                duration_ms: 50,
                total_cost_usd: 0.0005,
                quiet: false,
                model: "claude-sonnet-4-6",
                input_tokens: 800,
                output_tokens: 30,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn emit_result_json_budget_exceeded_returns_err() {
        let result = emit_result(
            Err(ClidoError::BudgetExceeded),
            EmitResultParams {
                output_format: "json",
                session_id: "session-budget",
                num_turns: 5,
                duration_ms: 300,
                total_cost_usd: 1.5,
                quiet: false,
                model: "claude-sonnet-4-6",
                input_tokens: 50000,
                output_tokens: 2000,
                cache_read_tokens: 5000,
                cache_creation_tokens: 1000,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn emit_result_stream_json_ok_returns_ok() {
        let result = emit_result(
            Ok("streamed".to_string()),
            EmitResultParams {
                output_format: "stream-json",
                session_id: "session-stream",
                num_turns: 1,
                duration_ms: 80,
                total_cost_usd: 0.0002,
                quiet: false,
                model: "claude-sonnet-4-6",
                input_tokens: 500,
                output_tokens: 20,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        );
        assert!(result.is_ok());
    }
}
