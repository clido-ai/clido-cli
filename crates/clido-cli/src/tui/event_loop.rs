use std::io::{stdout, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn trunc_tool_detail(s: &str, max_chars: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max_chars {
        t.to_string()
    } else {
        format!(
            "{}…",
            t.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

/// Convert tool JSON input to a human-readable summary.
fn format_tool_detail(name: &str, input: &str) -> String {
    // Try to parse as JSON for better formatting
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(input) {
        match name {
            "Ls" | "ls" => {
                let path = json.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                let depth = json.get("depth").and_then(|v| v.as_u64());
                let hidden = json
                    .get("show_hidden")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Some(d) = depth {
                    if hidden {
                        format!("{} (depth {}, hidden)", path, d)
                    } else {
                        format!("{} (depth {})", path, d)
                    }
                } else {
                    path.to_string()
                }
            }
            "Read" | "read" => json
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(input)
                .to_string(),
            "Write" | "write" => {
                let path = json
                    .get("file_path")
                    .or(json.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(input);
                path.to_string()
            }
            "Edit" | "edit" => json
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or(input)
                .to_string(),
            "Bash" | "bash" | "Shell" | "shell" => json
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or(input)
                .to_string(),
            "Glob" | "glob" => json
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or(input)
                .to_string(),
            "Grep" | "grep" => {
                let pattern = json.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let path = json.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                format!("{} in {}", pattern, path)
            }
            "SemanticSearch" | "semantic_search" => {
                let q = json
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                let dir = json
                    .get("target_directory")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty());
                let mut s = trunc_tool_detail(q, 72);
                if let Some(d) = dir {
                    s.push_str("  ·  in ");
                    s.push_str(&trunc_tool_detail(d, 36));
                }
                s
            }
            "WebSearch" | "web_search" => json
                .get("query")
                .and_then(|v| v.as_str())
                .map(|q| trunc_tool_detail(q, 80))
                .unwrap_or_else(|| input.chars().take(60).collect()),
            "WebFetch" | "web_fetch" => json
                .get("url")
                .and_then(|v| v.as_str())
                .map(|u| trunc_tool_detail(u, 96))
                .unwrap_or_else(|| input.chars().take(60).collect()),
            "TodoWrite" | "todo_write" => {
                let n = json
                    .get("todos")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                if n == 0 {
                    "todo list".to_string()
                } else {
                    let first = json
                        .get("todos")
                        .and_then(|v| v.as_array())
                        .and_then(|a| a.first())
                        .and_then(|o| o.get("content"))
                        .and_then(|v| v.as_str())
                        .map(|c| trunc_tool_detail(c, 48));
                    match first {
                        Some(f) if !f.is_empty() => format!("{n} items  ·  {f}"),
                        _ => format!("{n} items"),
                    }
                }
            }
            "SpawnWorker" | "spawn_worker" => json
                .get("task")
                .and_then(|v| v.as_str())
                .map(|t| trunc_tool_detail(t, 88))
                .unwrap_or_else(|| "worker task".into()),
            "SpawnReviewer" | "spawn_reviewer" => json
                .get("criteria")
                .and_then(|v| v.as_str())
                .map(|c| trunc_tool_detail(c, 88))
                .unwrap_or_else(|| "review".into()),
            "ApplyPatch" | "apply_patch" => {
                let patch = json.get("patch").and_then(|v| v.as_str()).unwrap_or("");
                let lines = patch.lines().count();
                let file_hint = patch
                    .lines()
                    .find(|l| l.starts_with("+++ "))
                    .map(|l| l.trim_start_matches("+++ ").trim())
                    .filter(|s| !s.is_empty() && *s != "/dev/null");
                match file_hint {
                    Some(f) => format!("{}  ·  {lines} lines", trunc_tool_detail(f, 48)),
                    None => format!("unified diff  ·  {lines} lines"),
                }
            }
            _ => {
                // For unknown tools, show first string value or truncate JSON
                if let Some(obj) = json.as_object() {
                    if let Some((_, v)) = obj.iter().find(|(_, v)| v.is_string()) {
                        v.as_str().unwrap_or(input).to_string()
                    } else {
                        input.chars().take(50).collect()
                    }
                } else {
                    input.chars().take(50).collect()
                }
            }
        }
    } else {
        // Not valid JSON, return as-is but truncated
        input.chars().take(60).collect()
    }
}

use clido_agent::AgentLoop;
use clido_core::ClidoError;
use clido_storage::SessionWriter;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, MouseEventKind,
    },
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
        std::fs::create_dir_all(&data).map_err(|e| format!("create data dir: {e}"))?;
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
            if let Some(mut stdin) = child.stdin.take() {
                if stdin.write_all(text.as_bytes()).is_err() {
                    // fall through to OSC 52 fallback
                } else {
                    drop(stdin);
                    if child.wait().map(|s| s.success()).unwrap_or(false) {
                        return Ok(());
                    }
                }
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
                if let Some(mut stdin) = child.stdin.take() {
                    if stdin.write_all(text.as_bytes()).is_err() {
                        continue; // try next clipboard tool
                    }
                    drop(stdin);
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

/// Read plain text from the system clipboard (inverse of [`copy_to_clipboard`]).
/// Used for Ctrl+V when the terminal does not emit a `Paste` event.
pub(super) fn read_clipboard() -> Result<String, String> {
    use std::process::{Command, Stdio};

    #[cfg(target_os = "macos")]
    {
        let out = Command::new("pbpaste")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("pbpaste: {e}"))?;
        if !out.status.success() {
            return Err("clipboard read failed".into());
        }
        String::from_utf8(out.stdout).map_err(|e| format!("clipboard: {e}"))
    }

    #[cfg(target_os = "linux")]
    {
        for cmd in [
            ["wl-paste", "-n"],
            ["xclip", "-selection", "clipboard", "-o"],
            ["xsel", "--clipboard", "--output"],
        ] {
            if let Ok(out) = Command::new(cmd[0])
                .args(&cmd[1..])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                if out.status.success() && !out.stdout.is_empty() {
                    return String::from_utf8(out.stdout).map_err(|e| format!("clipboard: {e}"));
                }
            }
        }
        return Err("clipboard read failed (install wl-paste, xclip, or xsel)".into());
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("clipboard read not supported on this platform".into())
    }
}

// ── Agent background task ─────────────────────────────────────────────────────

pub(super) enum AgentAction {
    Run(String),
    Resume(String),
    SwitchModel(String),
    SetWorkspace(std::path::PathBuf),
    CompactNow,
    SetAllowedExternalPaths(Vec<std::path::PathBuf>),
    /// Inject a note/hint into the running conversation, interrupting current execution.
    Note(String),
    /// Switch to a different profile while preserving session history.
    SwitchProfile(String),
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn agent_task(
    mut cli: Cli,
    mut workspace_root: std::path::PathBuf,
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
    mut kill_rx: mpsc::UnboundedReceiver<()>,
    mut allowed_paths_rx: mpsc::UnboundedReceiver<Vec<std::path::PathBuf>>,
    mut note_rx: mpsc::UnboundedReceiver<String>,
    path_permission_rx: mpsc::UnboundedReceiver<std::path::PathBuf>,
    mut profile_switch_rx: mpsc::UnboundedReceiver<String>,
) {
    // Session-scoped paths outside workspace (also passed into AgentSetup on rebuild).
    let mut allowed_external_paths: Vec<std::path::PathBuf> = Vec::new();

    let setup_result = match preloaded_config {
        Some(loaded) => AgentSetup::build_with_preloaded_and_store(
            &cli,
            &workspace_root,
            loaded,
            preloaded_pricing,
            reviewer_enabled.clone(),
            Some(todo_store.clone()),
            &allowed_external_paths,
        ),
        None => AgentSetup::build_for_workspace_session(
            &cli,
            &workspace_root,
            reviewer_enabled.clone(),
            todo_store.clone(),
            &allowed_external_paths,
        ),
    };
    let mut setup = match setup_result {
        Ok(s) => s,
        Err(e) => {
            if event_tx.send(AgentEvent::Err(format!("{e}"))).is_err() {
                return;
            }
            return;
        }
    };

    let perms = Arc::new(Mutex::new(PermsState::default()));
    setup.ask_user = Some(Arc::new(TuiAskUser {
        perm_tx: perm_tx.clone(),
        perms: perms.clone(),
    }));

    // Deduplication: check for existing sessions created within the last 5 seconds
    // with the same content to prevent duplicate sessions during rapid recovery.
    // Check for recent session to prevent duplicates during rapid recovery
    let (session_id, writer_result): (String, anyhow::Result<SessionWriter>) = if let Some(id) =
        &cli.resume
    {
        (id.clone(), SessionWriter::append(&workspace_root, id))
    } else {
        // Check if a session was created very recently (within 5 seconds) to prevent
        // duplicate sessions when recovery races or multiple runtimes start simultaneously.
        match clido_storage::find_recent_session(&workspace_root, std::time::Duration::from_secs(5))
        {
            Some(existing_id) => {
                tracing::info!(
                    "Reusing recent session {} instead of creating new",
                    existing_id
                );
                (
                    existing_id.clone(),
                    SessionWriter::append(&workspace_root, &existing_id),
                )
            }
            None => {
                let new_id = uuid::Uuid::new_v4().to_string();
                (
                    new_id.clone(),
                    SessionWriter::create(&workspace_root, &new_id),
                )
            }
        }
    };
    let mut writer = match writer_result {
        Ok(w) => w,
        Err(e) => {
            if event_tx.send(AgentEvent::Err(format!("{e}"))).is_err() {
                return;
            }
            return;
        }
    };
    if event_tx
        .send(AgentEvent::SessionStarted(session_id.clone()))
        .is_err()
    {
        return;
    }

    let emitter: Arc<dyn EventEmitter> = Arc::new(TuiEmitter {
        tx: event_tx.clone(),
    });

    let planner_mode = cli.planner;
    let context_max_tokens = setup.config.max_context_tokens.unwrap_or(200_000) as u64;
    // Capture the utility (fast) provider for async title generation.
    let title_provider = setup
        .fast_provider
        .clone()
        .unwrap_or_else(|| setup.provider.clone());
    let title_model = setup
        .fast_config
        .as_ref()
        .map(|c| c.model.clone())
        .unwrap_or_else(|| setup.config.model.clone());
    let git_workspace_root = Arc::new(Mutex::new(workspace_root.clone()));
    let gwr = git_workspace_root.clone();
    let git_context_fn: Box<dyn Fn() -> Option<String> + Send + Sync> = Box::new(move || {
        let ws = gwr.lock().ok()?;
        GitContext::discover(ws.as_path()).map(|ctx| ctx.to_prompt_section())
    });
    let mut agent = AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user)
        .with_path_permission_receiver(path_permission_rx)
        .with_fast_provider(setup.fast_provider, setup.fast_config)
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
                if event_tx
                    .send(AgentEvent::Err(format!("resume failed: {}", e)))
                    .is_err()
                {
                    return;
                }
            }
            Ok(lines) => {
                let new_history = clido_agent::session_lines_to_messages(&lines);
                agent.replace_history(new_history);
                match SessionWriter::append(&workspace_root, &resume_session_id) {
                    Ok(new_writer) => {
                        writer = new_writer;
                    }
                    Err(e) => {
                        if event_tx
                            .send(AgentEvent::Err(format!("resume writer: {}", e)))
                            .is_err()
                        {
                            return;
                        }
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
                    if event_tx.send(AgentEvent::Thinking(format!(
                        "⚠ Some files referenced in this session have changed since it was recorded:\n{}\n\
                         The agent's context may be stale for these files.",
                        list
                    ))).is_err() { return; }
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
                if event_tx
                    .send(AgentEvent::ResumedSession { messages: msgs })
                    .is_err()
                {
                    return;
                }
            }
        }
    }

    loop {
        // Apply queued workdir changes before other actions so prompts never run
        // against stale tooling/permissions after a switch command.
        while let Ok(new_workspace) = workdir_rx.try_recv() {
            match AgentSetup::build_for_workspace_session(
                &cli,
                &new_workspace,
                reviewer_enabled.clone(),
                todo_store.clone(),
                &allowed_external_paths,
            ) {
                Ok(new_setup) => {
                    workspace_root = new_workspace.clone();
                    if let Ok(mut g) = git_workspace_root.lock() {
                        *g = new_workspace.clone();
                    }
                    agent.replace_tools(new_setup.registry);
                    agent.reset_permission_mode_override();
                    if let Ok(mut state) = perms.lock() {
                        state.clear_all_grants();
                    }
                    if event_tx
                        .send(AgentEvent::WorkdirSwitched {
                            path: new_workspace,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(e) => {
                    if event_tx
                        .send(AgentEvent::Err(format!("workdir switch failed: {}", e)))
                        .is_err()
                    {
                        return;
                    }
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
            profile = profile_switch_rx.recv() => {
                match profile {
                    Some(p) => AgentAction::SwitchProfile(p),
                    None => break,
                }
            }
            _ = kill_rx.recv() => {
                // Kill signal - set cancel flag and let agent finish current tools gracefully
                // This ensures tool results are added to history before stopping
                cancel.store(true, Ordering::Relaxed);
                continue;
            }
            paths = allowed_paths_rx.recv() => {
                match paths {
                    Some(p) => AgentAction::SetAllowedExternalPaths(p),
                    None => break,
                }
            }
            note = note_rx.recv() => {
                match note {
                    Some(n) => AgentAction::Note(n),
                    None => break,
                }
            }
        };

        match action {
            AgentAction::SwitchModel(model_name) => {
                agent.set_model(model_name.clone());
                if event_tx
                    .send(AgentEvent::ModelSwitched {
                        to_model: model_name,
                    })
                    .is_err()
                {
                    return;
                }
            }
            AgentAction::SetWorkspace(new_workspace) => {
                match AgentSetup::build_for_workspace_session(
                    &cli,
                    &new_workspace,
                    reviewer_enabled.clone(),
                    todo_store.clone(),
                    &allowed_external_paths,
                ) {
                    Ok(new_setup) => {
                        workspace_root = new_workspace.clone();
                        if let Ok(mut g) = git_workspace_root.lock() {
                            *g = new_workspace.clone();
                        }
                        agent.replace_tools(new_setup.registry);
                        agent.reset_permission_mode_override();
                        if let Ok(mut state) = perms.lock() {
                            state.clear_all_grants();
                        }
                        if event_tx
                            .send(AgentEvent::WorkdirSwitched {
                                path: new_workspace,
                            })
                            .is_err()
                        {
                            return;
                        }
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
                        if event_tx
                            .send(AgentEvent::Compacted { before, after })
                            .is_err()
                        {
                            return;
                        }
                        // Emit updated token counts so the context bar refreshes.
                        if event_tx
                            .send(AgentEvent::TokenUsage {
                                input_tokens: agent.cumulative_input_tokens,
                                output_tokens: agent.cumulative_output_tokens,
                                cost_usd: agent.cumulative_cost_usd,
                                context_max_tokens,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(e) => {
                        if event_tx
                            .send(AgentEvent::Err(format!("compact: {}", e)))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            AgentAction::SetAllowedExternalPaths(paths) => {
                let path_count = paths.len();
                allowed_external_paths = paths.clone();
                let wr = workspace_root.clone();
                let cli_c = cli.clone();
                let rev = reviewer_enabled.clone();
                let td = todo_store.clone();
                let p = allowed_external_paths.clone();
                let rt = tokio::runtime::Handle::current();
                let reg_res = tokio::task::spawn_blocking(move || {
                    let _guard = rt.enter();
                    crate::agent_setup::regenerate_tool_registry(&cli_c, &wr, rev, td, &p)
                })
                .await;
                match reg_res {
                    Ok(Ok(new_registry)) => {
                        agent.replace_tools(new_registry);
                        if event_tx
                            .send(AgentEvent::Info {
                                message: format!(
                                    "External paths updated: {} path(s) allowed",
                                    path_count
                                ),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(Err(e)) => {
                        let _ = event_tx.send(AgentEvent::Err(format!(
                            "external paths update failed: {e}"
                        )));
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!(
                            "external paths update failed: {e}"
                        )));
                    }
                }
            }
            AgentAction::Note(text) => {
                // Inject the note into agent history as a user message.
                // This interrupts current execution so the note is seen immediately.
                agent.push_user_message(text);
                if event_tx
                    .send(AgentEvent::Info {
                        message: "Note received — agent will see it immediately".to_string(),
                    })
                    .is_err()
                {
                    return;
                }
                // Cancel current execution so agent restarts with the note in context.
                // The cancel flag is checked after tool execution; agent will return
                // Interrupted and the loop will continue, picking up the queued action.
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            AgentAction::SwitchProfile(profile_name) => {
                let wr = workspace_root.clone();
                let loaded_res =
                    tokio::task::spawn_blocking(move || clido_core::load_config(&wr)).await;
                let loaded = match loaded_res.ok().and_then(|r| r.ok()) {
                    Some(l) => l,
                    _ => {
                        let _ = event_tx.send(AgentEvent::Err(
                            "profile switch: could not reload config from disk".into(),
                        ));
                        continue;
                    }
                };
                if !loaded.profiles.contains_key(&profile_name) {
                    let _ = event_tx.send(AgentEvent::Err(format!(
                        "profile switch: profile '{}' not found in config",
                        profile_name
                    )));
                    continue;
                }
                let mut try_cli = cli.clone();
                try_cli.profile = Some(profile_name.clone());
                match AgentSetup::build_with_preloaded_and_store(
                    &try_cli,
                    &workspace_root,
                    loaded,
                    setup.pricing_table.clone(),
                    reviewer_enabled.clone(),
                    Some(todo_store.clone()),
                    &allowed_external_paths,
                ) {
                    Ok(new_setup) => {
                        cli.profile = Some(profile_name.clone());
                        setup.provider_name = new_setup.provider_name.clone();
                        setup.pricing_table = new_setup.pricing_table.clone();
                        setup.fast_provider = new_setup.fast_provider.clone();
                        setup.fast_config = new_setup.fast_config.clone();
                        setup.config = new_setup.config.clone();
                        let switched_model = new_setup.config.model.clone();
                        agent.switch_profile(
                            new_setup.provider,
                            new_setup.config,
                            new_setup.registry,
                        );
                        setup.ask_user = Some(Arc::new(TuiAskUser {
                            perm_tx: perm_tx.clone(),
                            perms: perms.clone(),
                        }));
                        if event_tx
                            .send(AgentEvent::Info {
                                message: format!("Switched to profile '{}'", profile_name),
                            })
                            .is_err()
                        {
                            return;
                        }
                        if event_tx
                            .send(AgentEvent::ModelSwitched {
                                to_model: switched_model,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!("profile switch: {e}")));
                    }
                }
            }
            AgentAction::Run(prompt) => {
                let run_start = std::time::Instant::now();
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
                        if event_tx
                            .send(AgentEvent::TokenUsage {
                                input_tokens: plan_usage.input_tokens,
                                output_tokens: plan_usage.output_tokens,
                                cost_usd: plan_cost,
                                context_max_tokens,
                            })
                            .is_err()
                        {
                            return;
                        }
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
                                if event_tx.send(AgentEvent::PlanReady { plan }).is_err() {
                                    return;
                                }
                                // Mark first_turn as consumed so the next prompt (execution)
                                // does not try to re-plan.
                                first_turn = false;
                                // Do not proceed with agent execution — wait for the user to
                                // approve/edit and press 'x' in the plan editor. The editor's
                                // 'x' key sends a combined prompt via send_now.
                                continue;
                            } else {
                                if event_tx
                                    .send(AgentEvent::PlanCreated {
                                        tasks: task_descriptions,
                                    })
                                    .is_err()
                                {
                                    return;
                                }
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
                if event_tx
                    .send(AgentEvent::TokenUsage {
                        input_tokens: agent.cumulative_input_tokens,
                        output_tokens: agent.cumulative_output_tokens,
                        cost_usd: agent.cumulative_cost_usd,
                        context_max_tokens,
                    })
                    .is_err()
                {
                    return;
                }

                let mut session_exit: &str = "success";

                match result {
                    Ok(text) => {
                        auto_continue_count = 0; // reset on clean completion
                        if event_tx.send(AgentEvent::Response(text.clone())).is_err() {
                            return;
                        }

                        // Generate session title after first successful response.
                        if !title_generated {
                            title_generated = true;
                            let title_prompt = prompt.clone();
                            let title_tx = event_tx.clone();
                            let tp = title_provider.clone();
                            let tm = title_model.clone();
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
                                        if title_tx.send(AgentEvent::TitleGenerated(title)).is_err()
                                        {
                                            tracing::debug!("title channel closed");
                                        }
                                    }
                                }
                            });
                        }
                    }
                    Err(ClidoError::Interrupted) => {
                        auto_continue_count = 0;
                        session_exit = "interrupted";
                        if event_tx.send(AgentEvent::Interrupted).is_err() {
                            return;
                        }
                    }
                    Err(ClidoError::MaxTurnsExceeded) => {
                        auto_continue_count += 1;
                        if auto_continue_count <= MAX_AUTO_CONTINUES {
                            // Silently inject a continue prompt — the agent picks up from
                            // exactly where it left off since history is intact.
                            if event_tx
                                .send(AgentEvent::Thinking(
                                    "↻ Continuing (turn limit reached)…".to_string(),
                                ))
                                .is_err()
                            {
                                return;
                            }
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
                            if event_tx
                                .send(AgentEvent::TokenUsage {
                                    input_tokens: agent.cumulative_input_tokens,
                                    output_tokens: agent.cumulative_output_tokens,
                                    cost_usd: agent.cumulative_cost_usd,
                                    context_max_tokens,
                                })
                                .is_err()
                            {
                                return;
                            }
                            match continue_result {
                                Ok(text) => {
                                    auto_continue_count = 0;
                                    if event_tx.send(AgentEvent::Response(text)).is_err() {
                                        return;
                                    }
                                }
                                Err(ClidoError::Interrupted) => {
                                    auto_continue_count = 0;
                                    session_exit = "interrupted";
                                    if event_tx.send(AgentEvent::Interrupted).is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    session_exit = "error";
                                    if event_tx.send(AgentEvent::Err(format!("{e}"))).is_err() {
                                        return;
                                    }
                                }
                            }
                        } else {
                            // Hard cap hit: surface a friendly, actionable message.
                            session_exit = "error";
                            if event_tx
                                .send(AgentEvent::Err(format!(
                                    "Reached the turn limit {} times without finishing.\n\
                                 History is intact — type \"continue\" to keep going,\n\
                                 or start a new task.",
                                    MAX_AUTO_CONTINUES
                                )))
                                .is_err()
                            {
                                return;
                            }
                            auto_continue_count = 0; // reset so next message works
                        }
                    }
                    Err(ClidoError::BudgetExceeded) => {
                        // Show a warning in chat but don't block — user can keep going
                        // by sending another message. Remove or raise --max-budget-usd to silence.
                        if event_tx.send(AgentEvent::Response(
                            "  ⚠ budget limit reached (set via --max-budget-usd or config). \
                             You can keep sending messages; raise or remove the limit to suppress this warning."
                                .to_string(),
                        )).is_err() { return; }
                    }
                    Err(ClidoError::RateLimited {
                        message,
                        retry_after_secs,
                        is_subscription_limit,
                    }) => {
                        session_exit = "rate_limited";
                        if event_tx
                            .send(AgentEvent::RateLimited {
                                message,
                                retry_after_secs,
                                is_subscription_limit,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(e) => {
                        session_exit = "error";
                        if event_tx.send(AgentEvent::Err(format!("{e}"))).is_err() {
                            return;
                        }
                    }
                }

                let _ = writer.write_line(&clido_storage::SessionLine::Result {
                    exit_status: session_exit.to_string(),
                    total_cost_usd: agent.cumulative_cost_usd,
                    num_turns: agent.turn_count(),
                    duration_ms: run_start.elapsed().as_millis() as u64,
                });
            }
            AgentAction::Resume(resume_session_id) => {
                match clido_storage::SessionReader::load(&workspace_root, &resume_session_id) {
                    Err(e) => {
                        if event_tx
                            .send(AgentEvent::Err(format!("resume failed: {}", e)))
                            .is_err()
                        {
                            return;
                        }
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
                        if event_tx
                            .send(AgentEvent::ResumedSession { messages: msgs })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        }
    }

    let _ = writer.flush();
}

// ── Model list builder ────────────────────────────────────────────────────────

/// Build the full sorted model list from the pricing table and user prefs.
/// Order: favorites → recent → rest (alphabetical by id within each group).
pub(super) fn build_model_list(
    pricing: &clido_core::PricingTable,
    prefs: &clido_core::ModelPrefs,
) -> Vec<ModelEntry> {
    use std::collections::HashMap;

    // Build role map from user prefs.
    let mut model_to_role: HashMap<String, String> = HashMap::new();
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
    /// Channel to force abort the agent task immediately (for /stop command).
    kill_tx: mpsc::UnboundedSender<()>,
    /// Channel to update allowed external paths for this session.
    allowed_paths_tx: mpsc::UnboundedSender<Vec<std::path::PathBuf>>,
    /// Channel to inject a note/hint into the running conversation.
    note_tx: mpsc::UnboundedSender<String>,
    /// Channel to grant permission for external path access (user clicked "Allow").
    path_permission_tx: mpsc::UnboundedSender<std::path::PathBuf>,
    /// Channel to request switching to a different profile seamlessly.
    profile_switch_tx: mpsc::UnboundedSender<String>,
}

/// Resolve the API key for display purposes (welcome screen, etc.).
///
/// Mirrors the resolution order in `provider::make_provider`:
///   1. `profile.api_key` (literal value in config.toml)
///   2. `profile.api_key_env` or the provider's conventional env var
///   3. Credentials file (`~/.config/clido/credentials`)
pub(super) fn resolve_display_api_key(profile: &clido_core::ProfileEntry) -> String {
    // 1. Direct value in config
    if let Some(ref k) = profile.api_key {
        if !k.is_empty() {
            return k.clone();
        }
    }

    let provider_name = profile.provider.as_str();

    // 2. Environment variable (explicit or conventional)
    let env_var = profile
        .api_key_env
        .as_deref()
        .unwrap_or_else(|| crate::provider::default_api_key_env(provider_name));
    if !env_var.is_empty() {
        if let Ok(val) = std::env::var(env_var) {
            if !val.is_empty() {
                return val;
            }
        }
    }

    // 3. Credentials file
    if let Some(dir) = crate::provider::default_config_dir() {
        let creds = crate::provider::load_credentials(&dir);
        if let Some(val) = creds.get(provider_name) {
            if !val.is_empty() {
                return val.clone();
            }
        }
    }

    String::new()
}

/// Refresh header fields after a seamless profile switch (same session).
pub(super) fn sync_tui_profile_from_disk(app: &mut App, profile_name: &str) {
    let Ok(loaded) = clido_core::load_config(&app.workspace_root) else {
        return;
    };
    let Ok(profile) = loaded.get_profile(profile_name) else {
        return;
    };
    app.provider = profile.provider.clone();
    app.model = profile.model.clone();
    app.api_key = resolve_display_api_key(profile);
    app.base_url = profile.base_url.clone();
    app.current_profile = profile_name.to_string();
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
    let (kill_tx, kill_rx) = mpsc::unbounded_channel::<()>();
    let (allowed_paths_tx, allowed_paths_rx) = mpsc::unbounded_channel::<Vec<std::path::PathBuf>>();
    let (note_tx, note_rx) = mpsc::unbounded_channel::<String>();
    let (path_permission_tx, path_permission_rx) = mpsc::unbounded_channel::<std::path::PathBuf>();
    let (profile_switch_tx, profile_switch_rx) = mpsc::unbounded_channel::<String>();

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
        kill_rx,
        allowed_paths_rx,
        note_rx,
        path_permission_rx,
        profile_switch_rx,
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
        kill_tx,
        allowed_paths_tx,
        note_tx,
        path_permission_tx,
        profile_switch_tx,
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
    let provider_for_event = provider.clone();
    tokio::spawn(async move {
        let base_url_ref = base_url.as_deref();
        let entries =
            clido_providers::fetch_provider_models(&provider, &api_key, base_url_ref).await;
        let ids: Vec<String> = entries
            .into_iter()
            .filter(|m| m.available)
            .map(|m| m.id)
            .collect();
        let _ = tx.send(AgentEvent::ModelsLoaded {
            ids,
            provider: provider_for_event,
        });
    });
}

pub(super) async fn run_tui_inner(cli: Cli) -> Result<(), anyhow::Error> {
    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    // Canonicalize for consistent session storage regardless of how the path was specified
    let workspace_root =
        std::fs::canonicalize(&workspace_root).unwrap_or_else(|_| workspace_root.clone());

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
                let key = resolve_display_api_key(profile);
                (provider, model, key, profile.base_url.clone())
            }
            None => ("?".to_string(), "?".to_string(), String::new(), None),
        }
    };

    // Build utility (fast) provider for TUI-initiated background tasks (/enhance, etc.).
    // Falls back to the main provider if no fast provider is configured.
    let (tui_utility_provider, tui_utility_model) = {
        let profile_name = cli.profile.as_deref().unwrap_or_else(|| {
            loaded_config
                .as_ref()
                .map(|c| c.default_profile.as_str())
                .unwrap_or("default")
        });
        let fast_cfg = loaded_config
            .as_ref()
            .and_then(|c| c.get_profile(profile_name).ok())
            .and_then(|p| p.fast.clone());
        let main_fallback = || -> (Arc<dyn clido_providers::ModelProvider>, String) {
            let p = clido_providers::build_provider(
                &provider,
                api_key.clone(),
                model.clone(),
                base_url.as_deref(),
            )
            .unwrap_or_else(|e| panic!("cannot build main provider for TUI: {e}"));
            (p, model.clone())
        };
        if let Some(ref cfg) = fast_cfg {
            match crate::agent_setup::build_tui_fast_provider(cfg) {
                Ok(p) => (p, cfg.model.clone()),
                Err(e) => {
                    tracing::warn!("fast provider build failed, falling back to main: {e}");
                    main_fallback()
                }
            }
        } else {
            main_fallback()
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

    // Reviewer is always available (sub-agents always registered now).
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
        // Reset terminal: disable mouse tracking, bracketed paste, show cursor
        let reset_seq = b"\x1b[?1002l\x1b[?1003l\x1b[?2004l\x1b[?25h\x1b[0m";
        let _ = std::io::stderr().write_all(reset_seq);
        let _ = execute!(
            std::io::stderr(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste
        );
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

    // Flush any pending input to clear garbage (e.g., ^[[201~ from bracketed paste)
    #[cfg(unix)]
    unsafe {
        libc::tcflush(0, libc::TCIFLUSH);
    }

    // Reset terminal state: disable mouse tracking (1002, 1003) and bracketed paste (2004)
    // before enabling our own settings. This ensures clean state on startup.
    out.write_all(b"\x1b[?1002l\x1b[?1003l\x1b[?2004l")?;
    out.flush()?;

    // Enable mouse capture for scrolling and bracketed paste for multiline paste support.
    // Text selection still works with Shift+drag.
    // We properly clean up on exit to avoid escape sequence leakage.
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let plan_dry_run = cli.plan_dry_run;

    // Build model list from already-loaded config + pricing (no extra disk I/O needed).
    let (known_models, current_profile) = {
        let profile = cli
            .profile
            .clone()
            .or_else(|| loaded_config.as_ref().map(|c| c.default_profile.clone()))
            .unwrap_or_else(|| "default".to_string());
        let models = build_model_list(&pricing_table, &model_prefs);
        (models, profile)
    };

    let mut app = App::new(
        AgentChannels {
            prompt_tx: runtime.prompt_tx.clone(),
            resume_tx: runtime.resume_tx.clone(),
            model_switch_tx: runtime.model_switch_tx.clone(),
            workdir_tx: runtime.workdir_tx.clone(),
            compact_now_tx: runtime.compact_now_tx.clone(),
            fetch_tx: runtime.fetch_tx.clone(),
            kill_tx: runtime.kill_tx.clone(),
            allowed_paths_tx: runtime.allowed_paths_tx.clone(),
            note_tx: runtime.note_tx.clone(),
            path_permission_tx: runtime.path_permission_tx.clone(),
            profile_switch_tx: runtime.profile_switch_tx.clone(),
        },
        cancel,
        provider.clone(),
        model,
        workspace_root.clone(),
        notify_enabled,
        image_state,
        plan_dry_run,
        known_models,
        model_prefs,
        current_profile,
        reviewer_enabled,
        runtime.todo_store.clone(),
        api_key.clone(),
        base_url.clone(),
        tui_utility_provider,
        tui_utility_model,
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

    // Spawn background update check (non-blocking, rate-limited to once per 24h).
    // Results arrive as AgentEvent::UpdateAvailable if a newer version exists.
    crate::update_check::spawn_update_check(runtime.fetch_tx.clone(), false);
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
                // Prefer the session ID we received from the agent; fall back to
                // the original CLI `--resume` value so we never accidentally
                // create a brand-new session during recovery.
                let resume_id = app.current_session_id.as_deref().or(cli.resume.as_deref());
                if let Some(sid) = resume_id {
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
                app.channels.prompt_tx = runtime.prompt_tx.clone();
                app.channels.resume_tx = runtime.resume_tx.clone();
                app.channels.model_switch_tx = runtime.model_switch_tx.clone();
                app.channels.workdir_tx = runtime.workdir_tx.clone();
                app.channels.compact_now_tx = runtime.compact_now_tx.clone();
                app.channels.fetch_tx = runtime.fetch_tx.clone();
                app.channels.kill_tx = runtime.kill_tx.clone();
                app.channels.allowed_paths_tx = runtime.allowed_paths_tx.clone();
                app.channels.note_tx = runtime.note_tx.clone();
                app.channels.path_permission_tx = runtime.path_permission_tx.clone();
                app.channels.profile_switch_tx = runtime.profile_switch_tx.clone();
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
    // Reset terminal: disable mouse tracking, bracketed paste, show cursor, reset colors
    let reset_seq = b"\x1b[?1002l\x1b[?1003l\x1b[?2004l\x1b[?25h\x1b[0m";
    let _ = terminal.backend_mut().write_all(reset_seq);
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    );

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
        next_cli.resume = app.current_session_id.clone().or(cli.resume.clone());
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
                .map(resolve_display_api_key)
                .unwrap_or_default();
            crate::setup::SetupPreFill {
                provider: app.provider.clone(),
                api_key,
                model: app.model.clone(),
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
    const STALL_WARNING_SECS: u64 = 30;
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

                // Check for background rate limit ping
                if app.rate_limit_pinging && !app.rate_limit_cancelled {
                    if let Some(next_ping) = app.rate_limit_next_ping {
                        if std::time::Instant::now() >= next_ping {
                            app.rate_limit_ping_count += 1;
                            let ping_count = app.rate_limit_ping_count;

                            // Send a minimal request to test if API is back
                            // Use the model fetch as a lightweight ping
                            if !app.models_loading && !app.api_key.is_empty() {
                                app.models_loading = true;
                                spawn_model_fetch(
                                    app.provider.clone(),
                                    app.api_key.clone(),
                                    app.base_url.clone(),
                                    app.channels.fetch_tx.clone(),
                                );
                                app.push_toast(
                                    format!("Rate limit recovery: ping #{} sent", ping_count),
                                    TUI_STATE_WARN,
                                    std::time::Duration::from_secs(3),
                                );
                            }

                            // Schedule next ping in 15 minutes
                            app.rate_limit_next_ping = Some(
                                std::time::Instant::now() + std::time::Duration::from_secs(15 * 60)
                            );
                        }
                    }
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
                    let elapsed = baseline.elapsed().as_secs();
                    if elapsed >= STALL_TIMEOUT_SECS {
                        return Ok(EventLoopExit::Recover(
                            "agent appears stalled (no progress events)".to_string(),
                        ));
                    }
                    // Show warning at 30 seconds if agent seems stuck
                    if elapsed >= STALL_WARNING_SECS {
                        let should_warn = app.last_stall_warning.is_none_or(|t| {
                            t.elapsed().as_secs() >= 30
                        });
                        if should_warn {
                            app.push(ChatLine::Info(format!(
                                "  ⚠ Agent hasn't responded for {}s. Press Ctrl+Enter to interrupt or wait...",
                                elapsed
                            )));
                            app.last_stall_warning = Some(std::time::Instant::now());
                            dirty = true;
                        }
                    }
                }
                // Auto-resume after rate limit: when the timer expires and user
                // hasn't cancelled, send a "continue" message to the agent.
                if let Some(resume_at) = app.rate_limit_resume_at {
                    if !app.rate_limit_cancelled {
                        dirty = true; // keep redrawing to update countdown
                        if std::time::Instant::now() >= resume_at && !app.busy {
                            app.rate_limit_resume_at = None;
                            app.push(ChatLine::Info(
                                "  ▶ Rate limit reset — resuming automatically…".into(),
                            ));
                            app.send_silent(
                                "continue where you left off — you were interrupted by a rate limit, pick up from where you stopped".to_string(),
                            );
                        }
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
                        } else if let Some(ref mut ed) = app.plan.text_editor {
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
                        } else if let Some(ref mut ed) = app.workflow_editor {
                            // Route paste into workflow editor — supports multiline paste.
                            let paste_lines: Vec<&str> = text.lines().collect();
                            if let Some(first) = paste_lines.first() {
                                let line = &mut ed.lines[ed.cursor_row];
                                let byte_pos = line
                                    .char_indices()
                                    .nth(ed.cursor_col)
                                    .map(|(i, _)| i)
                                    .unwrap_or(line.len());
                                if paste_lines.len() == 1 {
                                    line.insert_str(byte_pos, first);
                                    ed.cursor_col += first.chars().count();
                                } else {
                                    // Split current line and insert pasted lines
                                    let after: String = line[byte_pos..].to_string();
                                    line.truncate(byte_pos);
                                    line.push_str(first);
                                    for (i, pl) in paste_lines.iter().enumerate().skip(1) {
                                        let new_line = if i == paste_lines.len() - 1 {
                                            format!("{pl}{after}")
                                        } else {
                                            pl.to_string()
                                        };
                                        ed.lines.insert(ed.cursor_row + i, new_line);
                                    }
                                    ed.cursor_row += paste_lines.len() - 1;
                                    ed.cursor_col = paste_lines.last().unwrap_or(&"").chars().count();
                                }
                            }
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
                                    text.replace('\n', "")
                                } else {
                                    text.clone()
                                };
                                let b = char_byte_pos_tui(&ov.input, ov.input_cursor);
                                ov.input.insert_str(b, &clean);
                                ov.input_cursor += clean.chars().count();
                            }
                        } else if app.pending_perm.is_some() {
                            if let Some(ref mut fb) = app.perm_feedback_input {
                                fb.push_str(&text);
                            }
                            // Option list: ignore stray paste (do not leak into chat input).
                        } else if let Some(mp) = app.model_picker.as_mut() {
                            let insert = text.replace(['\n', '\r'], " ");
                            if !insert.is_empty() {
                                mp.filter.push_str(&insert);
                                mp.selected = 0;
                                mp.scroll_offset = 0;
                                mp.clamp();
                            }
                        } else if let Some(sp) = app.session_picker.as_mut() {
                            sp.picker.filter.paste(&text);
                            sp.picker.apply_filter();
                        } else if let Some(pp) = app.profile_picker.as_mut() {
                            pp.picker.filter.paste(&text);
                            pp.picker.apply_filter();
                        } else {
                            let byte_pos = char_byte_pos(&app.text_input.text, app.text_input.cursor);
                            app.text_input.text.insert_str(byte_pos, &text);
                            app.text_input.cursor += text.chars().count();
                            app.selected_cmd = None;
                            app.text_input.history_idx = None;
                        }
                    }
                    Some(Ok(Event::Mouse(m))) => {
                        use super::state::FocusTarget;
                        let focus = app.focus();
                        match m.kind {
                            MouseEventKind::ScrollDown => {
                                match focus {
                                    FocusTarget::Overlay => {
                                        app.overlay_stack.scroll_by(1);
                                    }
                                    FocusTarget::ModelPicker => {
                                        if let Some(mp) = app.model_picker.as_mut() {
                                            let n = mp.filtered().len();
                                            if n > 0 {
                                                mp.selected = (mp.selected + 1).min(n - 1);
                                                mp.scroll_offset = mp.scroll_offset.min(mp.selected);
                                            }
                                        }
                                    }
                                    FocusTarget::SessionPicker => {
                                        if let Some(sp) = app.session_picker.as_mut() {
                                            sp.picker.move_down();
                                        }
                                    }
                                    FocusTarget::ProfilePicker => {
                                        if let Some(pp) = app.profile_picker.as_mut() {
                                            pp.picker.move_down();
                                        }
                                    }
                                    _ => scroll_down(app, 1),
                                }
                            }
                            MouseEventKind::ScrollUp => {
                                match focus {
                                    FocusTarget::Overlay => {
                                        app.overlay_stack.scroll_by(-1);
                                    }
                                    FocusTarget::ModelPicker => {
                                        if let Some(mp) = app.model_picker.as_mut() {
                                            mp.selected = mp.selected.saturating_sub(1);
                                        }
                                    }
                                    FocusTarget::SessionPicker => {
                                        if let Some(sp) = app.session_picker.as_mut() {
                                            sp.picker.move_up();
                                        }
                                    }
                                    FocusTarget::ProfilePicker => {
                                        if let Some(pp) = app.profile_picker.as_mut() {
                                            pp.picker.move_up();
                                        }
                                    }
                                    _ => scroll_up(app, 1),
                                }
                            }
                            MouseEventKind::Down(_) => {
                                // Start text selection on mouse-down inside the chat area.
                                let (cy0, cy1) = app.layout.chat_area_y;
                                if !focus.is_modal()
                                    && m.row >= cy0
                                    && m.row < cy1
                                {
                                    let chat_row = (m.row - cy0) as usize;
                                    let content_row = chat_row + (app.scroll as usize);
                                    app.selection.start(content_row, m.column as usize);
                                    app.selection_mode = true;
                                }
                            }
                            MouseEventKind::Drag(_) => {
                                // Extend selection while dragging.
                                if app.selection_mode {
                                    let cy0 = app.layout.chat_area_y.0;
                                    let chat_row = m.row.saturating_sub(cy0) as usize;
                                    let content_row = chat_row + (app.scroll as usize);
                                    app.selection.update(content_row, m.column as usize);
                                }
                            }
                            MouseEventKind::Up(_) => {
                                if app.selection_mode {
                                    app.selection_mode = false;
                                    // If anchor == focus the user just clicked without
                                    // dragging — clear the selection so it doesn't
                                    // linger as a zero-width highlight.
                                    if app.selection.anchor == app.selection.focus {
                                        app.selection.clear();
                                    } else {
                                        // Auto-copy on mouse-up.
                                        let text = app.get_selected_text();
                                        if !text.is_empty() {
                                            let toast_pos = (m.column, m.row);
                                            match copy_to_clipboard(&text) {
                                                Ok(()) => {
                                                    let char_count = text.chars().count();
                                                    app.push_toast_at(
                                                        format!("Copied {char_count} chars"),
                                                        TUI_TEXT,
                                                        std::time::Duration::from_secs(2),
                                                        toast_pos,
                                                    );
                                                }
                                                Err(e) => {
                                                    app.push_toast_at(
                                                        format!("Copy failed: {e}"),
                                                        TUI_STATE_ERR,
                                                        std::time::Duration::from_secs(3),
                                                        toast_pos,
                                                    );
                                                }
                                            }
                                        }
                                        app.selection.clear();
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        // Force a clean redraw after terminal resize to avoid stale cells.
                        // Preserve scroll ratio so user doesn't lose their place.
                        let ratio = if app.layout.max_scroll > 0 && !app.following {
                            Some(app.scroll as f64 / app.layout.max_scroll as f64)
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
                        app.turn_tool_tally.record(&name);
                        // Format detail from JSON to human-readable
                        let detail_formatted = format_tool_detail(&name, &detail);
                        app.push_status(
                            tool_use_id.clone(),
                            name.clone(),
                            detail_formatted.clone(),
                        );
                        app.push(ChatLine::ToolCall {
                            tool_use_id,
                            name,
                            detail: detail_formatted,
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
                                app.stats.session_total_cost_usd,
                                &app.provider,
                            );
                        }
                        // Revert per-turn model override if active.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.channels.model_switch_tx.send(prev.clone());
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
                        app.push(ChatLine::Info("  ↻ Interrupted — processing next item".into()));
                        // Revert per-turn model override on interruption too.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.channels.model_switch_tx.send(prev);
                        }
                        app.on_agent_done();
                    }

                    Some(AgentEvent::Err(msg)) => {
                        last_agent_activity = std::time::Instant::now();
                        app.enhancing = false;
                        // Revert per-turn model override on error too.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.channels.model_switch_tx.send(prev);
                        }
                        app.overlay_stack.push(OverlayKind::Error(
                            ErrorOverlay::from_message(msg),
                        ));
                        app.on_agent_done();
                    }
                    Some(AgentEvent::RateLimited { message, retry_after_secs, is_subscription_limit }) => {
                        last_agent_activity = std::time::Instant::now();
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.channels.model_switch_tx.send(prev);
                        }
                        // Build a user-friendly message with reset time
                        let reset_info = match retry_after_secs {
                            Some(secs) if secs >= 3600 => {
                                let h = secs / 3600;
                                let m = (secs % 3600) / 60;
                                format!("resets in ~{}h {:02}m", h, m)
                            }
                            Some(secs) if secs >= 60 => {
                                format!("resets in ~{}m", secs / 60)
                            }
                            Some(secs) => format!("resets in ~{}s", secs),
                            None => "reset time unknown".to_string(),
                        };
                        if is_subscription_limit {
                            app.push(ChatLine::Info(format!(
                                "  ⚠ Subscription limit reached — {reset_info}"
                            )));
                            app.push(ChatLine::Info(format!(
                                "    {message}"
                            )));
                        } else {
                            app.push(ChatLine::Info(format!(
                                "  ⚠ Rate limited — {reset_info}. {message}"
                            )));
                        }

                        // Auto-resume: if we know the reset time, schedule automatic
                        // continuation. The user can press Escape to cancel.
                        if let Some(secs) = retry_after_secs {
                            let resume_at = std::time::Instant::now()
                                + std::time::Duration::from_secs(secs + 5); // +5s buffer
                            app.rate_limit_resume_at = Some(resume_at);
                            app.rate_limit_cancelled = false;
                            app.push(ChatLine::Info(
                                "    ⏳ Will auto-resume when limit resets. Press Esc to cancel.".into()
                            ));
                        } else {
                            // Unknown reset time - start background pinging mode
                            app.rate_limit_cancelled = false;
                            app.rate_limit_pinging = true;
                            app.rate_limit_next_ping = Some(
                                std::time::Instant::now() + std::time::Duration::from_secs(15 * 60)
                            );
                            app.rate_limit_ping_count = 0;
                            app.push(ChatLine::Info(
                                "    ⏳ Auto-recovery active: checking every 15 min. Press Esc to cancel.".into()
                            ));
                        }
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
                        let delta_in = if input_tokens >= app.stats.session_input_tokens {
                            input_tokens - app.stats.session_input_tokens
                        } else {
                            input_tokens // new run — full value is the delta
                        };
                        let delta_out = if output_tokens >= app.stats.session_output_tokens {
                            output_tokens - app.stats.session_output_tokens
                        } else {
                            output_tokens
                        };
                        let delta_cost = if cost_usd >= app.stats.session_cost_usd {
                            cost_usd - app.stats.session_cost_usd
                        } else {
                            cost_usd
                        };
                        app.stats.session_input_tokens = input_tokens;
                        app.stats.session_output_tokens = output_tokens;
                        app.stats.session_cost_usd = cost_usd;
                        app.stats.session_total_input_tokens += delta_in;
                        app.stats.session_total_output_tokens += delta_out;
                        app.stats.session_total_cost_usd += delta_cost;
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
                        app.plan.last_plan = Some(tasks.clone());
                        app.plan.last_plan_snapshot = build_plan_from_tasks(&tasks);
                    }
                    Some(AgentEvent::PlanReady { plan }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.plan.last_plan_snapshot = Some(plan.clone());
                        app.plan.last_plan = Some(
                            plan.tasks
                                .iter()
                                .map(|t| t.description.clone())
                                .collect::<Vec<_>>(),
                        );
                        // Open the plan editor overlay (blocks execution until user presses x or Esc).
                        app.plan.selected_task = 0;
                        app.plan.task_editing = None;
                        app.plan.editor = Some(PlanEditor::new(plan));
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
                    Some(AgentEvent::ModelsLoaded { ids, provider }) => {
                        app.models_loading = false;
                        // 15-minute rate-limit pings use model-list fetch; when it succeeds, resume the agent.
                        if app.rate_limit_pinging && !app.rate_limit_cancelled && !ids.is_empty() {
                            app.rate_limit_pinging = false;
                            app.rate_limit_next_ping = None;
                            app.rate_limit_ping_count = 0;
                            app.push(ChatLine::Info(
                                "  ▶ API reachable again — resuming automatically…".into(),
                            ));
                            if !app.busy {
                                app.send_silent(
                                    "continue where you left off — you were interrupted by a rate limit, pick up from where you stopped".to_string(),
                                );
                            }
                        }
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
                                        provider: provider.clone(),
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
                            // Also refresh the profile overlay model picker if active.
                            if let Some(overlay) = &mut app.profile_overlay {
                                if let Some(picker) = &mut overlay.profile_model_picker {
                                    let all: Vec<String> =
                                        app.known_models.iter().map(|m| m.id.clone()).collect();
                                    picker.refresh_models(all);
                                }
                            }
                        }
                    }
                    Some(AgentEvent::TitleGenerated(title)) => {
                        app.session_title = Some(title);
                    }
                    Some(AgentEvent::EnhancedPrompt(enhanced)) => {
                        app.enhancing = false;
                        app.push(ChatLine::Section("Enhanced Prompt".into()));
                        for line in enhanced.lines() {
                            app.push(ChatLine::Info(format!("  {line}")));
                        }
                        app.push(ChatLine::Info("".into()));
                        // Place the enhanced prompt in the input for user review/editing.
                        app.text_input.set_text(enhanced);
                        app.push_toast(
                            "Enter to send, edit first if you like",
                            TUI_STATE_INFO,
                            std::time::Duration::from_secs(3),
                        );
                    }
                    Some(AgentEvent::UpdateAvailable { version }) => {
                        app.push(ChatLine::Info(format!(
                            "  ↻ Update available: v{} (current: v{}). Run /update to install.",
                            version,
                            env!("CARGO_PKG_VERSION")
                        )));
                    }
                    Some(AgentEvent::UpdateStatus(status)) => {
                        app.push(ChatLine::Info(format!("  {}", status)));
                    }
                    Some(AgentEvent::Info { message }) => {
                        app.push(ChatLine::Info(format!("  {}", message)));
                    }
                    Some(AgentEvent::PathPermissionRequest { path, tool_name }) => {
                        // Show interactive prompt for external path access
                        app.push(ChatLine::Info(format!(
                            "  🔒 Tool '{}' wants to access: {}",
                            tool_name,
                            path.display()
                        )));
                        app.push(ChatLine::Info(
                            "  Allow this path for this session? [y]es / [n]o / [a]lways (add to allowed paths)".into()
                        ));
                        // Store the pending request so input handler can respond
                        app.pending_path_permission = Some(path);
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

        } // end select!

        // ── Throttle render
        // ── Spawn /enhance task if pending ─────────────────────────────
        if let Some(raw_prompt) = app.pending_enhance.take() {
            let enhance_tx = app.channels.fetch_tx.clone();
            let ep = app.utility_provider.clone();
            let em = app.utility_model.clone();
            tokio::spawn(async move {
                let system = crate::prompt_enhance::build_system_prompt(None);
                let msgs = vec![clido_core::Message {
                    role: clido_core::Role::User,
                    content: vec![clido_core::ContentBlock::Text { text: raw_prompt }],
                }];
                let cfg = clido_core::AgentConfig {
                    model: em,
                    max_turns: 1,
                    system_prompt: Some(system),
                    ..Default::default()
                };
                match ep.complete(&msgs, &[], &cfg).await {
                    Ok(resp) => {
                        let enhanced = resp
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
                        if !enhanced.is_empty() {
                            let _ = enhance_tx.send(AgentEvent::EnhancedPrompt(enhanced));
                        } else {
                            let _ = enhance_tx.send(AgentEvent::Err(
                                "Enhancement returned empty response".into(),
                            ));
                        }
                    }
                    Err(e) => {
                        let _ = enhance_tx.send(AgentEvent::Err(format!(
                            "Enhancement failed: {e} — sending original prompt"
                        )));
                    }
                }
            });
        }

        if app.quit {
            return Ok(EventLoopExit::Quit);
        }
    }
    Ok(EventLoopExit::Quit)
}
