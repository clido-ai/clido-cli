use std::io::{stdout, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clido_agent::AgentLoop;
use clido_core::ClidoError;
use clido_storage::SessionWriter;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::agent_setup::AgentSetup;
use crate::cli::Cli;
use crate::git_context::GitContext;
use clido_planner::PlanEditor;

use super::*;

pub(super) fn tui_memory_store_path() -> Result<std::path::PathBuf, String> {
    if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") {
        let data = dirs.data_dir().to_path_buf();
        std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
        return Ok(data.join("memory.db"));
    }
    Ok(std::path::PathBuf::from(".clido-memory.db"))
}

pub(super) fn resolve_workdir_arg(arg: &str) -> Result<std::path::PathBuf, String> {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return Err("workdir path cannot be empty".into());
    }
    let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .map(|h| h.join(rest))
            .map_err(|_| "HOME is not set".to_string())?
    } else if trimmed == "~" {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .map_err(|_| "HOME is not set".to_string())?
    } else {
        std::path::PathBuf::from(trimmed)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|e| format!("could not resolve current dir: {}", e))?
            .join(expanded)
    };
    let canonical = std::fs::canonicalize(&absolute)
        .map_err(|e| format!("could not access '{}': {}", absolute.display(), e))?;
    if !canonical.is_dir() {
        return Err(format!("not a directory: {}", canonical.display()));
    }
    Ok(canonical)
}

/// Copy text to the system clipboard.
/// Tries native clipboard tools first (pbcopy on macOS, wl-copy on Wayland,
/// xclip/xsel on X11), then falls back to OSC 52 escape sequence.
pub(super) fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if text.is_empty() {
        return Err("nothing to copy".into());
    }

    // macOS
    #[cfg(target_os = "macos")]
    {
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return Ok(());
            }
        }
    }

    // Linux — try wl-copy (Wayland) then xclip then xsel
    #[cfg(target_os = "linux")]
    {
        for (cmd, args) in &[
            ("wl-copy", vec![]),
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
        ] {
            if let Ok(mut child) = Command::new(cmd)
                .args(args.as_slice())
                .stdin(Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                if child.wait().map(|s| s.success()).unwrap_or(false) {
                    return Ok(());
                }
            }
        }
    }

    // Fallback: OSC 52 escape sequence (works in terminals that support it,
    // e.g. iTerm2, kitty, Alacritty with the feature enabled).
    {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let encoded = STANDARD.encode(text.as_bytes());
        print!("\x1b]52;c;{}\x07", encoded);
        std::io::stdout()
            .flush()
            .map_err(|e| format!("clipboard write failed: {}", e))?;
    }
    Ok(())
}

#[allow(dead_code)]
pub(super) fn copy_to_clipboard_osc52(text: &str) -> Result<(), String> {
    copy_to_clipboard(text)
}

// ── Agent background task ─────────────────────────────────────────────────────

pub(super) enum AgentAction {
    Run(String),
    Resume(String),
    SwitchModel(String),
    SetWorkspace(std::path::PathBuf),
    CompactNow,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn agent_task(
    cli: Cli,
    workspace_root: std::path::PathBuf,
    preloaded_config: Option<clido_core::LoadedConfig>,
    preloaded_pricing: clido_core::PricingTable,
    mut prompt_rx: mpsc::UnboundedReceiver<String>,
    mut resume_rx: mpsc::UnboundedReceiver<String>,
    mut model_switch_rx: mpsc::UnboundedReceiver<String>,
    mut workdir_rx: mpsc::UnboundedReceiver<std::path::PathBuf>,
    mut compact_now_rx: mpsc::UnboundedReceiver<()>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    perm_tx: mpsc::UnboundedSender<PermRequest>,
    cancel: std::sync::Arc<AtomicBool>,
    image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
    reviewer_enabled: Arc<AtomicBool>,
    todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
) {
    let setup_result = match preloaded_config {
        Some(loaded) => AgentSetup::build_with_preloaded_and_store(
            &cli,
            &workspace_root,
            loaded,
            preloaded_pricing,
            reviewer_enabled,
            Some(todo_store),
        ),
        None => AgentSetup::build(&cli, &workspace_root),
    };
    let mut setup = match setup_result {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Err(e.to_string()));
            return;
        }
    };

    let perms = Arc::new(Mutex::new(PermsState::default()));
    setup.ask_user = Some(Arc::new(TuiAskUser {
        perm_tx,
        perms: perms.clone(),
    }));

    let session_id = if let Some(id) = &cli.resume {
        id.clone()
    } else {
        uuid::Uuid::new_v4().to_string()
    };
    let writer = if cli.resume.is_some() {
        SessionWriter::append(&workspace_root, &session_id)
    } else {
        SessionWriter::create(&workspace_root, &session_id)
    };
    let mut writer = match writer {
        Ok(w) => w,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Err(e.to_string()));
            return;
        }
    };
    let _ = event_tx.send(AgentEvent::SessionStarted(session_id.clone()));

    let emitter: Arc<dyn EventEmitter> = Arc::new(TuiEmitter {
        tx: event_tx.clone(),
    });

    let planner_mode = cli.planner;
    let context_max_tokens = setup.config.max_context_tokens.unwrap_or(200_000) as u64;
    // Capture values for async title generation before setup is moved.
    let title_provider = setup.provider.clone();
    let title_fast_model = setup
        .fast_model
        .clone()
        .unwrap_or_else(|| setup.config.model.clone());
    let git_workspace = workspace_root.clone();
    let git_context_fn: Box<dyn Fn() -> Option<String> + Send + Sync> =
        Box::new(move || GitContext::discover(&git_workspace).map(|ctx| ctx.to_prompt_section()));
    let mut agent = AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user)
        .with_fast_model(setup.fast_model)
        .with_reasoning_model(setup.reasoning_model)
        .with_emitter(emitter)
        .with_planner(planner_mode)
        .with_git_context_fn(git_context_fn);

    let mut first_turn = true;
    let mut title_generated = cli.resume.is_some(); // skip title gen for resumed sessions
                                                    // Auto-continue counter: when the turn limit is hit mid-task, clido automatically
                                                    // injects "please continue" so the agent never stops mid-work. We cap this at
                                                    // MAX_AUTO_CONTINUES to avoid infinite loops on genuinely stuck agents.
    const MAX_AUTO_CONTINUES: u32 = 5;
    let mut auto_continue_count: u32 = 0;

    if let Some(resume_session_id) = cli.resume.clone() {
        match clido_storage::SessionReader::load(&workspace_root, &resume_session_id) {
            Err(e) => {
                let _ = event_tx.send(AgentEvent::Err(format!("resume failed: {}", e)));
            }
            Ok(lines) => {
                let new_history = clido_agent::session_lines_to_messages(&lines);
                agent.replace_history(new_history);
                match SessionWriter::append(&workspace_root, &resume_session_id) {
                    Ok(new_writer) => {
                        writer = new_writer;
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!("resume writer: {}", e)));
                    }
                }
                // Warn if any files referenced in the session have changed since recording.
                let stale_records = clido_storage::SessionReader::stale_file_records(&lines);
                let stale = clido_storage::stale_paths(&stale_records);
                if !stale.is_empty() {
                    let list = stale
                        .iter()
                        .map(|p| format!("  • {}", p))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let _ = event_tx.send(AgentEvent::Thinking(format!(
                        "⚠ Some files referenced in this session have changed since it was recorded:\n{}\n\
                         The agent's context may be stale for these files.",
                        list
                    )));
                }
                let mut msgs: Vec<(String, String)> = Vec::new();
                for line in &lines {
                    match line {
                        clido_storage::SessionLine::UserMessage { content, .. } => {
                            if let Some(t) = content
                                .first()
                                .and_then(|c| c.get("text"))
                                .and_then(|v| v.as_str())
                            {
                                msgs.push(("user".to_string(), t.to_string()));
                            }
                        }
                        clido_storage::SessionLine::AssistantMessage { content } => {
                            let text: String = content
                                .iter()
                                .filter_map(|c| {
                                    if c.get("type").and_then(|v| v.as_str()) == Some("text") {
                                        c.get("text")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("");
                            if !text.trim().is_empty() {
                                msgs.push(("assistant".to_string(), text));
                            }
                        }
                        _ => {}
                    }
                }
                first_turn = false;
                let _ = event_tx.send(AgentEvent::ResumedSession { messages: msgs });
            }
        }
    }

    loop {
        // Apply queued workdir changes before other actions so prompts never run
        // against stale tooling/permissions after a switch command.
        while let Ok(new_workspace) = workdir_rx.try_recv() {
            match AgentSetup::build(&cli, &new_workspace) {
                Ok(new_setup) => {
                    agent.replace_tools(new_setup.registry);
                    agent.reset_permission_mode_override();
                    if let Ok(mut state) = perms.lock() {
                        state.clear_all_grants();
                    }
                    let _ = event_tx.send(AgentEvent::WorkdirSwitched {
                        path: new_workspace,
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::Err(format!("workdir switch failed: {}", e)));
                }
            }
        }

        let action = tokio::select! {
            msg = prompt_rx.recv() => {
                match msg {
                    Some(prompt) => AgentAction::Run(prompt),
                    None => break,
                }
            }
            resume_id = resume_rx.recv() => {
                match resume_id {
                    Some(id) => AgentAction::Resume(id),
                    None => break,
                }
            }
            model_name = model_switch_rx.recv() => {
                match model_name {
                    Some(m) => AgentAction::SwitchModel(m),
                    None => break,
                }
            }
            new_workspace = workdir_rx.recv() => {
                match new_workspace {
                    Some(path) => AgentAction::SetWorkspace(path),
                    None => break,
                }
            }
            compact = compact_now_rx.recv() => {
                match compact {
                    Some(()) => AgentAction::CompactNow,
                    None => break,
                }
            }
        };

        match action {
            AgentAction::SwitchModel(model_name) => {
                agent.set_model(model_name.clone());
                let _ = event_tx.send(AgentEvent::ModelSwitched {
                    to_model: model_name,
                });
            }
            AgentAction::SetWorkspace(new_workspace) => {
                match AgentSetup::build(&cli, &new_workspace) {
                    Ok(new_setup) => {
                        agent.replace_tools(new_setup.registry);
                        agent.reset_permission_mode_override();
                        if let Ok(mut state) = perms.lock() {
                            state.clear_all_grants();
                        }
                        let _ = event_tx.send(AgentEvent::WorkdirSwitched {
                            path: new_workspace,
                        });
                    }
                    Err(e) => {
                        let _ =
                            event_tx.send(AgentEvent::Err(format!("workdir switch failed: {}", e)));
                    }
                }
            }
            AgentAction::CompactNow => {
                match agent.compact_history_now().await {
                    Ok((before, after)) => {
                        let _ = event_tx.send(AgentEvent::Compacted { before, after });
                        // Emit updated token counts so the context bar refreshes.
                        let _ = event_tx.send(AgentEvent::TokenUsage {
                            input_tokens: agent.cumulative_input_tokens,
                            output_tokens: agent.cumulative_output_tokens,
                            cost_usd: agent.cumulative_cost_usd,
                            context_max_tokens,
                        });
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!("compact: {}", e)));
                    }
                }
            }
            AgentAction::Run(prompt) => {
                cancel.store(false, std::sync::atomic::Ordering::Relaxed);

                // When --planner is active and this is the first turn, attempt to parse
                // a plan from the prompt. On success, emit PlanCreated so the TUI can
                // display the planned steps. On any failure, fall back to the reactive loop
                // transparently (no error shown to the user).
                if planner_mode && first_turn {
                    // Make a real LLM call to decompose the task into a JSON plan.
                    // On any failure (network, parse, invalid graph), silently fall back
                    // to the reactive loop — no error is shown to the user.
                    let planning_prompt = format!(
                        "You are a task planner. Decompose the following task into a JSON task graph.\n\
                         Format: {{\"goal\":\"<goal>\",\"tasks\":[{{\"id\":\"t1\",\"description\":\"<description>\",\"depends_on\":[]}},...]}}\n\
                         Tasks that can run in parallel should have no shared dependencies.\n\
                         Keep it to 2-5 tasks maximum. Respond with ONLY the JSON, no explanation.\n\n\
                         Task: {}",
                        prompt
                    );
                    if let Ok((plan_text, plan_usage)) =
                        agent.complete_simple_with_usage(&planning_prompt).await
                    {
                        let plan_cost = clido_core::pricing::compute_cost_usd(
                            &plan_usage,
                            agent.current_model(),
                            &setup.pricing_table,
                        );
                        let _ = event_tx.send(AgentEvent::TokenUsage {
                            input_tokens: plan_usage.input_tokens,
                            output_tokens: plan_usage.output_tokens,
                            cost_usd: plan_cost,
                            context_max_tokens,
                        });
                        // Try to parse as a full Plan with metadata first.
                        let plan_opt = clido_planner::parse_plan_with_meta(&plan_text).ok();
                        if let Some(plan) = plan_opt {
                            let task_descriptions: Vec<String> = plan
                                .tasks
                                .iter()
                                .map(|t| {
                                    if t.depends_on.is_empty() {
                                        format!("{}: {}", t.id, t.description)
                                    } else {
                                        format!(
                                            "{}: {}  (depends: {})",
                                            t.id,
                                            t.description,
                                            t.depends_on.join(", ")
                                        )
                                    }
                                })
                                .collect();
                            // If plan_no_edit is NOT set, emit PlanReady to open the TUI editor.
                            if !cli.plan_no_edit {
                                let _ = event_tx.send(AgentEvent::PlanReady { plan });
                                // Mark first_turn as consumed so the next prompt (execution)
                                // does not try to re-plan.
                                first_turn = false;
                                // Do not proceed with agent execution — wait for the user to
                                // approve/edit and press 'x' in the plan editor. The editor's
                                // 'x' key sends a combined prompt via send_now.
                                continue;
                            } else {
                                let _ = event_tx.send(AgentEvent::PlanCreated {
                                    tasks: task_descriptions,
                                });
                            }
                        }
                        // If parse fails, silently proceed (fallback to reactive)
                    }
                }

                // Drain any pending image attached via /image before this turn.
                let pending_img = image_state.lock().ok().and_then(|mut g| g.take());
                let extra_blocks: Vec<clido_core::ContentBlock> =
                    if let Some((media_type, base64_data)) = pending_img {
                        vec![clido_core::ContentBlock::Image {
                            media_type,
                            base64_data,
                        }]
                    } else {
                        vec![]
                    };

                // Spawn a heartbeat task that keeps the TUI stall-detector alive while the
                // LLM is generating (which can take >45 s for long responses).  The task
                // is aborted as soon as agent.run() returns.
                let hb_tx = event_tx.clone();
                let heartbeat = tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
                    interval.tick().await; // skip the immediate first tick
                    loop {
                        interval.tick().await;
                        if hb_tx.send(AgentEvent::Heartbeat).is_err() {
                            break;
                        }
                    }
                });

                let result = if extra_blocks.is_empty() {
                    if first_turn {
                        agent
                            .run(
                                &prompt,
                                Some(&mut writer),
                                Some(&setup.pricing_table),
                                Some(cancel.clone()),
                            )
                            .await
                    } else {
                        agent
                            .run_next_turn(
                                &prompt,
                                Some(&mut writer),
                                Some(&setup.pricing_table),
                                Some(cancel.clone()),
                            )
                            .await
                    }
                } else if first_turn {
                    agent
                        .run_with_extra_blocks(
                            &prompt,
                            extra_blocks,
                            Some(&mut writer),
                            Some(&setup.pricing_table),
                            Some(cancel.clone()),
                        )
                        .await
                } else {
                    agent
                        .run_next_turn_with_extra_blocks(
                            &prompt,
                            extra_blocks,
                            Some(&mut writer),
                            Some(&setup.pricing_table),
                            Some(cancel.clone()),
                        )
                        .await
                };
                heartbeat.abort();
                first_turn = false;

                // Emit token usage before response/error so TUI updates cost display.
                let _ = event_tx.send(AgentEvent::TokenUsage {
                    input_tokens: agent.cumulative_input_tokens,
                    output_tokens: agent.cumulative_output_tokens,
                    cost_usd: agent.cumulative_cost_usd,
                    context_max_tokens,
                });

                let mut session_exit: &str = "success";

                match result {
                    Ok(text) => {
                        auto_continue_count = 0; // reset on clean completion
                        let _ = event_tx.send(AgentEvent::Response(text.clone()));

                        // Generate session title after first successful response.
                        if !title_generated {
                            title_generated = true;
                            let title_prompt = prompt.clone();
                            let title_tx = event_tx.clone();
                            let tp = title_provider.clone();
                            let tm = title_fast_model.clone();
                            let mut title_writer =
                                clido_storage::SessionWriter::append(&workspace_root, &session_id)
                                    .ok();
                            tokio::spawn(async move {
                                let msgs = vec![clido_core::Message {
                                    role: clido_core::Role::User,
                                    content: vec![clido_core::ContentBlock::Text {
                                        text: format!(
                                            "Generate a concise title (max 6 words) for this conversation. \
                                             Output ONLY the title, no quotes, no explanation.\n\n\
                                             User message: {}",
                                            title_prompt.chars().take(200).collect::<String>()
                                        ),
                                    }],
                                }];
                                let cfg = clido_core::AgentConfig {
                                    model: tm,
                                    max_turns: 1,
                                    ..Default::default()
                                };
                                if let Ok(resp) = tp.complete(&msgs, &[], &cfg).await {
                                    let title = resp
                                        .content
                                        .iter()
                                        .find_map(|b| {
                                            if let clido_core::ContentBlock::Text { text } = b {
                                                Some(text.trim().to_string())
                                            } else {
                                                None
                                            }
                                        })
                                        .unwrap_or_default();
                                    if !title.is_empty() {
                                        if let Some(ref mut w) = title_writer {
                                            let _ =
                                                w.write_line(&clido_storage::SessionLine::Title {
                                                    title: title.clone(),
                                                });
                                        }
                                        let _ = title_tx.send(AgentEvent::TitleGenerated(title));
                                    }
                                }
                            });
                        }
                    }
                    Err(ClidoError::Interrupted) => {
                        auto_continue_count = 0;
                        session_exit = "interrupted";
                        let _ = event_tx.send(AgentEvent::Interrupted);
                    }
                    Err(ClidoError::MaxTurnsExceeded) => {
                        auto_continue_count += 1;
                        if auto_continue_count <= MAX_AUTO_CONTINUES {
                            // Silently inject a continue prompt — the agent picks up from
                            // exactly where it left off since history is intact.
                            let _ = event_tx.send(AgentEvent::Thinking(
                                "↻ Continuing (turn limit reached)…".to_string(),
                            ));
                            // Heartbeat also covers the auto-continue LLM call.
                            let hb_tx2 = event_tx.clone();
                            let hb2 = tokio::spawn(async move {
                                let mut iv =
                                    tokio::time::interval(std::time::Duration::from_secs(15));
                                iv.tick().await;
                                loop {
                                    iv.tick().await;
                                    if hb_tx2.send(AgentEvent::Heartbeat).is_err() {
                                        break;
                                    }
                                }
                            });
                            // Call run_next_turn directly with a continue message.
                            let continue_result = agent
                                .run_next_turn(
                                    "Please continue where you left off.",
                                    Some(&mut writer),
                                    Some(&setup.pricing_table),
                                    Some(cancel.clone()),
                                )
                                .await;
                            hb2.abort();
                            let _ = event_tx.send(AgentEvent::TokenUsage {
                                input_tokens: agent.cumulative_input_tokens,
                                output_tokens: agent.cumulative_output_tokens,
                                cost_usd: agent.cumulative_cost_usd,
                                context_max_tokens,
                            });
                            match continue_result {
                                Ok(text) => {
                                    auto_continue_count = 0;
                                    let _ = event_tx.send(AgentEvent::Response(text));
                                }
                                Err(ClidoError::Interrupted) => {
                                    auto_continue_count = 0;
                                    session_exit = "interrupted";
                                    let _ = event_tx.send(AgentEvent::Interrupted);
                                }
                                Err(e) => {
                                    session_exit = "error";
                                    let _ = event_tx.send(AgentEvent::Err(e.to_string()));
                                }
                            }
                        } else {
                            // Hard cap hit: surface a friendly, actionable message.
                            session_exit = "error";
                            let _ = event_tx.send(AgentEvent::Err(format!(
                                "Reached the turn limit {} times without finishing.\n\
                                 History is intact — type \"continue\" to keep going,\n\
                                 or start a new task.",
                                MAX_AUTO_CONTINUES
                            )));
                            auto_continue_count = 0; // reset so next message works
                        }
                    }
                    Err(ClidoError::BudgetExceeded) => {
                        // Show a warning in chat but don't block — user can keep going
                        // by sending another message. Remove or raise --max-budget-usd to silence.
                        let _ = event_tx.send(AgentEvent::Response(
                            "  ⚠ budget limit reached (set via --max-budget-usd or config). \
                             You can keep sending messages; raise or remove the limit to suppress this warning."
                                .to_string(),
                        ));
                    }
                    Err(e) => {
                        session_exit = "error";
                        let _ = event_tx.send(AgentEvent::Err(e.to_string()));
                    }
                }

                let _ = writer.write_line(&clido_storage::SessionLine::Result {
                    exit_status: session_exit.to_string(),
                    total_cost_usd: agent.cumulative_cost_usd,
                    num_turns: agent.turn_count(),
                    duration_ms: 0,
                });
            }
            AgentAction::Resume(resume_session_id) => {
                match clido_storage::SessionReader::load(&workspace_root, &resume_session_id) {
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!("resume failed: {}", e)));
                    }
                    Ok(lines) => {
                        let new_history = clido_agent::session_lines_to_messages(&lines);
                        agent.replace_history(new_history);
                        // Switch the writer to append to the resumed session.
                        match SessionWriter::append(&workspace_root, &resume_session_id) {
                            Ok(new_writer) => {
                                writer = new_writer;
                            }
                            Err(e) => {
                                let _ =
                                    event_tx.send(AgentEvent::Err(format!("resume writer: {}", e)));
                            }
                        }
                        // Collect display messages for the TUI (user + assistant).
                        let mut msgs: Vec<(String, String)> = Vec::new();
                        for line in &lines {
                            match line {
                                clido_storage::SessionLine::UserMessage { content, .. } => {
                                    if let Some(t) = content
                                        .first()
                                        .and_then(|c| c.get("text"))
                                        .and_then(|v| v.as_str())
                                    {
                                        msgs.push(("user".to_string(), t.to_string()));
                                    }
                                }
                                clido_storage::SessionLine::AssistantMessage { content } => {
                                    let text: String = content
                                        .iter()
                                        .filter_map(|c| {
                                            if c.get("type").and_then(|v| v.as_str())
                                                == Some("text")
                                            {
                                                c.get("text")
                                                    .and_then(|v| v.as_str())
                                                    .map(|s| s.to_string())
                                            } else {
                                                None
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join("");
                                    if !text.trim().is_empty() {
                                        msgs.push(("assistant".to_string(), text));
                                    }
                                }
                                _ => {}
                            }
                        }
                        first_turn = false; // already have history
                        let _ = event_tx.send(AgentEvent::ResumedSession { messages: msgs });
                    }
                }
            }
        }
    }

    let _ = writer.flush();
}

// ── Model list builder ────────────────────────────────────────────────────────

/// Build the full sorted model list from the pricing table, roles config, and user prefs.
/// Order: favorites → recent → rest (alphabetical by id within each group).
pub(super) fn build_model_list(
    pricing: &clido_core::PricingTable,
    roles: &std::collections::HashMap<String, String>,
    prefs: &clido_core::ModelPrefs,
) -> Vec<ModelEntry> {
    use std::collections::HashMap;

    // Invert roles map: model_id → role name (use first role found if multiple).
    let mut model_to_role: HashMap<String, String> = HashMap::new();
    for (role, model_id) in roles {
        model_to_role
            .entry(model_id.clone())
            .or_insert_with(|| role.clone());
    }
    for (role, model_id) in &prefs.roles {
        model_to_role
            .entry(model_id.clone())
            .or_insert_with(|| role.clone());
    }

    // Build all entries from the pricing table.
    let mut all: Vec<ModelEntry> = pricing
        .models
        .values()
        .map(|e| ModelEntry {
            id: e.name.clone(),
            provider: e.provider.clone(),
            input_mtok: e.input_per_mtok,
            output_mtok: e.output_per_mtok,
            context_k: e.context_window.map(|w| w / 1000),
            role: model_to_role.get(&e.name).cloned(),
            is_favorite: prefs.is_favorite(&e.name),
        })
        .collect();

    all.sort_by(|a, b| a.id.cmp(&b.id));

    // Partition into three groups.
    let mut favorites: Vec<ModelEntry> = all.iter().filter(|m| m.is_favorite).cloned().collect();
    let recent_ids: Vec<&str> = prefs.recent.iter().map(|s| s.as_str()).collect();
    let mut recent: Vec<ModelEntry> = recent_ids
        .iter()
        .filter_map(|id| all.iter().find(|m| m.id == *id && !m.is_favorite))
        .cloned()
        .collect();
    let fav_or_recent: std::collections::HashSet<&str> = favorites
        .iter()
        .chain(recent.iter())
        .map(|m| m.id.as_str())
        .collect();
    let mut rest: Vec<ModelEntry> = all
        .iter()
        .filter(|m| !fav_or_recent.contains(m.id.as_str()))
        .cloned()
        .collect();

    favorites.sort_by(|a, b| a.id.cmp(&b.id));
    rest.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = favorites;
    out.append(&mut recent);
    out.append(&mut rest);
    out
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub(crate) fn run_tui(
    cli: Cli,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), anyhow::Error>> + Send>> {
    Box::pin(run_tui_inner(cli))
}

pub(super) struct AgentRuntimeHandles {
    prompt_tx: mpsc::UnboundedSender<String>,
    resume_tx: mpsc::UnboundedSender<String>,
    model_switch_tx: mpsc::UnboundedSender<String>,
    workdir_tx: mpsc::UnboundedSender<std::path::PathBuf>,
    compact_now_tx: mpsc::UnboundedSender<()>,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    perm_rx: mpsc::UnboundedReceiver<PermRequest>,
    /// Clone of the event_tx channel — lets code outside the agent task send events to the TUI.
    fetch_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Shared todo list written by the agent's TodoWrite tool — readable by the TUI.
    todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
    /// JoinHandle for the agent background task — aborted on TUI exit to prevent zombies.
    agent_handle: tokio::task::JoinHandle<()>,
}

pub(super) fn start_agent_runtime(
    cli: Cli,
    workspace_root: std::path::PathBuf,
    preloaded_config: Option<clido_core::LoadedConfig>,
    preloaded_pricing: clido_core::PricingTable,
    cancel: Arc<AtomicBool>,
    image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
    reviewer_enabled: Arc<AtomicBool>,
) -> AgentRuntimeHandles {
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<String>();
    let (resume_tx, resume_rx) = mpsc::unbounded_channel::<String>();
    let (model_switch_tx, model_switch_rx) = mpsc::unbounded_channel::<String>();
    let (workdir_tx, workdir_rx) = mpsc::unbounded_channel::<std::path::PathBuf>();
    let (compact_now_tx, compact_now_rx) = mpsc::unbounded_channel::<()>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (perm_tx, perm_rx) = mpsc::unbounded_channel::<PermRequest>();

    // Pre-create the shared todo store so both the agent task and the TUI app can share it.
    let todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let agent_handle = tokio::spawn(agent_task(
        cli,
        workspace_root,
        preloaded_config,
        preloaded_pricing,
        prompt_rx,
        resume_rx,
        model_switch_rx,
        workdir_rx,
        compact_now_rx,
        event_tx.clone(),
        perm_tx,
        cancel,
        image_state,
        reviewer_enabled,
        todo_store.clone(),
    ));

    AgentRuntimeHandles {
        prompt_tx,
        resume_tx,
        model_switch_tx,
        workdir_tx,
        compact_now_tx,
        event_rx,
        perm_rx,
        fetch_tx: event_tx,
        todo_store,
        agent_handle,
    }
}

/// Spawn a background task to fetch the model list from the provider API.
/// Results arrive via `AgentEvent::ModelsLoaded` on the given channel.
pub(super) fn spawn_model_fetch(
    provider: String,
    api_key: String,
    base_url: Option<String>,
    tx: mpsc::UnboundedSender<AgentEvent>,
) {
    tokio::spawn(async move {
        let base_url_ref = base_url.as_deref();
        let entries =
            clido_providers::fetch_provider_models(&provider, &api_key, base_url_ref).await;
        let ids: Vec<String> = entries
            .into_iter()
            .filter(|m| m.available)
            .map(|m| m.id)
            .collect();
        let _ = tx.send(AgentEvent::ModelsLoaded(ids));
    });
}

pub(super) async fn run_tui_inner(cli: Cli) -> Result<(), anyhow::Error> {
    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));

    // Prune session files older than 30 days in the background (non-fatal).
    {
        let wr = workspace_root.clone();
        tokio::task::spawn_blocking(move || {
            let _ = clido_storage::prune_old_sessions(&wr, 30);
        });
    }

    // Load config, pricing table, and model prefs concurrently to minimise startup latency.
    let wr = workspace_root.clone();
    let (config_res, pricing_res, prefs_res) = tokio::join!(
        tokio::task::spawn_blocking(move || clido_core::load_config(&wr)),
        tokio::task::spawn_blocking(clido_core::load_pricing),
        tokio::task::spawn_blocking(clido_core::ModelPrefs::load),
    );
    let loaded_config: Option<clido_core::LoadedConfig> = config_res.ok().and_then(|r| r.ok());
    let (pricing_table, _) =
        pricing_res.unwrap_or_else(|_| (clido_core::PricingTable::default(), None));
    let model_prefs = prefs_res.unwrap_or_else(|_| clido_core::ModelPrefs::default());

    // Derive provider + model from the loaded config (mirrors read_provider_model logic).
    let (provider, model, api_key, base_url) = {
        let profile_name = cli.profile.as_deref().unwrap_or_else(|| {
            loaded_config
                .as_ref()
                .map(|c| c.default_profile.as_str())
                .unwrap_or("default")
        });
        match loaded_config
            .as_ref()
            .and_then(|c| c.get_profile(profile_name).ok())
        {
            Some(profile) => {
                let model = cli.model.clone().unwrap_or_else(|| profile.model.clone());
                let provider = cli
                    .provider
                    .clone()
                    .unwrap_or_else(|| profile.provider.clone());
                // Resolve the API key: direct value takes priority over env var.
                let key = profile
                    .api_key
                    .clone()
                    .or_else(|| {
                        profile
                            .api_key_env
                            .as_deref()
                            .and_then(|e| std::env::var(e).ok())
                    })
                    .unwrap_or_default();
                (provider, model, key, profile.base_url.clone())
            }
            None => ("?".to_string(), "?".to_string(), String::new(), None),
        }
    };

    // Resolve notify setting: CLI flags take priority over config.
    let notify_enabled = if cli.no_notify {
        false
    } else if cli.notify {
        true
    } else {
        loaded_config
            .as_ref()
            .map(|c| c.agent.notify)
            .unwrap_or(false)
    };

    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));

    // Reviewer toggle: shared between App (TUI control) and SpawnReviewerTool (enforcement).
    // Derive initial state from config: enabled by default if reviewer is configured.
    let reviewer_configured = loaded_config
        .as_ref()
        .map(|c| c.agents.reviewer.is_some())
        .unwrap_or(false);
    let reviewer_enabled = Arc::new(AtomicBool::new(true));
    let mut runtime = start_agent_runtime(
        cli.clone(),
        workspace_root.clone(),
        loaded_config.clone(),
        pricing_table.clone(),
        cancel.clone(),
        image_state.clone(),
        reviewer_enabled.clone(),
    );

    // Install a panic hook so the terminal is always restored even on crash.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stderr(), LeaveAlternateScreen);
        #[cfg(unix)]
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) == 0 && t.c_lflag & libc::ICANON == 0 {
                t.c_iflag |= (libc::ICRNL | libc::IXON) as libc::tcflag_t;
                t.c_oflag |= (libc::OPOST | libc::ONLCR) as libc::tcflag_t;
                t.c_lflag |= (libc::ICANON
                    | libc::ECHO
                    | libc::ECHOE
                    | libc::ECHOK
                    | libc::ISIG
                    | libc::IEXTEN) as libc::tcflag_t;
                libc::tcsetattr(0, libc::TCSAFLUSH, &t);
            }
        }
        original_hook(info);
    }));

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let plan_dry_run = cli.plan_dry_run;

    // Build model list from already-loaded config + pricing (no extra disk I/O needed).
    let (config_roles, known_models, current_profile) = {
        let roles = loaded_config
            .as_ref()
            .map(|c| c.roles.as_map())
            .unwrap_or_default();
        let profile = cli
            .profile
            .clone()
            .or_else(|| loaded_config.as_ref().map(|c| c.default_profile.clone()))
            .unwrap_or_else(|| "default".to_string());
        let models = build_model_list(&pricing_table, &roles, &model_prefs);
        (roles, models, profile)
    };

    let mut app = App::new(
        runtime.prompt_tx.clone(),
        runtime.resume_tx.clone(),
        runtime.model_switch_tx.clone(),
        runtime.workdir_tx.clone(),
        runtime.compact_now_tx.clone(),
        cancel,
        provider.clone(),
        model,
        workspace_root.clone(),
        notify_enabled,
        image_state,
        plan_dry_run,
        known_models,
        model_prefs,
        config_roles,
        current_profile,
        reviewer_enabled,
        reviewer_configured,
        runtime.todo_store.clone(),
        api_key.clone(),
        base_url.clone(),
        runtime.fetch_tx.clone(),
    );
    // Kick off a live model-list fetch from the provider API immediately at startup.
    // Results arrive as AgentEvent::ModelsLoaded and update app.known_models.
    if !api_key.is_empty() {
        spawn_model_fetch(
            provider.clone(),
            api_key.clone(),
            base_url.clone(),
            runtime.fetch_tx.clone(),
        );
        app.models_loading = true;
    }
    let mut recovery_attempts: u8 = 0;
    let result = loop {
        match event_loop(
            &mut app,
            &mut terminal,
            &mut runtime.event_rx,
            &mut runtime.perm_rx,
        )
        .await?
        {
            EventLoopExit::Quit => break Ok(()),
            EventLoopExit::ProfileSwitch(profile_name) => {
                // Switch active profile on disk.
                if let Some(config_path) = clido_core::global_config_path() {
                    let _ = clido_core::switch_active_profile(&config_path, &profile_name);
                }

                // Reload config from disk.
                let wr = workspace_root.clone();
                let fresh_config: Option<clido_core::LoadedConfig> =
                    tokio::task::spawn_blocking(move || clido_core::load_config(&wr).ok())
                        .await
                        .ok()
                        .flatten();

                // Extract new profile settings.
                let (new_provider, new_model, new_api_key, new_base_url) = {
                    let pname = profile_name.as_str();
                    match fresh_config
                        .as_ref()
                        .and_then(|c| c.get_profile(pname).ok())
                    {
                        Some(profile) => {
                            let key = profile
                                .api_key
                                .clone()
                                .or_else(|| {
                                    profile
                                        .api_key_env
                                        .as_deref()
                                        .and_then(|e| std::env::var(e).ok())
                                })
                                .unwrap_or_default();
                            (
                                profile.provider.clone(),
                                profile.model.clone(),
                                key,
                                profile.base_url.clone(),
                            )
                        }
                        None => ("?".to_string(), "?".to_string(), String::new(), None),
                    }
                };

                // Abort old agent runtime.
                runtime.agent_handle.abort();

                // Start fresh agent runtime with updated config.
                let mut switch_cli = cli.clone();
                switch_cli.profile = Some(profile_name.clone());
                if let Some(sid) = app.current_session_id.as_deref() {
                    switch_cli.resume = Some(sid.to_string());
                }

                runtime = start_agent_runtime(
                    switch_cli,
                    workspace_root.clone(),
                    fresh_config.clone().or_else(|| loaded_config.clone()),
                    pricing_table.clone(),
                    app.cancel.clone(),
                    app.image_state.clone(),
                    app.reviewer_enabled.clone(),
                );

                // Update app state in-place.
                app.provider = new_provider.clone();
                app.model = new_model.clone();
                app.api_key = new_api_key.clone();
                app.base_url = new_base_url.clone();
                app.current_profile = profile_name.clone();
                app.prompt_tx = runtime.prompt_tx.clone();
                app.resume_tx = runtime.resume_tx.clone();
                app.model_switch_tx = runtime.model_switch_tx.clone();
                app.workdir_tx = runtime.workdir_tx.clone();
                app.compact_now_tx = runtime.compact_now_tx.clone();
                app.quit = false;
                app.busy = false;
                app.status_log.clear();
                app.cancel.store(false, Ordering::Relaxed);

                // Kick off model list fetch for new provider.
                if !new_api_key.is_empty() {
                    spawn_model_fetch(
                        new_provider,
                        new_api_key,
                        new_base_url,
                        runtime.fetch_tx.clone(),
                    );
                    app.models_loading = true;
                }

                app.push(ChatLine::Info(format!(
                    "  switched to profile '{profile_name}'"
                )));
                recovery_attempts = 0;
                continue;
            }
            EventLoopExit::Recover(reason) => {
                recovery_attempts = recovery_attempts.saturating_add(1);
                if recovery_attempts > 3 {
                    break Err(anyhow::anyhow!(
                        "agent recovery failed after {} attempts: {}",
                        recovery_attempts - 1,
                        reason
                    ));
                }
                let backoff_ms = 300u64.saturating_mul(1u64 << (recovery_attempts - 1));
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;

                // Re-use the current session so the recovered agent picks up the full
                // conversation history from disk, preventing context amnesia.
                let mut recovery_cli = cli.clone();
                if let Some(sid) = app.current_session_id.as_deref() {
                    recovery_cli.resume = Some(sid.to_string());
                    app.recovering = true;
                }

                runtime = start_agent_runtime(
                    recovery_cli,
                    workspace_root.clone(),
                    loaded_config.clone(),
                    pricing_table.clone(),
                    app.cancel.clone(),
                    app.image_state.clone(),
                    app.reviewer_enabled.clone(),
                );
                app.prompt_tx = runtime.prompt_tx.clone();
                app.resume_tx = runtime.resume_tx.clone();
                app.model_switch_tx = runtime.model_switch_tx.clone();
                app.workdir_tx = runtime.workdir_tx.clone();
                app.compact_now_tx = runtime.compact_now_tx.clone();
                app.push(ChatLine::Thinking("↻ recovering runtime…".to_string()));
                app.busy = false;
                app.status_log.clear();
                app.cancel.store(false, Ordering::Relaxed);
                app.drain_input_queue();
                continue;
            }
        }
    };

    // Abort the agent background task to prevent zombies after TUI exits.
    runtime.agent_handle.abort();

    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );

    // Handle /profile <name> switch request.
    if let Some(profile_name) = app.wants_profile_switch.take() {
        if let Some(config_path) = clido_core::global_config_path() {
            let _ = clido_core::switch_active_profile(&config_path, &profile_name);
        }
        let mut next_cli = cli.clone();
        next_cli.resume = app.restart_resume_session.take();
        return run_tui(next_cli).await;
    }

    // Handle /profile new — create a new profile via the guided wizard, then restart TUI.
    if app.wants_profile_create {
        crate::setup::run_create_profile(None).await?;
        return run_tui(cli).await;
    }

    // Handle /profile edit <name> — edit existing profile via wizard, then restart TUI.
    if let Some(profile_name) = app.wants_profile_edit.take() {
        let entry = clido_core::load_config(&workspace_root)
            .ok()
            .and_then(|c| c.profiles.get(&profile_name).cloned());
        if let Some(entry) = entry {
            crate::setup::run_edit_profile(profile_name, entry).await?;
        } else {
            tracing::warn!(profile = %profile_name, "profile not found for /profile edit");
        }
        let mut next_cli = cli.clone();
        next_cli.resume = app.restart_resume_session.take();
        return run_tui(next_cli).await;
    }

    // Handle /init reinit request.
    if app.wants_reinit {
        let pre_fill = {
            let loaded = clido_core::load_config(&workspace_root).ok();
            let profile = loaded
                .as_ref()
                .map(|c| c.default_profile.clone())
                .unwrap_or_else(|| "default".to_string());
            let prof_entry = loaded
                .as_ref()
                .and_then(|c| c.profiles.get(&profile).cloned());
            let api_key = prof_entry
                .as_ref()
                .and_then(|p| {
                    p.api_key
                        .clone()
                        .or_else(|| p.api_key_env.as_ref().and_then(|e| std::env::var(e).ok()))
                })
                .unwrap_or_default();
            let roles: Vec<(String, String)> = loaded
                .as_ref()
                .map(|c| {
                    let mut v: Vec<(String, String)> = c.roles.as_map().into_iter().collect();
                    v.sort_by(|a, b| a.0.cmp(&b.0));
                    v
                })
                .unwrap_or_default();
            crate::setup::SetupPreFill {
                provider: app.provider.clone(),
                api_key,
                model: app.model.clone(),
                roles,
                profile_name: String::new(),
                is_new_profile: false,
                saved_api_keys: Vec::new(),
            }
        };
        // Run setup wizard with current values pre-filled.
        crate::setup::run_reinit(pre_fill).await?;
        // Re-launch the TUI.
        let cli_for_reinit = cli.clone();
        return run_tui(cli_for_reinit).await;
    }

    result
}

pub(super) enum EventLoopExit {
    Quit,
    Recover(String),
    /// Switch to a different profile without restarting the TUI.
    ProfileSwitch(String),
}

pub(super) async fn event_loop(
    app: &mut App,
    terminal: &mut ratatui::Terminal<CrosstermBackend<std::io::Stdout>>,
    event_rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
    perm_rx: &mut mpsc::UnboundedReceiver<PermRequest>,
) -> Result<EventLoopExit, anyhow::Error> {
    let mut crossterm_events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    let mut last_agent_activity = std::time::Instant::now();
    // Stall timeout: trigger recovery only if truly no activity (heartbeats keep this fresh
    // during long LLM calls, so 120 s is a reliable hard ceiling for genuinely hung agents).
    const STALL_TIMEOUT_SECS: u64 = 120;
    // Only redraw when state has actually changed to reduce CPU usage.
    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|f| render(f, app))?;
            dirty = false;
        }

        tokio::select! {
            _ = tick.tick() => {
                // Only mark dirty when spinner is actually animating.
                if app.busy || app.pending_perm.is_some() {
                    app.tick_spinner();
                    dirty = true;
                }
                if app.busy && app.pending_perm.is_none() {
                    let baseline = if let Some(turn_start) = app.turn_start {
                        if turn_start > last_agent_activity {
                            turn_start
                        } else {
                            last_agent_activity
                        }
                    } else {
                        last_agent_activity
                    };
                    if baseline.elapsed().as_secs() >= STALL_TIMEOUT_SECS {
                        return Ok(EventLoopExit::Recover(
                            "agent appears stalled (no progress events)".to_string(),
                        ));
                    }
                }
            }
            maybe = crossterm_events.next() => {
                dirty = true;
                match maybe {
                    Some(Ok(Event::Key(key))) => {
                        // Ctrl+L: force a full terminal redraw (screen recovery).
                        if key.modifiers == KeyModifiers::CONTROL
                            && key.code == KeyCode::Char('l')
                        {
                            let _ = terminal.clear();
                        } else {
                            handle_key(app, key);
                        }
                    }
                    Some(Ok(Event::Paste(mut text))) => {
                        // Normalise line endings to \n but preserve newlines so users can
                        // paste multiline markdown without it collapsing into a single line.
                        text = text.replace("\r\n", "\n").replace('\r', "\n");
                        if text.is_empty() {
                            // nothing to do
                        } else if let Some(ref mut ed) = app.plan_text_editor {
                            // Route paste into plan text editor at cursor position.
                            let line = &mut ed.lines[ed.cursor_row];
                            let byte_pos = line
                                .char_indices()
                                .nth(ed.cursor_col)
                                .map(|(i, _)| i)
                                .unwrap_or(line.len());
                            // Only insert first line (plan editor is line-based)
                            let paste_line = text.lines().next().unwrap_or(&text);
                            line.insert_str(byte_pos, paste_line);
                            ed.cursor_col += paste_line.chars().count();
                        } else if app.overlay_stack.handle_paste(&text) {
                            // Overlay stack consumed the paste
                        } else if let Some(ref mut ov) = app.profile_overlay {
                            // Route paste into the active profile overlay text input.
                            // Provider/model picker steps don't accept free-text paste.
                            let accepts_text = matches!(
                                &ov.mode,
                                ProfileOverlayMode::Creating {
                                    step: ProfileCreateStep::Name | ProfileCreateStep::ApiKey,
                                } | ProfileOverlayMode::EditField(_)
                            );
                            if accepts_text {
                                // Strip newlines from API keys
                                let clean = if matches!(
                                    &ov.mode,
                                    ProfileOverlayMode::Creating {
                                        step: ProfileCreateStep::ApiKey,
                                    } | ProfileOverlayMode::EditField(ProfileEditField::ApiKey)
                                ) {
                                    text.lines().collect::<Vec<_>>().join("")
                                } else {
                                    text.clone()
                                };
                                let b = char_byte_pos_tui(&ov.input, ov.input_cursor);
                                ov.input.insert_str(b, &clean);
                                ov.input_cursor += clean.chars().count();
                            }
                        } else {
                            let byte_pos = char_byte_pos(&app.text_input.text, app.text_input.cursor);
                            app.text_input.text.insert_str(byte_pos, &text);
                            app.text_input.cursor += text.chars().count();
                            app.selected_cmd = None;
                            app.text_input.history_idx = None;
                        }
                    }
                    Some(Ok(Event::Mouse(m))) => {
                        match m.kind {
                            MouseEventKind::ScrollDown => scroll_down(app, 3),
                            MouseEventKind::ScrollUp => scroll_up(app, 3),
                            _ => {}
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        // Force a clean redraw after terminal resize to avoid stale cells.
                        // Preserve scroll ratio so user doesn't lose their place.
                        let ratio = if app.max_scroll > 0 && !app.following {
                            Some(app.scroll as f64 / app.max_scroll as f64)
                        } else {
                            None
                        };
                        let _ = terminal.clear();
                        // Width changed — render cache is now stale (line-wrapping differs).
                        app.render_cache.clear();
                        app.render_cache_msg_count = 0;
                        // Restore approximate scroll position after redraw recalculates max_scroll.
                        // The actual clamping is done in render_frame when max_scroll is recomputed.
                        app.pending_scroll_ratio = ratio;
                    }
                    None => break,
                    _ => {}
                }
            }
            maybe = event_rx.recv() => {
                dirty = true;
                match maybe {
                    Some(AgentEvent::ToolStart {
                        tool_use_id,
                        name,
                        detail,
                    }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.push_status(
                            tool_use_id.clone(),
                            name.clone(),
                            detail.clone(),
                        );
                        app.push(ChatLine::ToolCall {
                            tool_use_id,
                            name,
                            detail,
                            done: false,
                            is_error: false,
                        });
                    }
                    Some(AgentEvent::ToolDone {
                        tool_use_id,
                        is_error,
                        diff,
                        ..
                    }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.finish_status(&tool_use_id, is_error);
                        for line in app.messages.iter_mut() {
                            if let ChatLine::ToolCall {
                                tool_use_id: tid,
                                done,
                                is_error: e,
                                ..
                            } = line
                            {
                                if tid == &tool_use_id && !*done {
                                    *done = true;
                                    *e = is_error;
                                    break;
                                }
                            }
                        }
                        if let Some(d) = diff {
                            if !d.is_empty() {
                                app.push(ChatLine::Diff(d));
                            }
                        }
                    }
                    Some(AgentEvent::Thinking(text)) => {
                        last_agent_activity = std::time::Instant::now();
                        if let Some((num, step)) = extract_current_step_full(&text) {
                            app.current_step = Some(step);
                            app.last_executed_step_num = Some(num);
                        }
                        app.push(ChatLine::Thinking(text));
                        // Don't call on_agent_done — the agent is still running.
                    }
                    Some(AgentEvent::Response(text)) => {
                        last_agent_activity = std::time::Instant::now();
                        if let Some((num, step)) = extract_current_step_full(&text) {
                            app.current_step = Some(step);
                            app.last_executed_step_num = Some(num);
                        }
                        app.push(ChatLine::Assistant(text));
                        // Fire desktop notification + bell if enabled.
                        if app.notify_enabled {
                            let elapsed = app
                                .turn_start
                                .map(|s| s.elapsed().as_secs())
                                .unwrap_or(0);
                            let session_id = app
                                .current_session_id
                                .as_deref()
                                .unwrap_or("unknown");
                            crate::notify::notify_done(
                                session_id,
                                elapsed,
                                app.session_total_cost_usd,
                            );
                        }
                        // Revert per-turn model override if active.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.model_switch_tx.send(prev.clone());
                            app.push(ChatLine::Info(format!(
                                "  ↻ Model restored to {}",
                                prev
                            )));
                        }
                        app.on_agent_done();
                    }
                    Some(AgentEvent::ModelSwitched { to_model }) => {
                        last_agent_activity = std::time::Instant::now();
                        // Confirmation from agent_task that the model was switched.
                        // Update display model in case it diverged.
                        app.model = to_model;
                    }
                    Some(AgentEvent::WorkdirSwitched { path }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.workspace_root = path.clone();
                        // Reset the app-side AllowAll override — the agent's override was
                        // already reset in agent_task when replace_tools was called.
                        app.permission_mode_override = None;
                        app.push(ChatLine::Info(format!("  ✓ Working directory: {}", path.display())));
                        app.push(ChatLine::Info(
                            "  Permission grants were reset for safety after the switch."
                                .into(),
                        ));
                        app.push(ChatLine::Info(
                            "  Note: session history stays on the original project until restart."
                                .into(),
                        ));
                    }
                    Some(AgentEvent::SessionStarted(id)) => {
                        last_agent_activity = std::time::Instant::now();
                        app.current_session_id = Some(id);
                    }
                    Some(AgentEvent::Interrupted) => {
                        last_agent_activity = std::time::Instant::now();
                        // Revert per-turn model override on interruption too.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.model_switch_tx.send(prev);
                        }
                        app.on_agent_done();
                    }
                    Some(AgentEvent::Err(msg)) => {
                        last_agent_activity = std::time::Instant::now();
                        // Revert per-turn model override on error too.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.model_switch_tx.send(prev);
                        }
                        app.overlay_stack.push(OverlayKind::Error(
                            ErrorOverlay::from_message(msg),
                        ));
                        app.on_agent_done();
                    }
                    Some(AgentEvent::ResumedSession { messages }) => {
                        last_agent_activity = std::time::Instant::now();
                        if app.recovering {
                            // Silent recovery resume: the agent has restored its internal history
                            // from the session on disk. Preserve the current TUI display and just
                            // show a brief note so the user knows context was restored.
                            app.recovering = false;
                            app.push(ChatLine::Info(
                                "  ✓ context restored — please re-send your last message".into(),
                            ));
                        } else {
                            // Explicit /resume or startup resume: clear and replay.
                            app.messages.clear();
                            app.messages.push(ChatLine::WelcomeBrand);
                            let user_turns = messages.iter().filter(|(r, _)| r == "user").count();
                            let turn_label = if user_turns == 1 { "1 message".to_string() } else { format!("{} messages", user_turns) };
                            app.messages.push(ChatLine::Info(format!("  ↺ Session resumed — {}", turn_label)));
                            for (role, text) in messages {
                                if role == "user" {
                                    app.push(ChatLine::User(text));
                                } else if role == "assistant" {
                                    app.push(ChatLine::Assistant(text));
                                }
                            }
                        }
                        app.busy = false;
                    }
                    Some(AgentEvent::TokenUsage { input_tokens, output_tokens, cost_usd, context_max_tokens }) => {
                        last_agent_activity = std::time::Instant::now();
                        // Cumulative fields on the agent reset at the start of each Run.
                        // Use delta tracking: if new value < previous, this is a new run.
                        let delta_in = if input_tokens >= app.session_input_tokens {
                            input_tokens - app.session_input_tokens
                        } else {
                            input_tokens // new run — full value is the delta
                        };
                        let delta_out = if output_tokens >= app.session_output_tokens {
                            output_tokens - app.session_output_tokens
                        } else {
                            output_tokens
                        };
                        let delta_cost = if cost_usd >= app.session_cost_usd {
                            cost_usd - app.session_cost_usd
                        } else {
                            cost_usd
                        };
                        app.session_input_tokens = input_tokens;
                        app.session_output_tokens = output_tokens;
                        app.session_cost_usd = cost_usd;
                        app.session_total_input_tokens += delta_in;
                        app.session_total_output_tokens += delta_out;
                        app.session_total_cost_usd += delta_cost;
                        app.context_max_tokens = context_max_tokens;
                    }
                    Some(AgentEvent::Compacted { before, after }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.push(ChatLine::Info(format!(
                            "  ↻ Context compressed: {} → {} messages (older history summarised)",
                            before, after
                        )));
                    }
                    Some(AgentEvent::PlanCreated { tasks }) => {
                        last_agent_activity = std::time::Instant::now();
                        // Display the plan in the chat as an info block.
                        app.push(ChatLine::Info("  ┌─ Plan:".to_string()));
                        let count = tasks.len();
                        for (i, task) in tasks.iter().enumerate() {
                            let prefix = if i + 1 == count { "        └─" } else { "        ├─" };
                            app.push(ChatLine::Info(format!("{} {}", prefix, task)));
                        }
                        // Store last plan so /plan command can show it later.
                        app.last_plan = Some(tasks.clone());
                        app.last_plan_snapshot = build_plan_from_tasks(&tasks);
                    }
                    Some(AgentEvent::PlanReady { plan }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.last_plan_snapshot = Some(plan.clone());
                        app.last_plan = Some(
                            plan.tasks
                                .iter()
                                .map(|t| t.description.clone())
                                .collect::<Vec<_>>(),
                        );
                        // Open the plan editor overlay (blocks execution until user presses x or Esc).
                        app.plan_selected_task = 0;
                        app.plan_task_editing = None;
                        app.plan_editor = Some(PlanEditor::new(plan));
                        // Mark as busy so the spinner shows — agent is paused waiting for plan approval.
                    }
                    Some(AgentEvent::PlanTaskStarted { task_id }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.push(ChatLine::Info(format!("  ↻ Step {} started", task_id)));
                    }
                    Some(AgentEvent::PlanTaskDone { task_id, success }) => {
                        last_agent_activity = std::time::Instant::now();
                        let icon = if success { "✓" } else { "✗" };
                        app.push(ChatLine::Info(format!("  {} Step {} done", icon, task_id)));
                    }
                    Some(AgentEvent::Heartbeat) => {
                        // Silent keep-alive from agent_task during slow LLM responses.
                        // Just refresh the activity timestamp to prevent false stall detection.
                        last_agent_activity = std::time::Instant::now();
                    }
                    Some(AgentEvent::BudgetWarning { percent, cost, limit }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.push(ChatLine::Info(format!(
                            "  ⚠ {}% of budget used (${:.4} / ${:.4})",
                            percent, cost, limit
                        )));
                    }
                    Some(AgentEvent::ModelsLoaded(ids)) => {
                        app.models_loading = false;
                        if !ids.is_empty() {
                            // Merge API-fetched model IDs with existing pricing data.
                            // Keep existing entries (which may have cost metadata) for known IDs,
                            // and add stub entries for newly-discovered ones.
                            let existing: std::collections::HashSet<String> =
                                app.known_models.iter().map(|m| m.id.clone()).collect();
                            for id in &ids {
                                if !existing.contains(id) {
                                    app.known_models.push(ModelEntry {
                                        id: id.clone(),
                                        provider: app.provider.clone(),
                                        input_mtok: 0.0,
                                        output_mtok: 0.0,
                                        context_k: None,
                                        role: None,
                                        is_favorite: false,
                                    });
                                }
                            }
                            // If model picker is open, refresh its list to show the new models.
                            if let Some(picker) = &mut app.model_picker {
                                let all: Vec<String> =
                                    app.known_models.iter().map(|m| m.id.clone()).collect();
                                picker.refresh_models(all);
                            }
                        }
                    }
                    Some(AgentEvent::TitleGenerated(title)) => {
                        app.session_title = Some(title);
                    }
                    None => {
                        return Ok(EventLoopExit::Recover(
                            "agent event channel closed unexpectedly".to_string(),
                        ));
                    }
                }
            }
            maybe = perm_rx.recv() => {
                dirty = true;
                if let Some(req) = maybe {
                    last_agent_activity = std::time::Instant::now();
                    app.pending_perm = Some(PendingPerm {
                        tool_name: req.tool_name,
                        preview: req.preview,
                        reply: req.reply,
                    });
                    // Don't clear busy — agent is still running, awaiting our reply.
                } else {
                    return Ok(EventLoopExit::Recover(
                        "permission channel closed unexpectedly".to_string(),
                    ));
                }
            }
        }

        if app.quit {
            // Check if this is a profile switch rather than a real quit.
            if let Some(profile_name) = app.wants_profile_switch.take() {
                return Ok(EventLoopExit::ProfileSwitch(profile_name));
            }
            return Ok(EventLoopExit::Quit);
        }
    }
    Ok(EventLoopExit::Quit)
}
