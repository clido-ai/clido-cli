use std::path::{Path, PathBuf};

use clido_index::RepoIndex;
use clido_memory::MemoryStore;

use crate::list_picker::ListPicker;
use crate::overlay::{ErrorOverlay, OverlayKind, ReadOnlyOverlay};

use super::state::{PlanPanelVisibility, StatusRailVisibility};
use super::*;

/// Switch the default profile on disk, tell the agent to rebuild provider/tools in-process,
/// and refresh header state — **without** restarting the TUI (same session).
pub(super) fn switch_profile_seamless(app: &mut App, name: &str) {
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
        Ok(loaded) => {
            if !loaded.profiles.contains_key(name) {
                app.push(ChatLine::Info(format!(
                    "  profile '{}' not found. Use /profiles to list or /profile new to create.",
                    name
                )));
            } else if name == loaded.default_profile {
                app.push(ChatLine::Info(format!(
                    "  profile '{}' is already active.",
                    name
                )));
            } else {
                let config_path = clido_core::global_config_path()
                    .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                if let Err(e) = clido_core::switch_active_profile(&config_path, name) {
                    app.push(ChatLine::Info(format!(
                        "  ✗ Failed to switch profile: {}",
                        e
                    )));
                    return;
                }
                let _ = app.channels.profile_switch_tx.send(name.to_string());
                crate::tui::event_loop::sync_tui_profile_from_disk(app, name);
                if !app.api_key.is_empty() {
                    crate::tui::event_loop::spawn_model_fetch(
                        app.provider.clone(),
                        app.api_key.clone(),
                        app.base_url.clone(),
                        app.channels.fetch_tx.clone(),
                    );
                    app.models_loading = true;
                }
                app.push(ChatLine::Info(format!(
                    "  switched to profile '{}' (session continues)...",
                    name
                )));
            }
        }
    }
}

/// Rebuild the running agent from disk for this profile (same name), e.g. after editing API keys.
pub(super) fn reload_active_profile_in_agent(app: &mut App, name: &str) {
    let _ = app.channels.profile_switch_tx.send(name.to_string());
    crate::tui::event_loop::sync_tui_profile_from_disk(app, name);
    if !app.api_key.is_empty() {
        crate::tui::event_loop::spawn_model_fetch(
            app.provider.clone(),
            app.api_key.clone(),
            app.base_url.clone(),
            app.channels.fetch_tx.clone(),
        );
        app.models_loading = true;
    }
}

/// Return true if `input` is an exact slash command or a slash command followed
/// by a space (i.e. a command with arguments). Used to decide whether Enter
/// should execute a command or send the input as a chat message.
pub(super) fn is_known_slash_cmd(input: &str) -> bool {
    if !input.starts_with('/') {
        return false;
    }
    slash_commands()
        .into_iter()
        .any(|(cmd, _)| input == cmd || input.starts_with(&format!("{} ", cmd)))
}

pub(super) fn slash_completions(input: &str) -> Vec<(&'static str, &'static str)> {
    if !input.starts_with('/') {
        return vec![];
    }
    slash_commands()
        .into_iter()
        .filter(|(cmd, _)| cmd.starts_with(input))
        .collect()
}

/// A row in the autocomplete popup: either a non-selectable section header or a
/// selectable command. `flat_idx` is the index into `slash_completions()` output.
pub(super) enum CompletionRow {
    Header(&'static str),
    Cmd {
        flat_idx: usize,
        cmd: &'static str,
        desc: &'static str,
    },
}

/// Grouped version of `slash_completions`: same matches but interleaved with
/// section headers so the popup can show them visually.
pub(super) fn slash_completion_rows(input: &str) -> Vec<CompletionRow> {
    if !input.starts_with('/') {
        return vec![];
    }
    let mut rows = Vec::new();
    let mut flat_idx = 0usize;
    for (section, cmds) in slash_command_sections() {
        let matches: Vec<_> = cmds
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(input))
            .collect();
        if !matches.is_empty() {
            rows.push(CompletionRow::Header(section));
            for (cmd, desc) in matches {
                rows.push(CompletionRow::Cmd {
                    flat_idx,
                    cmd,
                    desc,
                });
                flat_idx += 1;
            }
        }
    }
    rows
}

/// Parse `@model-name remaining prompt` per-turn override syntax.
/// Returns `Some((model_id, prompt))` only when input starts with `@` followed
/// by a model name token and a space-separated prompt.
/// Returns `None` for normal input that contains `@` mid-string.
pub(super) fn parse_per_turn_model(input: &str) -> Option<(String, String)> {
    if !input.starts_with('@') {
        return None;
    }
    let rest = &input[1..];
    let space_idx = rest.find(' ')?;
    let model = rest[..space_idx].trim().to_string();
    let prompt = rest[space_idx + 1..].trim().to_string();
    if model.is_empty() || prompt.is_empty() {
        return None;
    }
    Some((model, prompt))
}

// ── Per-command handler functions (extracted from execute_slash) ─────────────

pub(super) fn cmd_help(app: &mut App) {
    app.push(ChatLine::Info("".into()));
    app.push(ChatLine::Section("Navigation".into()));
    app.push(ChatLine::Info("Enter              send message".into()));
    app.push(ChatLine::Info(
        "Shift+Enter        insert newline (multiline input)".into(),
    ));
    app.push(ChatLine::Info("Ctrl+Enter         interrupt & send".into()));
    app.push(ChatLine::Info(
        "↑↓                 input history / multiline cursor".into(),
    ));
    app.push(ChatLine::Info(
        "PgUp/PgDn          scroll conversation".into(),
    ));
    app.push(ChatLine::Info(
        "Ctrl+Home/End      jump to top/bottom".into(),
    ));
    app.push(ChatLine::Info("Ctrl+U             clear input".into()));
    app.push(ChatLine::Info(
        "Ctrl+W             delete word backward".into(),
    ));
    app.push(ChatLine::Info("Alt+←/→            jump by word".into()));
    app.push(ChatLine::Info("".into()));
    app.push(ChatLine::Section("Agent Controls".into()));
    app.push(ChatLine::Info("Ctrl+C             quit".into()));
    app.push(ChatLine::Info(
        "Ctrl+/             interrupt current run only".into(),
    ));
    app.push(ChatLine::Info(
        "Ctrl+Y             copy last assistant message".into(),
    ));
    app.push(ChatLine::Info(
        "Queue              type while agent runs, sends on finish".into(),
    ));
    app.push(ChatLine::Info("".into()));
    for (section, cmds) in slash_command_sections() {
        app.push(ChatLine::Section(section.to_string()));
        for (cmd, desc) in cmds {
            app.push(ChatLine::Info(format!("{:<18} {}", cmd, desc)));
        }
        app.push(ChatLine::Info("".into()));
    }
    app.push(ChatLine::Section("Per-turn Override".into()));
    app.push(ChatLine::Info(
        "@model-name <msg>  use a different model for one turn".into(),
    ));
    app.push(ChatLine::Info(
        "                   e.g. @claude-opus-4-6 refactor this".into(),
    ));
    app.push(ChatLine::Info("".into()));
}

pub(super) fn cmd_keys(app: &mut App) {
    let lines: Vec<(String, String)> = vec![
        (
            "Navigation".into(),
            "Enter              send message\n\
            Shift+Enter        insert newline (multiline)\n\
            Ctrl+Enter         interrupt & send\n\
            ↑↓ (empty input)   scroll conversation\n\
            ↑↓ (with text)     history navigation\n\
            PgUp/PgDn          scroll 10 lines\n\
            Ctrl+Home/End      jump to top/bottom\n\
            Home/End           cursor start/end of line\n\
            Alt+←/→            jump by word\n\
            Ctrl+U             clear input\n\
            Ctrl+W             delete word backward"
                .into(),
        ),
        (
            "Agent Controls".into(),
            "Ctrl+C             quit\n\
            Ctrl+/             interrupt current run\n\
            Ctrl+Y             copy last assistant message\n\
            Ctrl+L             refresh screen\n\
            Queue              type while agent runs, auto-sends on finish"
                .into(),
        ),
        (
            "Pickers".into(),
            "↑↓                 navigate items\n\
            Enter              select / confirm\n\
            Esc                close / cancel\n\
            1-9                jump to first 9 matches (/ commands menu)\n\
            Type               filter long lists (model, profile, session pickers)\n\
            Backspace          remove filter char\n\
            Ctrl+F             toggle favorite (model picker)\n\
            Ctrl+S             save as default (model picker)\n\
            Ctrl+D             delete selected session (session picker)\n\
            Ctrl+N             new profile (profile picker)\n\
            Ctrl+E             edit profile (profile picker)"
                .into(),
        ),
        (
            "Plan Editor".into(),
            "Ctrl+S             save plan\n\
            Esc                discard changes"
                .into(),
        ),
        (
            "Per-turn Override".into(),
            "@model-name <msg>  use a different model for one turn".into(),
        ),
    ];
    app.overlay_stack
        .push(OverlayKind::ReadOnly(ReadOnlyOverlay::new(
            "Keyboard Shortcuts",
            lines,
        )));
}

pub(super) fn cmd_fast(app: &mut App) {
    let new_model = app
        .model_prefs
        .roles
        .get("fast")
        .map(|s| s.as_str())
        .unwrap_or("claude-haiku-4-5-20251001")
        .to_string();
    app.model = new_model.clone();
    let _ = app.channels.model_switch_tx.send(new_model.clone());
    app.model_prefs.push_recent(&new_model);
    app.model_prefs.save();
    app.push(ChatLine::Info(format!("  ✓ Model: {} (fast)", new_model)));
}

pub(super) fn cmd_smart(app: &mut App) {
    let new_model = app
        .model_prefs
        .roles
        .get("smart")
        .map(|s| s.as_str())
        .or_else(|| app.model_prefs.roles.get("reasoning").map(|s| s.as_str()))
        .unwrap_or("claude-opus-4-6")
        .to_string();
    app.model = new_model.clone();
    let _ = app.channels.model_switch_tx.send(new_model.clone());
    app.model_prefs.push_recent(&new_model);
    app.model_prefs.save();
    app.push(ChatLine::Info(format!("  ✓ Model: {} (smart)", new_model)));
}

pub(super) fn cmd_model(app: &mut App, cmd: &str) {
    let arg = cmd.trim_start_matches("/model").trim();
    if arg.is_empty() {
        // No name given → open the interactive model picker (same as /models).
        let models = app.known_models.clone();
        // Trigger a fresh API fetch if we have no models yet and aren't already loading.
        if models.is_empty() && !app.models_loading && !app.api_key.is_empty() {
            spawn_model_fetch(
                app.provider.clone(),
                app.api_key.clone(),
                app.base_url.clone(),
                app.channels.fetch_tx.clone(),
            );
            app.models_loading = true;
        }
        app.model_picker = Some(ModelPickerState {
            models,
            filter: String::new(),
            selected: 0,
            scroll_offset: 0,
        });
    } else {
        let new_model = arg.to_string();
        app.model = new_model.clone();
        let _ = app.channels.model_switch_tx.send(new_model.clone());
        app.model_prefs.push_recent(&new_model);
        app.model_prefs.save();
        app.push(ChatLine::Info(format!("  ✓ Model: {}", new_model)));
    }
}

pub(super) fn cmd_models(app: &mut App) {
    let models = app.known_models.clone();
    // Trigger a fresh API fetch if we have no models yet and aren't already loading.
    if models.is_empty() && !app.models_loading && !app.api_key.is_empty() {
        spawn_model_fetch(
            app.provider.clone(),
            app.api_key.clone(),
            app.base_url.clone(),
            app.channels.fetch_tx.clone(),
        );
        app.models_loading = true;
    }
    app.model_picker = Some(ModelPickerState {
        models,
        filter: String::new(),
        selected: 0,
        scroll_offset: 0,
    });
}

pub(super) fn cmd_fav(app: &mut App) {
    let model_id = app.model.clone();
    app.model_prefs.toggle_favorite(&model_id);
    app.model_prefs.save();
    // Rebuild model list with updated favorites.
    let (pricing, _) = clido_core::load_pricing();
    app.known_models = build_model_list(&pricing, &app.model_prefs);
    let is_fav = app.model_prefs.is_favorite(&model_id);
    let icon = if is_fav { "★" } else { "☆" };
    app.push(ChatLine::Info(format!(
        "  {} {} {}",
        icon,
        model_id,
        if is_fav {
            "added to favorites"
        } else {
            "removed from favorites"
        }
    )));
}

pub(super) fn cmd_reviewer(app: &mut App, cmd: &str) {
    if !app.reviewer_configured {
        app.push(ChatLine::Info(
            "  reviewer not configured — run /init to add a reviewer sub-agent".into(),
        ));
    } else {
        let arg = cmd.trim_start_matches("/reviewer").trim();
        let new_state = match arg {
            "on" => Some(true),
            "off" => Some(false),
            "" => None, // no arg → just show status
            _ => {
                app.push(ChatLine::Info("  Usage: /reviewer [on|off]".into()));
                return;
            }
        };
        if let Some(state) = new_state {
            app.reviewer_enabled.store(state, Ordering::Relaxed);
        }
        let current = app.reviewer_enabled.load(Ordering::Relaxed);
        let status = if current { "on ●" } else { "off ○" };
        app.push(ChatLine::Info(format!("  ✓ Reviewer {}", status)));
    }
}

pub(super) fn cmd_sessions(app: &mut App) {
    use clido_storage::list_sessions;
    match list_sessions(&app.workspace_root) {
        Err(e) => app.push(ChatLine::Info(format!(
            "  ✗ Could not list sessions: {}",
            e
        ))),
        Ok(sessions) if sessions.is_empty() => {
            app.push(ChatLine::Info(
                "  No sessions found for this project".into(),
            ));
        }
        Ok(sessions) => {
            // Interactive picker: resume with Enter, filter by id/title/preview, same as /model.
            let mut picker = ListPicker::new(sessions, 12);
            picker.apply_filter();
            app.session_picker = Some(SessionPickerState { picker });
        }
    }
}

/// Send a note/hint to the agent — adds a user message immediately.
/// Interrupts current agent execution so the note is seen right away.
pub(super) fn cmd_note(app: &mut App, cmd: &str) {
    let text = cmd.trim_start_matches("/note").trim();
    if text.is_empty() {
        app.push(ChatLine::Info(
            "  Usage: /note <text>  — send a hint/correction to the agent".into(),
        ));
        return;
    }
    // Add to chat history immediately with slash command formatting.
    app.push(ChatLine::SlashCommand {
        cmd: "/note".to_string(),
        text: Some(text.to_string()),
    });
    // Send via note channel — this interrupts current agent execution.
    let _ = app.channels.note_tx.send(text.to_string());
}

pub(super) fn cmd_workdir_arg(app: &mut App, cmd: &str) {
    let arg = cmd.trim_start_matches("/workdir").trim();
    match resolve_workdir_arg(arg) {
        Ok(path) => {
            let _ = app.channels.workdir_tx.send(path.clone());
            app.push(ChatLine::Info(format!(
                "  ↻ Switching to {}…",
                path.display()
            )));
            app.push(ChatLine::Info(
                "  Prompts stay on the current directory until the switch completes.".into(),
            ));
        }
        Err(e) => app.push(ChatLine::Info(format!(
            "  ✗ Working directory error: {}",
            e
        ))),
    }
}

pub(super) fn cmd_copy(app: &mut App, cmd: &str) {
    let arg = cmd.trim_start_matches("/copy").trim();
    // Collect assistant and user chat lines as plain text
    let chat_lines: Vec<(bool, &str)> = app
        .messages
        .iter()
        .filter_map(|m| match m {
            ChatLine::Assistant(t) => Some((false, t.as_str())),
            ChatLine::User(t) => Some((true, t.as_str())),
            _ => None,
        })
        .collect();
    if chat_lines.is_empty() {
        app.push(ChatLine::Info("  ✗ Nothing to copy yet".into()));
    } else if arg.is_empty() {
        // Default: copy last assistant reply
        match app.last_assistant_text().map(|s| s.to_string()) {
            Some(text) => match copy_to_clipboard(&text) {
                Ok(()) => app.push(ChatLine::Info("  ✓ Last reply copied to clipboard".into())),
                Err(e) => app.push(ChatLine::Info(format!("  ✗ Copy failed: {}", e))),
            },
            None => app.push(ChatLine::Info("  ✗ No assistant reply yet".into())),
        }
    } else {
        // /copy all  or  /copy <n>  — build a transcript
        let take_n: Option<usize> = if arg == "all" {
            None
        } else {
            arg.parse::<usize>().ok().map(|n| n * 2) // n exchanges = n user + n assistant
        };
        let slice: Vec<_> = match take_n {
            None => chat_lines.iter().collect(),
            Some(n) => chat_lines
                .iter()
                .rev()
                .take(n)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
        };
        let mut buf = String::new();
        for (is_user, text) in &slice {
            if *is_user {
                buf.push_str("You: ");
            } else {
                buf.push_str("clido: "); // keep clido, don't change it to assistant!
            }
            buf.push_str(text);
            buf.push_str("\n\n");
        }
        let count = slice.len();
        match copy_to_clipboard(buf.trim()) {
            Ok(()) => app.push(ChatLine::Info(format!(
                "  ✓ Copied {} message{} to clipboard",
                count,
                if count == 1 { "" } else { "s" }
            ))),
            Err(e) => app.push(ChatLine::Info(format!("  ✗ Copy failed: {}", e))),
        }
    }
}

pub(super) fn cmd_search(app: &mut App, cmd: &str) {
    let query = cmd.trim_start_matches("/search").trim();
    if query.is_empty() {
        app.push(ChatLine::Info(
            "  Usage: /search <query>  — search this conversation".into(),
        ));
    } else {
        let q_lower = query.to_lowercase();
        let mut hits: Vec<(usize, &str, String)> = Vec::new(); // (turn_index, role, snippet)
        let mut turn = 0usize;
        for line in &app.messages {
            match line {
                ChatLine::User(text) => {
                    turn += 1;
                    if text.to_lowercase().contains(&q_lower) {
                        hits.push((turn, "you", truncate_chars(text, 80)));
                    }
                }
                ChatLine::Assistant(text) => {
                    if text.to_lowercase().contains(&q_lower) {
                        hits.push((turn, "assistant", truncate_chars(text, 80)));
                    }
                }
                _ => {}
            }
        }
        if hits.is_empty() {
            app.push(ChatLine::Info(format!(
                "  No results for \"{}\" in this conversation",
                query
            )));
        } else {
            app.push(ChatLine::Info(format!(
                "  {} result{} for \"{}\":",
                hits.len(),
                if hits.len() == 1 { "" } else { "s" },
                query
            )));
            for (turn_idx, role, snippet) in &hits {
                app.push(ChatLine::Info(format!(
                    "  [turn {}] {}  {}",
                    turn_idx, role, snippet
                )));
            }
        }
    }
}

pub(super) fn cmd_export(app: &mut App) {
    // Export conversation as a markdown file.
    let mut md = String::new();
    md.push_str("# Conversation Export\n\n");
    let mut turn = 0usize;
    for line in &app.messages {
        match line {
            ChatLine::User(text) => {
                turn += 1;
                md.push_str(&format!("## Turn {} — You\n\n{}\n\n", turn, text));
            }
            ChatLine::Assistant(text) => {
                md.push_str(&format!(
                    "## Turn {} — {}\n\n{}\n\n",
                    turn, TUI_CHAT_AGENT_LABEL, text
                ));
            }
            _ => {}
        }
    }
    if turn == 0 {
        app.push(ChatLine::Info(
            "  Nothing to export — start a conversation first".into(),
        ));
    } else {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // YYYYMMDD-HHMMSS from unix timestamp (UTC).
        let mins = secs / 60;
        let hours = mins / 60;
        let days = hours / 24;
        let s = secs % 60;
        let m = mins % 60;
        let h = hours % 24;
        // Approximate calendar date (good enough for a filename).
        let d = days % 31 + 1;
        let mo = (days / 31) % 12 + 1;
        let yr = 1970 + days / 365;
        let filename = format!(
            "conversation-{:04}{:02}{:02}-{:02}{:02}{:02}.md",
            yr, mo, d, h, m, s
        );
        let path = app.workspace_root.join(&filename);
        match std::fs::write(&path, &md) {
            Ok(()) => app.push(ChatLine::Info(format!(
                "  ✓ Exported {} turns → {}",
                turn,
                path.display()
            ))),
            Err(e) => app.push(ChatLine::Info(format!("  ✗ Export failed: {}", e))),
        }
    }
}

pub(super) fn cmd_memory(app: &mut App, cmd: &str) {
    let query = cmd.trim_start_matches("/memory").trim();
    if query.is_empty() {
        // No query → show recent memories and total count.
        match tui_memory_store_path() {
            Ok(path) => match MemoryStore::open(&path) {
                Ok(store) => match store.list(5) {
                    Ok(entries) if entries.is_empty() => {
                        app.push(ChatLine::Info(
                            "  No memories saved yet — the agent stores facts automatically while working".into(),
                        ));
                    }
                    Ok(entries) => {
                        app.push(ChatLine::Info(
                            "  Recent memories (use /memory <query> to search):".into(),
                        ));
                        for e in &entries {
                            app.push(ChatLine::Info(format!(
                                "  · {}",
                                truncate_chars(&e.content, 90)
                            )));
                        }
                    }
                    Err(_) => {
                        app.push(ChatLine::Info(
                            "  Usage: /memory <query>  — search saved memories".into(),
                        ));
                    }
                },
                Err(_) => {
                    app.push(ChatLine::Info(
                        "  No memory store found — memories are saved automatically as you work"
                            .into(),
                    ));
                }
            },
            Err(e) => app.push(ChatLine::Info(format!("  ✗ Memory error: {}", e))),
        }
    } else {
        match tui_memory_store_path() {
            Ok(path) => match MemoryStore::open(&path) {
                Ok(store) => match store.search_hybrid(query, 15) {
                    Ok(entries) if entries.is_empty() => {
                        app.push(ChatLine::Info(format!(
                            "  No memory matches for \"{}\"",
                            query
                        )));
                    }
                    Ok(entries) => {
                        app.push(ChatLine::Info(format!(
                            "  Found {} memory match(es) for \"{}\"",
                            entries.len(),
                            query
                        )));
                        for e in entries.iter().take(15) {
                            app.push(ChatLine::Info(format!(
                                "  · {}",
                                truncate_chars(&e.content, 100)
                            )));
                        }
                    }
                    Err(e) => {
                        app.push(ChatLine::Info(format!("  ✗ Memory search failed: {}", e)));
                    }
                },
                Err(e) => app.push(ChatLine::Info(format!(
                    "  ✗ Cannot open memory store: {}",
                    e
                ))),
            },
            Err(e) => app.push(ChatLine::Info(format!("  ✗ Memory error: {}", e))),
        }
    }
}

pub(super) fn cmd_cost(app: &mut App) {
    let is_subscription = clido_providers::is_subscription_provider(&app.provider);
    if is_subscription {
        app.push(ChatLine::Info(
            "  Subscription provider — no per-call cost tracking available.".into(),
        ));
    } else if app.stats.session_total_cost_usd == 0.0 {
        app.push(ChatLine::Info(
            "  Session cost: $0.0000 (no API calls yet)".into(),
        ));
    } else if let Some(budget) = app.max_budget_usd {
        let pct = (app.stats.session_total_cost_usd / budget * 100.0).min(100.0);
        app.push(ChatLine::Info(format!(
            "  Session cost: ${:.4} / ${:.2} ({:.0}% of budget)",
            app.stats.session_total_cost_usd, budget, pct
        )));
    } else {
        app.push(ChatLine::Info(format!(
            "  Session cost: ${:.4}",
            app.stats.session_total_cost_usd
        )));
    }
}

pub(super) fn cmd_tokens(app: &mut App) {
    let is_subscription = clido_providers::is_subscription_provider(&app.provider);

    if is_subscription {
        // For subscriptions, only show turns - token counts may be unreliable
        app.push(ChatLine::Info(
            "  ── Session Statistics ──────────────────────".into(),
        ));
        if app.stats.session_turn_count > 0 {
            app.push(ChatLine::Info(format!(
                "  Turns completed: {}",
                app.stats.session_turn_count
            )));
        } else {
            app.push(ChatLine::Info("  No turns completed yet".into()));
        }
        return;
    }

    // On-demand providers: show full token statistics
    let total = app.stats.session_total_input_tokens + app.stats.session_total_output_tokens;
    let total_str = if total >= 1000 {
        format!("{:.1}k", total as f64 / 1000.0)
    } else {
        total.to_string()
    };
    let ctx_pct = if app.context_max_tokens > 0 && app.stats.session_input_tokens > 0 {
        let pct = (app.stats.session_input_tokens as f64 / app.context_max_tokens as f64 * 100.0)
            .min(100.0);
        format!(
            "  Context window: {:.0}% used ({} / {} tokens)",
            pct, app.stats.session_input_tokens, app.context_max_tokens
        )
    } else {
        String::new()
    };
    app.push(ChatLine::Info(
        "  ── Session Token Usage ──────────────────────".into(),
    ));
    app.push(ChatLine::Info(format!(
        "  Input tokens:   {}",
        app.stats.session_total_input_tokens
    )));
    app.push(ChatLine::Info(format!(
        "  Output tokens:  {}",
        app.stats.session_total_output_tokens
    )));
    app.push(ChatLine::Info(format!("  Total tokens:   {}", total_str)));
    app.push(ChatLine::Info(format!(
        "  Estimated cost: ${:.6}",
        app.stats.session_total_cost_usd
    )));
    if let Some(budget) = app.max_budget_usd {
        let remaining = (budget - app.stats.session_total_cost_usd).max(0.0);
        let pct = (app.stats.session_total_cost_usd / budget * 100.0).min(100.0);
        app.push(ChatLine::Info(format!(
            "  Budget:         ${:.2} ({:.0}% used, ${:.4} remaining)",
            budget, pct, remaining
        )));
    }
    if !ctx_pct.is_empty() {
        app.push(ChatLine::Info(ctx_pct));
    }
    if app.stats.session_turn_count > 0 {
        app.push(ChatLine::Info(format!(
            "  Turns completed: {}",
            app.stats.session_turn_count
        )));
    }
}

pub(super) fn cmd_skills(app: &mut App, cmd: &str) {
    let sub = cmd.trim_start_matches("/skills").trim();
    let cfg = clido_core::load_config(&app.workspace_root)
        .map(|c| c.skills)
        .unwrap_or_default();

    match sub {
        "" | "list" => {
            match clido_core::skills::discover_skills(&app.workspace_root, &cfg.extra_paths) {
                Ok(skills) if skills.is_empty() => {
                    app.push(ChatLine::Info(
                        "  No skills — add .md/.txt under .clido/skills/ or ~/.clido/skills/"
                            .into(),
                    ));
                }
                Ok(skills) => {
                    if cfg.no_skills {
                        app.push(ChatLine::Info(
                            "  [skills] no-skills is on — nothing is injected.".into(),
                        ));
                    }
                    if !cfg.enabled.is_empty() {
                        app.push(ChatLine::Info(format!(
                            "  Whitelist mode: only {:?} may be active.",
                            cfg.enabled
                        )));
                    }
                    app.push(ChatLine::Info(format!(
                        "  Skills ({} on disk) — restart session to refresh agent",
                        skills.len()
                    )));
                    for s in skills {
                        let on =
                            clido_core::skills::is_skill_active_for_config(&s.manifest.id, &cfg);
                        let src = match s.source {
                            clido_core::skills::SkillSourceKind::Workspace => "ws",
                            clido_core::skills::SkillSourceKind::Global => "global",
                            clido_core::skills::SkillSourceKind::Extra => "extra",
                        };
                        let desc = s.manifest.description.clone();
                        let short = if desc.chars().count() > 52 {
                            format!("{}…", desc.chars().take(51).collect::<String>())
                        } else {
                            desc
                        };
                        app.push(ChatLine::Info(format!(
                            "  {}  `{}`  [{}]  {}",
                            if on { "✓" } else { "✗" },
                            s.manifest.id,
                            src,
                            short
                        )));
                    }
                }
                Err(e) => app.push(ChatLine::Info(format!("  ✗ {e}"))),
            }
        }
        "paths" => {
            app.push(ChatLine::Info("  Skill search paths:".into()));
            for (p, k) in
                clido_core::skills::resolve_skill_directories(&app.workspace_root, &cfg.extra_paths)
            {
                let label = match k {
                    clido_core::skills::SkillSourceKind::Workspace => "workspace",
                    clido_core::skills::SkillSourceKind::Global => "global",
                    clido_core::skills::SkillSourceKind::Extra => "extra",
                };
                let st = if p.is_dir() { "" } else { " (missing)" };
                app.push(ChatLine::Info(format!("    [{label}]{st} {}", p.display())));
            }
            if !cfg.registry_urls.is_empty() {
                app.push(ChatLine::Info("  Registry URLs (reserved):".into()));
                for u in &cfg.registry_urls {
                    app.push(ChatLine::Info(format!("    {u}")));
                }
            }
        }
        s if s.starts_with("disable ") => {
            let id = s.trim_start_matches("disable ").trim();
            if id.is_empty() {
                app.push(ChatLine::Info("  Usage: /skills disable <id>".into()));
                return;
            }
            match clido_core::set_skill_disabled_in_project(&app.workspace_root, id, true) {
                Ok(()) => app.push(ChatLine::Info(format!(
                    "  ✓ Disabled `{id}` — restart session to apply."
                ))),
                Err(e) => app.push(ChatLine::Info(format!("  ✗ {e}"))),
            }
        }
        s if s.starts_with("enable ") => {
            let id = s.trim_start_matches("enable ").trim();
            if id.is_empty() {
                app.push(ChatLine::Info("  Usage: /skills enable <id>".into()));
                return;
            }
            match clido_core::set_skill_disabled_in_project(&app.workspace_root, id, false) {
                Ok(()) => app.push(ChatLine::Info(format!(
                    "  ✓ `{id}` no longer disabled — restart session to apply."
                ))),
                Err(e) => app.push(ChatLine::Info(format!("  ✗ {e}"))),
            }
        }
        _ => {
            app.push(ChatLine::Info("  /skills commands:".into()));
            app.push(ChatLine::Info(
                "    /skills list          — show discovered skills".into(),
            ));
            app.push(ChatLine::Info(
                "    /skills paths         — show search directories".into(),
            ));
            app.push(ChatLine::Info(
                "    /skills disable <id>  — project config".into(),
            ));
            app.push(ChatLine::Info(
                "    /skills enable <id>   — remove from disabled".into(),
            ));
            app.push(ChatLine::Info(
                "  Config: [skills] in .clido/config.toml (enabled, extra-paths, …)".into(),
            ));
        }
    }
}

pub(super) fn cmd_todo(app: &mut App) {
    let todos = app.todo_store.lock().map(|g| g.clone()).unwrap_or_default();
    if todos.is_empty() {
        app.push(ChatLine::Info(
            "  No tasks yet — the agent will create a task list while working".into(),
        ));
    } else {
        app.push(ChatLine::Info(format!(
            "  Tasks ({} item{})  ▶ = in progress  ✓ = done  ✗ = blocked  ! = high priority:",
            todos.len(),
            if todos.len() == 1 { "" } else { "s" }
        )));
        for item in &todos {
            let icon = match item.status {
                clido_tools::TodoStatus::Done => "✓",
                clido_tools::TodoStatus::InProgress => "▶",
                clido_tools::TodoStatus::Blocked => "✗",
                clido_tools::TodoStatus::Pending => "○",
            };
            let pri = match item.priority {
                clido_tools::TodoPriority::High => "!",
                clido_tools::TodoPriority::Medium => " ",
                clido_tools::TodoPriority::Low => "·",
            };
            app.push(ChatLine::Info(format!(
                "  {} [{}] {}  {}",
                icon, pri, item.id, item.content
            )));
        }
    }
}

pub(super) fn cmd_undo(app: &mut App) {
    app.send_now(
        "Undo the last committed change.\n\
        \n\
        Steps:\n\
        1. Run `git log --oneline -5` to show the 5 most recent commits.\n\
        2. Run `git status` to check for any uncommitted changes.\n\
        3. Ask the user to confirm before running any reset command.\n\
        4. If there is a recent commit to undo, run `git reset --soft HEAD~1` to \
           undo the last commit and keep the changes staged.\n\
        5. Show what files are now staged and a brief summary of what was undone.\n\
        6. If there are only uncommitted changes (nothing committed yet), \
           ask the user which files to restore before acting."
            .to_string(),
    );
}

pub(super) fn cmd_rollback(app: &mut App, cmd: &str) {
    let id = cmd.trim_start_matches("/rollback").trim();
    if id.is_empty() {
        app.send_now(
            "Show available checkpoints for this session.\n\
            \n\
            Steps:\n\
            1. List checkpoints in `.clido/checkpoints/` if the directory exists.\n\
            2. Also run `git log --oneline -10` to show recent git history.\n\
            3. Report both lists so the user can choose what to roll back to.\n\
            4. Ask the user which checkpoint or commit hash to restore, \
               then wait for their input."
                .to_string(),
        );
    } else {
        let id = id.to_string();
        app.send_now(format!(
            "Roll back to checkpoint or commit `{id}`.\n\
            \n\
            Steps:\n\
            1. Check if `{id}` looks like a git commit hash (7-40 hex chars) or a \
               checkpoint ID (starts with `ck_`).\n\
            2. For a git commit hash: run `git status` first and show any uncommitted changes.\n\
            3. Ask the user for explicit confirmation before any destructive rollback.\n\
            4. If confirmed, create a safety backup (for example `git branch backup/before-rollback`) \
               before running `git reset --hard {id}`.\n\
            5. For a checkpoint ID: restore from `.clido/checkpoints/{id}/manifest.json` \
               by reading the manifest and restoring each listed file from its blob.\n\
            6. Show a summary of what was restored."
        ));
    }
}

fn task_strip_vis_label(vis: PlanPanelVisibility) -> &'static str {
    match vis {
        PlanPanelVisibility::On => "on",
        PlanPanelVisibility::Off => "off",
        PlanPanelVisibility::Auto => "auto",
    }
}

fn status_rail_vis_label(vis: StatusRailVisibility) -> &'static str {
    match vis {
        StatusRailVisibility::On => "on",
        StatusRailVisibility::Off => "off",
        StatusRailVisibility::Auto => "auto",
    }
}

fn apply_task_strip_mode(app: &mut App, sub: &str) {
    match sub {
        "on" => {
            app.plan_panel_visibility = PlanPanelVisibility::On;
            app.push(ChatLine::Info(
                "  Task list: on — always when the layout fits, even if empty.".into(),
            ));
        }
        "off" => {
            app.plan_panel_visibility = PlanPanelVisibility::Off;
            app.push(ChatLine::Info("  Task list: off.".into()));
        }
        "auto" => {
            app.plan_panel_visibility = PlanPanelVisibility::Auto;
            app.push(ChatLine::Info(
                "  Task list: auto — on larger terminals when there is something to list (default)."
                    .into(),
            ));
        }
        _ => {
            app.push(ChatLine::Info(
                "  Usage: /tasks on | off | auto   (same as /panel tasks …)".into(),
            ));
        }
    }
}

fn apply_status_rail_mode(app: &mut App, sub: &str) {
    match sub {
        "on" => {
            app.status_rail_visibility = StatusRailVisibility::On;
            app.push(ChatLine::Info(
                "  Side panel: on — right column from a slightly lower width than auto.".into(),
            ));
        }
        "off" => {
            app.status_rail_visibility = StatusRailVisibility::Off;
            app.push(ChatLine::Info(
                "  Side panel: off — full-width chat; status, tasks, and queue stack at the bottom."
                    .into(),
            ));
        }
        "auto" => {
            app.status_rail_visibility = StatusRailVisibility::Auto;
            app.push(ChatLine::Info(
                "  Side panel: auto — right column when the terminal is wide enough (default)."
                    .into(),
            ));
        }
        _ => {
            app.push(ChatLine::Info("  Usage: /panel on | off | auto".into()));
        }
    }
}

/// Right-hand status column: session, git, agent, queue, task block, tools (`/panel`).
pub(super) fn cmd_panel(app: &mut App, cmd: &str) {
    use crate::tui::render::{STATUS_RAIL_MIN_TERM_WIDTH, STATUS_RAIL_MIN_TERM_WIDTH_ON};
    let rest = cmd.trim_start_matches("/panel").trim();
    let parts: Vec<&str> = rest.split_whitespace().collect();
    match parts.as_slice() {
        [] => {
            let rail = status_rail_vis_label(app.status_rail_visibility);
            let tasks = task_strip_vis_label(app.plan_panel_visibility);
            app.push(ChatLine::Info(format!(
                "  Side panel: {rail}  ·  /panel on|off|auto   (auto ≥{} cols, on ≥{})",
                STATUS_RAIL_MIN_TERM_WIDTH, STATUS_RAIL_MIN_TERM_WIDTH_ON
            )));
            app.push(ChatLine::Info(format!(
                "  Task list: {tasks}  ·  /tasks on|off|auto   (alias /progress)"
            )));
        }
        ["tasks"] => {
            let tasks = task_strip_vis_label(app.plan_panel_visibility);
            app.push(ChatLine::Info(format!(
                "  Task list: {tasks}  ·  /tasks on | off | auto"
            )));
        }
        ["tasks", sub] => apply_task_strip_mode(app, sub),
        ["side"] | ["rail"] => {
            let rail = status_rail_vis_label(app.status_rail_visibility);
            app.push(ChatLine::Info(format!(
                "  Side panel: {rail}  ·  /panel on | off | auto"
            )));
        }
        ["side", sub] | ["rail", sub] => apply_status_rail_mode(app, sub),
        [sub] => apply_status_rail_mode(app, sub),
        _ => {
            app.push(ChatLine::Info(
                "  Usage: /panel   ·  /panel on|off|auto   ·  /panel tasks on|off|auto".into(),
            ));
        }
    }
}

/// Task list strip only — todos, planner snapshot, harness, live step (`/tasks`).
pub(super) fn cmd_tasks(app: &mut App, cmd: &str) {
    let sub = cmd.trim_start_matches("/tasks").trim();
    match sub {
        "" => {
            let vis = task_strip_vis_label(app.plan_panel_visibility);
            app.push(ChatLine::Info(format!(
                "  Task list: {vis}  ·  /tasks on | off | auto"
            )));
        }
        _ => apply_task_strip_mode(app, sub),
    }
}

/// Back-compat alias for [`cmd_tasks`] (`/progress` …).
pub(super) fn cmd_progress_strip(app: &mut App, cmd: &str) {
    let sub = cmd.trim_start_matches("/progress").trim();
    match sub {
        "" => {
            let vis = task_strip_vis_label(app.plan_panel_visibility);
            app.push(ChatLine::Info(format!(
                "  Task list: {vis}  ·  /tasks on|off|auto   ·  /panel toggles the whole side column"
            )));
        }
        _ => apply_task_strip_mode(app, sub),
    }
}

pub(super) fn cmd_plan(app: &mut App, cmd: &str) {
    let sub = cmd.trim_start_matches("/plan").trim().to_string();
    match sub.as_str() {
        "edit" => {
            if let Some(raw) = app.plan.last_plan_raw.clone() {
                app.plan.text_editor = Some(PlanTextEditor::from_raw(&raw));
            } else if let Some(plan) = app.plan.last_plan_snapshot.clone() {
                let raw = plan
                    .tasks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("Step {}: {}", i + 1, t.description))
                    .collect::<Vec<_>>()
                    .join("\n");
                app.plan.text_editor = Some(PlanTextEditor::from_raw(&raw));
            } else if let Some(tasks) = app.plan.last_plan.clone() {
                // fallback for plans from --plan mode (no raw text available)
                let raw = tasks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("Step {}: {}", i + 1, t))
                    .collect::<Vec<_>>()
                    .join("\n");
                app.plan.text_editor = Some(PlanTextEditor::from_raw(&raw));
            } else {
                app.push(ChatLine::Info(
                    "  ✗ No plan yet — use /plan <task> to create one".into(),
                ));
            }
        }
        "save" => {
            if let Some(ref editor) = app.plan.editor {
                app.plan.last_plan_snapshot = Some(editor.plan.clone());
                app.plan.last_plan = Some(
                    editor
                        .plan
                        .tasks
                        .iter()
                        .map(|t| t.description.clone())
                        .collect::<Vec<_>>(),
                );
                match clido_planner::save_plan(&app.workspace_root, &editor.plan) {
                    Ok(path) => app.push(ChatLine::Info(format!(
                        "  ✓ Plan saved: {}",
                        path.display()
                    ))),
                    Err(e) => {
                        app.overlay_stack
                            .push(OverlayKind::Error(ErrorOverlay::from_message(format!(
                                "Could not save plan: {}",
                                e
                            ))))
                    }
                }
            } else if let Some(ref plan) = app.plan.last_plan_snapshot {
                match clido_planner::save_plan(&app.workspace_root, plan) {
                    Ok(path) => app.push(ChatLine::Info(format!(
                        "  ✓ Plan saved: {}",
                        path.display()
                    ))),
                    Err(e) => {
                        app.overlay_stack
                            .push(OverlayKind::Error(ErrorOverlay::from_message(format!(
                                "Could not save plan: {}",
                                e
                            ))))
                    }
                }
            } else {
                app.push(ChatLine::Info("  ✗ No active plan to save".into()));
            }
        }
        "list" => match clido_planner::list_plans(&app.workspace_root) {
            Ok(summaries) if summaries.is_empty() => {
                app.push(ChatLine::Info(
                    "  No saved plans — use /plan <task> to create and /plan save to save".into(),
                ));
            }
            Ok(summaries) => {
                app.push(ChatLine::Info(format!(
                    "  Saved plans ({}):",
                    summaries.len()
                )));
                for s in &summaries {
                    let done_frac = if s.task_count > 0 {
                        format!("{}/{}", s.done, s.task_count)
                    } else {
                        "—".to_string()
                    };
                    app.push(ChatLine::Info(format!(
                        "  {}  [{} done]  {}",
                        {
                            let g = &s.goal;
                            if g.chars().count() > 58 {
                                format!("{}…", g.chars().take(57).collect::<String>())
                            } else {
                                g.clone()
                            }
                        },
                        done_frac,
                        s.id
                    )));
                }
                app.push(ChatLine::Info(
                    "  Use /rollback <id> to restore a plan checkpoint".into(),
                ));
            }
            Err(e) => {
                app.overlay_stack
                    .push(OverlayKind::Error(ErrorOverlay::from_message(format!(
                        "list plans: {}",
                        e
                    ))));
            }
        },
        "on" | "off" | "auto" => {
            app.push(ChatLine::Info(format!(
                "  `/plan` is for planning (e.g. `/plan <task>`). For layout: `/panel` (side column) or `/tasks` (task strip) — not `/plan {sub}`."
            )));
        }
        "" => {
            // /plan with no task — show existing plan if any
            if let Some(plan) = app.plan.last_plan_snapshot.clone() {
                if plan.tasks.is_empty() {
                    app.push(ChatLine::Info(
                        "  Usage: /plan <task>  — have the agent plan before executing".into(),
                    ));
                    return;
                }
                app.push(ChatLine::Info("  ┌─ Current plan:".into()));
                let count = plan.tasks.len();
                for (i, t) in plan.tasks.iter().enumerate() {
                    let prefix = if i + 1 == count {
                        "  └─"
                    } else {
                        "  ├─"
                    };
                    app.push(ChatLine::Info(format!("{} {}", prefix, t.description)));
                }
            } else {
                match app.plan.last_plan.clone() {
                    Some(tasks) if !tasks.is_empty() => {
                        app.push(ChatLine::Info("  ┌─ Current plan:".into()));
                        let count = tasks.len();
                        for (i, t) in tasks.iter().enumerate() {
                            let prefix = if i + 1 == count {
                                "  └─"
                            } else {
                                "  ├─"
                            };
                            app.push(ChatLine::Info(format!("{} {}", prefix, t)));
                        }
                    }
                    _ => {
                        app.push(ChatLine::Info(
                            "  Usage: /plan <task>  — have the agent plan before executing".into(),
                        ));
                    }
                }
            }
        }
        task => {
            // /plan <task> — ask the agent to plan first, then wait for confirmation
            let task = task.to_string();
            // Echo the user's /plan command to the chat with highlighted formatting
            app.push(ChatLine::SlashCommand {
                cmd: "/plan".to_string(),
                text: Some(task.clone()),
            });
            app.plan.awaiting_plan_response = true;
            let prompt = format!(
                "Create a plan for the following task using **exactly** these sections (in order):\n\
                 1. Goal\n\
                 2. Current State\n\
                 3. Problems / Gaps\n\
                 4. Approach\n\
                 5. Steps — numbered \"Step N: …\" with clear, actionable todos\n\
                 6. Risks / Edge Cases\n\n\
                 After section 5, call **TodoWrite** with one todo per step (same order). \
                 Present the **complete** plan and todos, then **STOP** — do not implement \
                 anything until the user explicitly confirms.\n\nTask: {task}"
            );
            app.send_silent(prompt);
        }
    }
}

pub(super) fn cmd_branch(app: &mut App, cmd: &str) {
    let name = cmd.trim_start_matches("/branch").trim().to_string();
    if name.is_empty() {
        app.push(ChatLine::Info("  Usage: /branch <name>".into()));
        app.push(ChatLine::Info(
            "  creates a new branch and switches to it".into(),
        ));
    } else {
        app.send_now(format!(
            "Create and switch to a new git branch named `{name}`.\n\
            \n\
            Steps:\n\
            1. Verify this is a git repo. Stop if not.\n\
            2. Check for uncommitted changes with `git status`. If there are any, \
               stash them first (`git stash`) so the branch switch is clean.\n\
            3. Create and switch: `git checkout -b {name}`.\n\
            4. If the stash was created, pop it: `git stash pop`. \
               If the pop causes conflicts, show them clearly and stop.\n\
            5. Push the branch and set upstream: `git push -u origin {name}`.\n\
            6. Report the new branch name and current status."
        ));
    }
}

pub(super) fn cmd_sync(app: &mut App) {
    app.send_now(
        "Sync the current branch with its upstream.\n\
        \n\
        Steps:\n\
        1. Verify this is a git repo. Stop if not.\n\
        2. Run `git status` — if there are uncommitted changes, stash them first \
           (`git stash`).\n\
        3. Run `git fetch origin`.\n\
        4. Run `git rebase origin/<current-branch>` (use `git rev-parse \
           --abbrev-ref HEAD` to get the branch name).\n\
        5. If rebase has conflicts: show which files conflict, attempt to resolve \
           straightforward ones (whitespace, formatting), then `git rebase --continue`. \
           If conflicts are non-trivial, stop and explain what needs manual resolution.\n\
        6. If a stash was created, pop it: `git stash pop`.\n\
        7. Report how many commits were rebased and the current HEAD."
            .to_string(),
    );
}

pub(super) fn cmd_pr(app: &mut App, cmd: &str) {
    let title_arg = cmd.trim_start_matches("/pr").trim().to_string();
    let title_instruction = if title_arg.is_empty() {
        "Generate a PR title (≤70 chars, imperative mood) and body from the branch diff."
            .to_string()
    } else {
        format!("Use this as the PR title: {title_arg}")
    };
    app.send_now(format!(
        "Create a pull request for the current branch.\n\
        \n\
        Steps:\n\
        1. Verify this is a git repo with a remote. Stop if not.\n\
        2. Check `git status` — if there are uncommitted changes, ask whether to \
           ship them first (run /ship) or proceed with existing commits.\n\
        3. Get the current branch: `git rev-parse --abbrev-ref HEAD`. \
           If it's main or master, warn and stop — PRs should come from a feature branch.\n\
        4. Get the default base branch (try `git symbolic-ref refs/remotes/origin/HEAD` \
           or fall back to `main`).\n\
        5. Run `git log <base>..<current> --oneline` and \
           `git diff <base>..<current> --stat` to understand the changes.\n\
        6. {title_instruction}\n\
           For the body, write:\n\
           - ## Summary — 2–4 bullet points of what changed and why\n\
           - ## Test plan — what to verify\n\
        7. Make sure the branch is pushed: `git push -u origin <branch>` if needed.\n\
        8. Create the PR: `gh pr create --title \"<title>\" --body \"<body>\" \
           --base <base>`.\n\
           If `gh` is not available, print the title and body and tell the user \
           to create the PR manually.\n\
        9. Print the PR URL."
    ));
}

pub(super) fn cmd_ship(app: &mut App, cmd: &str) {
    let custom_msg = cmd.trim_start_matches("/ship").trim();
    let msg_instruction = if custom_msg.is_empty() {
        "Generate a commit message from the staged diff: imperative mood, ≤72 chars subject, \
         body only if the change is complex. Append trailer: \
         `Co-Authored-By: Claude <noreply@clido.ai>`"
            .to_string()
    } else {
        format!("Use this commit message verbatim: {custom_msg}")
    };
    app.send_now(format!(
        "Git ship: stage all changes and push.\n\
        \n\
        Steps:\n\
        1. Verify this is a git repo (`git rev-parse --git-dir`). Stop if not.\n\
        2. Run `git status` — if nothing to commit, report and stop.\n\
        3. Run `git diff HEAD` and `git status -s` to understand changes.\n\
        4. Warn and skip any sensitive files (*.env, *secret*, *credential*, *password*) \
           before staging.\n\
        5. `git add -A` (excluding sensitive files).\n\
        6. {msg_instruction}\n\
        7. `git commit -m \"<message>\"` — if it fails (hook, lint, tests):\n\
           - Read the error, fix the root cause (format/lint/test as needed).\n\
           - Re-stage affected files and retry the commit.\n\
           - Repeat up to 3 attempts total. Never use --no-verify.\n\
           - If still failing after 3 attempts, explain what is blocking and stop.\n\
        8. `git push` — if rejected:\n\
           - Diverged history: `git pull --rebase origin <branch>` then push again.\n\
           - No upstream: `git push -u origin <branch>`.\n\
           - Never force-push to main or master.\n\
        9. Report the commit hash and pushed branch."
    ));
}

pub(super) fn cmd_save(app: &mut App, cmd: &str) {
    let custom_msg = cmd.trim_start_matches("/save").trim();
    let msg_instruction = if custom_msg.is_empty() {
        "Generate a commit message from the staged diff: imperative mood, ≤72 chars subject, \
         body only if the change is complex. Append trailer: \
         `Co-Authored-By: Claude <noreply@clido.ai>`"
            .to_string()
    } else {
        format!("Use this commit message verbatim: {custom_msg}")
    };
    app.send_now(format!(
        "Git save: stage all changes and commit locally (no push).\n\
        \n\
        Steps:\n\
        1. Verify this is a git repo (`git rev-parse --git-dir`). Stop if not.\n\
        2. Run `git status` — if nothing to commit, report and stop.\n\
        3. Run `git diff HEAD` and `git status -s` to understand changes.\n\
        4. Warn and skip any sensitive files (*.env, *secret*, *credential*, *password*) \
           before staging.\n\
        5. `git add -A` (excluding sensitive files).\n\
        6. {msg_instruction}\n\
        7. `git commit -m \"<message>\"` — if it fails (hook, lint, tests):\n\
           - Read the error, fix the root cause (format/lint/test as needed).\n\
           - Re-stage affected files and retry the commit.\n\
           - Repeat up to 3 attempts total. Never use --no-verify.\n\
           - If still failing after 3 attempts, explain what is blocking and stop.\n\
        8. Report the commit hash and message."
    ));
}

pub(super) fn cmd_notify(app: &mut App, cmd: &str) {
    let arg = cmd.trim_start_matches("/notify").trim();
    match arg {
        "on" => {
            app.notify_enabled = true;
            app.push(ChatLine::Info("  ✓ Notifications enabled".into()));
        }
        "off" => {
            app.notify_enabled = false;
            app.push(ChatLine::Info("  ✓ Notifications disabled".into()));
        }
        "" => {
            app.notify_enabled = !app.notify_enabled;
            let state = if app.notify_enabled { "on" } else { "off" };
            app.push(ChatLine::Info(format!("  Notifications {}", state)));
        }
        _ => {
            app.push(ChatLine::Info("  Usage: /notify [on|off]".into()));
        }
    }
}

pub(super) fn cmd_index(app: &mut App) {
    let db_path = app.workspace_root.join(".clido").join("index.db");
    if !db_path.exists() {
        app.push(ChatLine::Info(
            "  Index not built — run `clido index build` in a terminal to enable code search.  \
    Once built, the agent can search by concept rather than just filename."
                .into(),
        ));
    } else {
        match RepoIndex::open(&db_path) {
            Ok(index) => match index.stats() {
                Ok((files, symbols)) => {
                    app.push(ChatLine::Info(format!(
                        "  Index: {} files, {} symbols  (refresh: `clido index build`)",
                        files, symbols
                    )));
                }
                Err(e) => {
                    app.push(ChatLine::Info(format!("  ✗ Index error: {}", e)));
                }
            },
            Err(e) => {
                app.push(ChatLine::Info(format!("  ✗ Index unavailable: {}", e)));
            }
        }
    }
}

pub(super) fn cmd_rules(app: &mut App, args: &str) {
    // Parse args for "edit" or "edit global"
    let args_trimmed = args.trim();
    let edit_global = args_trimmed == "edit global";
    let edit_local = args_trimmed == "edit";

    if edit_local || edit_global {
        cmd_rules_edit(app, edit_global);
        return;
    }

    // Original /rules command (show rules)
    app.push(ChatLine::Info("".into()));
    app.push(ChatLine::Section("Project rules (.clido/rules.md)".into()));

    let (no_rules, rules_override) = match clido_core::load_config(&app.workspace_root) {
        Ok(loaded) => {
            let no = loaded.agent.no_rules;
            let ro = loaded.agent.rules_file.as_ref().map(|s| {
                let p = Path::new(s.as_str());
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    app.workspace_root.join(p)
                }
            });
            (no, ro)
        }
        Err(_) => (false, None),
    };

    if no_rules {
        app.push(ChatLine::Info(
            "  no-rules is on in config — project rules files are not loaded into the agent."
                .into(),
        ));
        app.push(ChatLine::Info(
            "  (Unrelated: /enhance <text> can still rewrite a prompt before you send it.)".into(),
        ));
        app.push(ChatLine::Info("".into()));
        return;
    }

    let files = clido_context::discover_rules(
        app.workspace_root.as_path(),
        false,
        rules_override.as_deref(),
    );

    if files.is_empty() {
        app.push(ChatLine::Info(
            "  No rules files found. The agent looks for, in order:".into(),
        ));
        app.push(ChatLine::Info(
            "    ~/.config/clido/rules.md  (global)".into(),
        ));
        app.push(ChatLine::Info(
            "    … then walking up from the workspace: .clido/rules.md, CLIDO.md".into(),
        ));
        app.push(ChatLine::Info(
            "  Tip: /enhance <prompt> is separate — it structures your message, not project rules."
                .into(),
        ));
    } else {
        const PREVIEW_CHARS: usize = 1_200;
        const PREVIEW_LINES: usize = 18;
        for f in &files {
            let total = f.content.chars().count();
            app.push(ChatLine::Info(format!(
                "  {}  ({} chars)",
                f.path.display(),
                total
            )));
            let snippet: String = f.content.chars().take(PREVIEW_CHARS).collect();
            let lines: Vec<&str> = snippet.lines().take(PREVIEW_LINES).collect();
            for line in lines {
                app.push(ChatLine::Info(format!("  │ {}", line)));
            }
            if total > snippet.chars().count() || f.content.lines().count() > PREVIEW_LINES {
                app.push(ChatLine::Info(
                    "  │ … truncated in chat — open the file for the full rules".into(),
                ));
            }
            app.push(ChatLine::Info("".into()));
        }
        app.push(ChatLine::Info(
            "  Order: global first, then parents → workspace (later files override in the prompt)."
                .into(),
        ));
    }
    app.push(ChatLine::Info("".into()));
}

/// Edit rules file - local or global
fn cmd_rules_edit(app: &mut App, global: bool) {
    if global {
        // Global rules: ~/.config/clido/rules.md
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"));
        let global_rules_path = match home {
            Ok(home) => PathBuf::from(home)
                .join(".config")
                .join("clido")
                .join("rules.md"),
            Err(_) => {
                app.push(ChatLine::Info(
                    "  ✗ Could not determine home directory".into(),
                ));
                return;
            }
        };

        // Ensure config directory exists
        let global_rules_dir = global_rules_path.parent().unwrap_or(&global_rules_path);
        if let Err(e) = std::fs::create_dir_all(global_rules_dir) {
            app.push(ChatLine::Info(format!(
                "  ✗ Failed to create config directory: {}",
                e
            )));
            return;
        }

        // Load current content or start with empty
        let current_content = std::fs::read_to_string(&global_rules_path).unwrap_or_default();

        // Send prompt to agent to edit rules
        let prompt = format!(
            "Please review and improve the following global clido rules. \
            Global rules apply to all clido sessions. Current content:\n\n```\n{}\n```\n\n\
            Please provide the improved rules. You can:\n\
            - Fix formatting issues\n\
            - Add missing guidelines\n\
            - Remove outdated rules\n\
            - Improve clarity\n\n\
            Return ONLY the rules content, no explanations.",
            if current_content.is_empty() {
                "(No rules yet - create initial rules)"
            } else {
                &current_content
            }
        );

        app.push(ChatLine::Info(
            "  Editing global rules (~/.config/clido/rules.md)...".into(),
        ));
        app.send_silent(prompt);

        // Store path for later saving
        app.pending_rules_edit = Some(global_rules_path);
    } else {
        // Local rules: CLIDO.md in workspace root
        let local_rules_path = app.workspace_root.join("CLIDO.md");

        // Load current content or start with empty
        let current_content = std::fs::read_to_string(&local_rules_path).unwrap_or_default();

        // Send prompt to agent to edit rules
        let prompt = format!(
            "Please review and improve the following project-specific clido rules. \
            These rules apply only to the current project. Current content:\n\n```\n{}\n```\n\n\
            Please provide the improved rules. You can:\n\
            - Fix formatting issues\n\
            - Add project-specific guidelines\n\
            - Remove outdated rules\n\
            - Improve clarity\n\n\
            Return ONLY the rules content, no explanations.",
            if current_content.is_empty() {
                "(No rules yet - create initial rules)"
            } else {
                &current_content
            }
        );

        app.push(ChatLine::Info(
            "  Editing local rules (.clido/rules.md)...".into(),
        ));
        app.send_silent(prompt);

        // Store path for later saving
        app.pending_rules_edit = Some(local_rules_path);
    }
}

pub(super) fn cmd_image(app: &mut App, cmd: &str) {
    let path_str = cmd.trim_start_matches("/image").trim();
    if path_str.is_empty() {
        app.push(ChatLine::Info(
            "  Usage: /image <path>  (attach an image to the next message)".into(),
        ));
    } else {
        let path = std::path::Path::new(path_str);
        match crate::image_input::ImageAttachment::from_path(path) {
            Some(att) => {
                let info = att.info_line();
                app.pending_image = Some(att);
                app.push(ChatLine::Info(format!("  {}", info)));
            }
            None => {
                app.push(ChatLine::Info(format!(
                    "  ✗ Could not load image '{}' — supported: PNG, JPEG, GIF, WebP",
                    path_str
                )));
            }
        }
    }
}

/// Add an external path to the allowed list for this session.
/// Tools will be able to read/write files in these paths in addition to the workspace.
pub(super) fn cmd_allow_path(app: &mut App, cmd: &str) {
    let path_str = cmd.trim_start_matches("/allow-path").trim();
    if path_str.is_empty() {
        app.push(ChatLine::Info(
            "  Usage: /allow-path <path>  (allow agent to access files outside workspace)".into(),
        ));
        return;
    }

    let path = std::path::Path::new(path_str);

    // Expand ~ to home directory
    let expanded: std::path::PathBuf = if let Some(rest) = path_str.strip_prefix("~/") {
        if let Some(home) = std::env::home_dir() {
            home.join(rest)
        } else {
            app.push(ChatLine::Info(
                "  ✗ Could not determine home directory".into(),
            ));
            return;
        }
    } else {
        path.to_path_buf()
    };

    // Check if path exists
    if !expanded.exists() {
        app.push(ChatLine::Info(format!(
            "  ✗ Path does not exist: {}",
            expanded.display()
        )));
        return;
    }

    // Canonicalize to resolve symlinks and get absolute path
    let canonical: std::path::PathBuf = match expanded.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            app.push(ChatLine::Info(format!(
                "  ✗ Could not resolve path: {} ({})",
                expanded.display(),
                e
            )));
            return;
        }
    };

    // Check if path is already in workspace (redundant but allowed)
    if canonical.starts_with(&app.workspace_root) {
        app.push(ChatLine::Info(format!(
            "  ℹ Path is already within workspace: {}",
            canonical.display()
        )));
        return;
    }

    // Check if already allowed
    if app.allowed_external_paths.contains(&canonical) {
        app.push(ChatLine::Info(format!(
            "  ℹ Path already allowed: {}",
            canonical.display()
        )));
        return;
    }

    // Add to allowed list
    app.allowed_external_paths.push(canonical.clone());

    // Send updated paths to agent task
    let _ = app
        .channels
        .allowed_paths_tx
        .send(app.allowed_external_paths.clone());

    app.push(ChatLine::Info(format!(
        "  ✓ Allowed external path: {}",
        canonical.display()
    )));
}

/// List all externally allowed paths for this session.
pub(super) fn cmd_allowed_paths(app: &mut App) {
    let paths: Vec<std::path::PathBuf> = app.allowed_external_paths.clone();
    if paths.is_empty() {
        app.push(ChatLine::Info(
            "  No external paths allowed. Use /allow-path <path> to add one.".into(),
        ));
        return;
    }

    app.push(ChatLine::Info(format!(
        "  {} external path(s) allowed this session:",
        paths.len()
    )));
    for (i, path) in paths.iter().enumerate() {
        app.push(ChatLine::Info(format!("    {}. {}", i + 1, path.display())));
    }
    app.push(ChatLine::Info("".into()));
    app.push(ChatLine::Info(
        "  Use /allow-path <path> to add more, or restart to clear.".into(),
    ));
}

pub(super) fn cmd_agents(app: &mut App) {
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
        Ok(loaded) => {
            let profile = loaded.profiles.get(&loaded.default_profile);
            app.push(ChatLine::Info("  Agent configuration:".into()));
            if let Some(p) = profile {
                app.push(ChatLine::Info(format!(
                    "  main      {} / {}",
                    p.provider, p.model
                )));
                if let Some(ref fast) = p.fast {
                    app.push(ChatLine::Info(format!(
                        "  fast      {} / {}",
                        fast.provider, fast.model
                    )));
                } else {
                    app.push(ChatLine::Info(
                        "  fast      not set  (uses main provider)".into(),
                    ));
                }
            }
            app.push(ChatLine::Info(
                "  Worker and reviewer sub-agents use the fast provider (or main if not set)."
                    .into(),
            ));
            app.push(ChatLine::Info("  Run /init to reconfigure.".into()));
        }
    }
}

pub(super) fn cmd_profiles(app: &mut App) {
    // Open the interactive profile picker (same as /profile)
    cmd_profile(app);
}

pub(super) fn cmd_profile(app: &mut App) {
    // No name given → open interactive profile picker.
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
        Ok(loaded) => {
            let active = loaded.default_profile.clone();
            let mut profiles: Vec<(String, clido_core::ProfileEntry)> =
                loaded.profiles.into_iter().collect();
            profiles.sort_by(|a, b| a.0.cmp(&b.0));
            let selected = profiles.iter().position(|(n, _)| n == &active).unwrap_or(0);
            let mut picker = ListPicker::new(profiles, 12);
            picker.selected = selected;
            app.profile_picker = Some(ProfilePickerState { picker, active });
        }
    }
}

pub(super) fn cmd_profile_edit(app: &mut App, cmd: &str) {
    let arg = cmd.trim_start_matches("/profile edit").trim();
    let name = if arg.is_empty() {
        app.current_profile.clone()
    } else {
        arg.to_string()
    };
    let config_path = clido_core::global_config_path()
        .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => {
            app.push(ChatLine::Info(format!("  ✗ Could not load config: {e}")));
        }
        Ok(loaded) => match loaded.profiles.get(&name).cloned() {
            None => {
                app.push(ChatLine::Info(format!(
                    "  ✗ Profile '{}' not found. Use /profiles to list.",
                    name
                )));
            }
            Some(entry) => {
                let all_profiles = loaded.profiles.clone();
                app.profile_overlay = Some(ProfileOverlayState::for_edit(
                    name,
                    &entry,
                    config_path,
                    &all_profiles,
                ));
            }
        },
    }
}

pub(super) fn cmd_profile_switch(app: &mut App, cmd: &str) {
    let name = cmd.trim_start_matches("/profile ").trim();
    switch_profile_seamless(app, name);
}

pub(super) fn cmd_profile_delete(app: &mut App, cmd: &str) {
    let name = cmd.trim_start_matches("/profile delete").trim();
    if name.is_empty() {
        app.push(ChatLine::Info("  Usage: /profile delete <name>".into()));
        return;
    }
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {e}"))),
        Ok(loaded) => {
            if !loaded.profiles.contains_key(name) {
                app.push(ChatLine::Info(format!(
                    "  ✗ Profile '{}' not found. Use /profiles to list.",
                    name
                )));
            } else if name == loaded.default_profile {
                app.push(ChatLine::Info(format!(
                    "  ✗ Cannot delete the active profile '{}'. Switch to another profile first.",
                    name
                )));
            } else {
                let config_path = clido_core::global_config_path()
                    .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                match clido_core::delete_profile_from_config(&config_path, name) {
                    Ok(()) => {
                        app.push(ChatLine::Info(format!("  ✓ Profile '{}' deleted.", name)));
                    }
                    Err(e) => {
                        app.push(ChatLine::Info(format!(
                            "  ✗ Failed to delete profile '{}': {e}",
                            name
                        )));
                    }
                }
            }
        }
    }
}

pub(super) fn cmd_config(app: &mut App) {
    // Show a complete, structured overview of all current settings.
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
        Ok(loaded) => {
            // Config file location.
            let global_path = clido_core::global_config_path();
            let project_path_opt = app.workspace_root.join(".clido/config.toml");
            let project_exists = project_path_opt.exists();
            let config_file_label = if project_exists {
                format!("{}", project_path_opt.display())
            } else {
                global_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.config/clido/config.toml".to_string())
            };

            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Active Profile".into()));
            let active = &loaded.default_profile;
            if let Some(p) = loaded.profiles.get(active) {
                let key_status = if p.api_key.is_some() {
                    "key ✓ (in config)"
                } else if p.api_key_env.is_some() {
                    "key ✓ (from env)"
                } else {
                    "key ✗ (not set)"
                };
                app.push(ChatLine::Info(format!(
                    "  {} — {} / {}  {}",
                    active, p.provider, p.model, key_status
                )));
                if let Some(ref url) = p.base_url {
                    app.push(ChatLine::Info(format!("  Custom endpoint: {}", url)));
                }
            }

            // All profiles.
            if loaded.profiles.len() > 1 {
                app.push(ChatLine::Info("".into()));
                app.push(ChatLine::Section("All Profiles".into()));
                let mut names: Vec<&String> = loaded.profiles.keys().collect();
                names.sort();
                for name in names {
                    let p = &loaded.profiles[name];
                    let marker = if name == active { "▶" } else { " " };
                    let fast_s = if let Some(ref f) = p.fast {
                        format!("  fast: {}/{}", f.provider, f.model)
                    } else {
                        String::new()
                    };
                    app.push(ChatLine::Info(format!(
                        "  {} {:<14}  {}/{}{}",
                        marker, name, p.provider, p.model, fast_s
                    )));
                }
            }

            // Agent behavior.
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Agent Behavior".into()));
            let a = &loaded.agent;
            app.push(ChatLine::Info(format!(
                "  max-turns           {}",
                a.max_turns
            )));
            if let Some(budget) = a.max_budget_usd {
                app.push(ChatLine::Info(format!(
                    "  max-budget-usd      ${:.2}",
                    budget
                )));
            } else {
                app.push(ChatLine::Info("  max-budget-usd      unlimited".into()));
            }
            if let Some(tools) = a.max_concurrent_tools {
                app.push(ChatLine::Info(format!("  max-concurrent-tools  {}", tools)));
            }
            if let Some(out_tok) = a.max_output_tokens {
                app.push(ChatLine::Info(format!("  max-output-tokens   {}", out_tok)));
            }
            app.push(ChatLine::Info(format!(
                "  auto-checkpoint     {}",
                if a.auto_checkpoint { "on" } else { "off" }
            )));
            app.push(ChatLine::Info(format!(
                "  quiet               {}",
                if a.quiet { "on" } else { "off" }
            )));
            app.push(ChatLine::Info(format!(
                "  notify              {}",
                if a.notify { "on" } else { "off" }
            )));
            if a.no_rules {
                app.push(ChatLine::Info(
                    "  no-rules            on (CLIDO.md ignored)".into(),
                ));
            }

            // Context.
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Context".into()));
            let c = &loaded.context;
            app.push(ChatLine::Info(format!(
                "  compaction-threshold  {:.0}%  (compress when context is this full)",
                c.compaction_threshold * 100.0
            )));
            if let Some(max_ctx) = c.max_context_tokens {
                app.push(ChatLine::Info(format!("  max-context-tokens  {}", max_ctx)));
            }
            // Show live session token usage if available.
            if app.stats.session_input_tokens > 0 {
                let used = app.stats.session_input_tokens;
                let limit = if app.context_max_tokens > 0 {
                    app.context_max_tokens
                } else {
                    0
                };
                let usage_str = if limit > 0 {
                    let pct = (used as f64 / limit as f64 * 100.0).min(100.0);
                    format!(
                        "  context now           {} / {} tokens  ({:.0}% used)",
                        used, limit, pct
                    )
                } else {
                    format!("  context now           {} tokens used this turn", used)
                };
                app.push(ChatLine::Info(usage_str));
            }

            // Prompt Enhancement.
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Prompt Enhancement".into()));
            app.push(ChatLine::Info(
                "  Use /enhance <prompt> to structure prompts via AI".into(),
            ));

            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Info(format!(
                "  Config file: {}",
                config_file_label
            )));
            app.push(ChatLine::Info(
                "  Use /configure <intent> to change settings in natural language".into(),
            ));
            app.push(ChatLine::Info("".into()));
        }
    }
}

pub(super) fn cmd_configure(app: &mut App, cmd: &str) {
    let intent = cmd.trim_start_matches("/configure").trim();
    if intent.is_empty() {
        app.push(ChatLine::Info("  Usage: /configure <intent>".into()));
        app.push(ChatLine::Info(
            "  Examples:  /configure optimize for speed".into(),
        ));
        app.push(ChatLine::Info(
            "             /configure use gpt-4o as default".into(),
        ));
        app.push(ChatLine::Info(
            "             /configure set max turns to 50".into(),
        ));
        app.push(ChatLine::Info(
            "             /configure add a fast role with claude-haiku".into(),
        ));
    } else {
        let global_path = clido_core::global_config_path();
        let project_path = app.workspace_root.join(".clido/config.toml");
        let (config_path, config_path_label) = if project_path.exists() {
            (project_path.clone(), project_path.display().to_string())
        } else {
            let gp = global_path
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("~/.config/clido/config.toml"));
            let label = gp.display().to_string();
            (gp, label)
        };
        let intent = intent.to_string();
        let prompt = format!(
            "The user wants to change their Clido configuration.\n\
            \n\
            Config file path: {config_path_label}\n\
            \n\
            User intent: \"{intent}\"\n\
            \n\
            Steps:\n\
            1. Read the current config file at `{config_path_label}` to understand the \
               exact format and current values.\n\
            2. Determine the minimum set of changes needed to fulfil the intent.\n\
            3. Before changing anything, summarise: what you will change and why.\n\
            4. Apply the changes using the Edit or Write tool. Make surgical changes only — \
               do NOT rewrite the entire file unless it does not exist yet.\n\
            5. Confirm what was changed with a brief summary.\n\
            \n\
            Config file format reference (TOML):\n\
            ```toml\n\
            default-profile = \"default\"\n\
            \n\
            [profile.default]\n\
            provider = \"anthropic\"  # anthropic | openai | openrouter | mistral | local | alibabacloud\n\
            model    = \"claude-sonnet-4-6\"\n\
            api_key  = \"sk-...\"      # optional; prefer api_key_env for safety\n\
            api_key_env = \"ANTHROPIC_API_KEY\"  # env var name\n\
            base_url = \"\"            # optional custom endpoint\n\
            \n\
            # Optional fast/cheap provider for utility tasks (titles, summaries, etc.):\n\
            # [profile.default.fast]\n\
            # provider = \"openai\"\n\
            # model    = \"gpt-4o-mini\"\n\
            \n\
            [agent]\n\
            max-turns            = 200     # maximum tool-use turns per run\n\
            max-budget-usd       = 5.0     # cost cap per run (omit for unlimited)\n\
            max-concurrent-tools = 4       # parallel read-only tool batches (cap)\n\
            max-output-tokens    = 8192    # max tokens per LLM response\n\
            quiet                = false   # suppress spinner / cost footer\n\
            notify               = false   # desktop notification on completion\n\
            auto-checkpoint      = true    # checkpoint before file-mutating turns\n\
            no-rules             = false   # ignore CLIDO.md rules files\n\
            \n\
            [context]\n\
            compaction-threshold = 0.58    # compress context when ~58% full (default)\n\
            max-context-tokens   = 100000  # optional hard cap on context size\n\
            \n\
            [index]\n\
            exclude-patterns = [\"*.lock\", \"vendor/**\"]\n\
            include-ignored  = false\n\
            ```\n\
            \n\
            Valid providers: anthropic, openai, openrouter, mistral, local, alibabacloud\n\
            \n\
            If the config file does not exist at `{config_path_label}`, create it with \
            sensible defaults plus the requested changes.\n\
            After writing, ask the user to restart clido or type /init to reload the config.",
            config_path_label = config_path_label,
            intent = intent,
        );
        let _ = config_path; // suppress unused warning
        app.send_now(prompt);
    }
}

pub(super) fn cmd_init(app: &mut App) {
    // Open the active profile in the in-TUI editor instead of exiting.
    let config_path = clido_core::global_config_path()
        .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => {
            app.push(ChatLine::Info(format!("  ✗ Could not load config: {e}")));
        }
        Ok(loaded) => {
            let name = app.current_profile.clone();
            let all_profiles = loaded.profiles.clone();
            match loaded.profiles.get(&name).cloned() {
                Some(entry) => {
                    app.profile_overlay = Some(ProfileOverlayState::for_edit(
                        name,
                        &entry,
                        config_path,
                        &all_profiles,
                    ));
                }
                None => {
                    // No matching profile — open a create flow for the default profile
                    app.profile_overlay =
                        Some(ProfileOverlayState::for_create(config_path, &all_profiles));
                }
            }
        }
    }
}

/// `/enhance <prompt>` — send the prompt to the utility provider for enhancement,
/// then submit the enhanced version to the main agent.
pub(super) fn cmd_enhance(app: &mut App, cmd: &str) {
    let raw = cmd.trim_start_matches("/enhance").trim();
    if raw.is_empty() {
        app.push(ChatLine::Info("".into()));
        app.push(ChatLine::Section("Prompt Enhancement".into()));
        app.push(ChatLine::Info("  Usage: /enhance <your prompt>".into()));
        app.push(ChatLine::Info("".into()));
        app.push(ChatLine::Info(
            "  Sends your prompt to the utility provider for intelligent".into(),
        ));
        app.push(ChatLine::Info(
            "  structuring. The enhanced prompt appears in the input field".into(),
        ));
        app.push(ChatLine::Info(
            "  so you can review and edit it before pressing Enter to send.".into(),
        ));
        app.push(ChatLine::Info("".into()));
        return;
    }
    // Queue: show "enhancing…" then send via channel so event_loop can await the async call.
    app.push(ChatLine::Info("  ✦ Enhancing prompt…".into()));
    app.enhancing = true;
    // Store the raw prompt — event_loop will pick it up and call the LLM.
    app.pending_enhance = Some(raw.to_string());
}

// ── Workflow orchestrator ─────────────────────────────────────────────────────
//
// Sequential steps are driven through the MAIN agent session: each step's
// rendered prompt is sent via `send_silent`, the agent's Response is intercepted
// in event_loop and `handle_workflow_step_response` is called to store the
// output and advance to the next step. This gives full tool-call visibility,
// normal permission prompts, and a resumable session.
//
// Parallel steps (parallel: true) cannot share a single agent, so they are
// run as isolated mini-agents in a background task. Results arrive via
// `AgentEvent::WorkflowParallelBatchDone` and are injected into context before
// the next sequential step starts.

/// Run a single isolated agent step (used for parallel batches only).
#[allow(clippy::too_many_arguments)]
async fn run_isolated_step(
    step_id: String,
    rendered_prompt: String,
    profile_name: String,
    workspace_root: std::path::PathBuf,
    run_id: String,
    tools: Option<Vec<String>>,
    system_prompt: Option<String>,
    max_turns: Option<u32>,
    outputs: Vec<clido_workflows::OutputDef>,
    context_snapshot: clido_workflows::WorkflowContext,
) -> (String, String, f64, u64) {
    use clido_agent::AgentLoop;
    use clido_core::{agent_config_from_loaded, load_config, load_pricing, PermissionMode};
    use clido_storage::SessionWriter;
    use clido_tools::default_registry_with_options;
    use std::io::Write as _;

    let do_run = async {
        let loaded = load_config(&workspace_root)?;
        let (pricing_table, _) = load_pricing();
        let profile = loaded.get_profile(&profile_name)?;
        clido_core::LoadedConfig::validate_provider(&profile.provider)?;
        let provider = crate::provider::make_provider(&profile_name, profile, None, None)
            .map_err(clido_core::ClidoError::Workflow)?;
        let model = loaded.get_profile(&profile_name)?.model.clone();
        let blocked = clido_core::global_config_path()
            .into_iter()
            .collect::<Vec<_>>();
        let mut registry = default_registry_with_options(workspace_root.clone(), blocked, false);
        registry = crate::agent_setup::load_mcp_tools_from_path(None, true, registry);
        let tools_explicitly_empty = tools.as_ref().is_some_and(|t| t.is_empty());
        registry = registry.with_filters(tools, None);
        if registry.schemas().is_empty() && !tools_explicitly_empty {
            return Err(clido_core::ClidoError::Workflow(
                "No tools available for step".into(),
            ));
        }
        let sp = system_prompt.unwrap_or_else(|| "You are a helpful coding assistant.".into());
        let mut config = agent_config_from_loaded(
            &loaded,
            &profile_name,
            max_turns,
            None,
            Some(model),
            Some(sp),
            Some(PermissionMode::AcceptAll),
            false,
            None,
        )?;
        if config.max_context_tokens.is_none() {
            if let Some(entry) = pricing_table.models.get(&config.model) {
                if let Some(cw) = entry.context_window {
                    config.max_context_tokens = Some(cw);
                }
            }
        }
        let session_id = format!("{run_id}_{step_id}");
        let mut writer = SessionWriter::create(&workspace_root, &session_id)?;
        let mut loop_ = crate::agent_setup::with_optional_trace_metrics(AgentLoop::new(
            provider, registry, config, None,
        ));
        let start = std::time::Instant::now();
        let text = loop_
            .run(
                &rendered_prompt,
                Some(&mut writer),
                Some(&pricing_table),
                None,
            )
            .await?;
        let duration_ms = start.elapsed().as_millis() as u64;
        let _ = writer.flush();
        // Apply save_to for this step.
        for out in &outputs {
            if let Some(ref tmpl) = out.save_to {
                if let Ok(path_str) =
                    clido_workflows::render_save_to(tmpl, &context_snapshot, &step_id)
                {
                    let p = std::path::Path::new(&path_str);
                    if let Some(parent) = p.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(p, &text);
                }
            }
        }
        Ok::<_, clido_core::ClidoError>((text, loop_.cumulative_cost_usd, duration_ms))
    };

    match do_run.await {
        Ok((text, cost, dur)) => (step_id, text, cost, dur),
        Err(e) => (step_id, format!("[error: {e}]"), 0.0, 0),
    }
}

/// Load a workflow, validate it, resolve inputs, check prerequisites, and start
/// orchestrating it through the main agent session.
fn start_workflow(
    app: &mut App,
    path: std::path::PathBuf,
    inputs: Vec<(String, String)>,
    profile_override: Option<String>,
) {
    use clido_workflows::{load as load_workflow, validate as validate_workflow, WorkflowContext};

    let def = match load_workflow(&path) {
        Ok(d) => d,
        Err(e) => {
            app.push(ChatLine::Info(format!("  ✗ {e}")));
            return;
        }
    };
    if let Err(e) = validate_workflow(&def) {
        app.push(ChatLine::Info(format!("  ✗ Invalid workflow: {e}")));
        return;
    }
    if let Err(e) = clido_workflows::check_prerequisites(&def) {
        app.push(ChatLine::Info(format!(
            "  ✗ Prerequisite check failed: {e}"
        )));
        return;
    }

    let overrides: Vec<(String, serde_json::Value)> = inputs
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    let resolved = match WorkflowContext::resolve_inputs(&def, &overrides) {
        Ok(r) => r,
        Err(e) => {
            app.push(ChatLine::Info(format!("  ✗ Input error: {e}")));
            return;
        }
    };

    let panel_steps = def
        .steps
        .iter()
        .map(|s| crate::tui::app_state::WorkflowStepEntry {
            step_id: s.id.clone(),
            name: s.name.clone().unwrap_or_else(|| s.id.clone()),
            status: crate::tui::app_state::WorkflowStepStatus::Pending,
        })
        .collect();

    let run_id = uuid::Uuid::new_v4().to_string();
    let sequential_count = def.steps.iter().filter(|s| !s.parallel).count();
    let step_count = def.steps.len();

    app.active_workflow = Some(crate::tui::app_state::ActiveWorkflow {
        name: def.name.clone(),
        steps: panel_steps,
        def,
        context: WorkflowContext::new(resolved),
        current_idx: 0,
        run_id,
        // Track against session_total_cost_usd so parallel step costs are included.
        start_cost: app.stats.session_total_cost_usd,
        start_time: std::time::Instant::now(),
        step_start_time: None,
        step_prev_model: None,
        parallel_abort: None,
        retry_attempts: 0,
        profile_override,
    });
    app.plan_panel_visibility = super::state::PlanPanelVisibility::On;

    app.push(ChatLine::Info(format!(
        "  ▶ Workflow '{}' starting ({step_count} steps)…",
        app.active_workflow.as_ref().unwrap().name,
    )));
    // Warn about context growth for long sequential workflows.
    if sequential_count > 5 {
        app.push(ChatLine::Info(format!(
            "  ⚠ {} sequential steps will share session context — step outputs are saved to disk",
            sequential_count
        )));
    }

    advance_workflow(app);
}

/// Advance the workflow orchestrator to the next step (or finish).
/// Called after a sequential step completes, after a parallel batch finishes,
/// and at the very start. Sets `app.busy` appropriately.
pub(super) fn advance_workflow(app: &mut App) {
    // Phase 1: snapshot what we need to decide what to do next.
    let (current_idx, total) = match app.active_workflow.as_ref() {
        Some(wf) => (wf.current_idx, wf.def.steps.len()),
        None => return,
    };

    if current_idx >= total {
        // All steps done — use session_total_cost_usd for accurate delta including parallel steps.
        let (name, cost_str, elapsed_ms) = {
            let wf = app.active_workflow.as_ref().unwrap();
            let is_subscription = clido_providers::is_subscription_provider(&app.provider);
            let cost = app.stats.session_total_cost_usd - wf.start_cost;
            let cost_str = if is_subscription {
                String::new()
            } else {
                format!(", ${:.4}", cost)
            };
            let ms = wf.start_time.elapsed().as_millis() as u64;
            (wf.name.clone(), cost_str, ms)
        };
        app.push(ChatLine::Info(format!(
            "  ✓ Workflow '{name}' complete — {total} steps{cost_str}, {elapsed_ms}ms"
        )));
        app.push(ChatLine::Info("  Type anything to continue.".into()));
        app.on_agent_done();
        app.active_workflow = None;
        return;
    }

    // Phase 2: check if next steps are parallel.
    let parallel_batch: Vec<clido_workflows::StepDef> = {
        let wf = app.active_workflow.as_ref().unwrap();
        let step = &wf.def.steps[current_idx];
        if step.parallel {
            wf.def.steps[current_idx..]
                .iter()
                .take_while(|s| s.parallel)
                .cloned()
                .collect()
        } else {
            vec![]
        }
    };

    if !parallel_batch.is_empty() {
        let batch_len = parallel_batch.len();

        // Mark all parallel steps as Active.
        {
            let wf = app.active_workflow.as_mut().unwrap();
            for step in &parallel_batch {
                if let Some(e) = wf.steps.iter_mut().find(|e| e.step_id == step.id) {
                    e.status = crate::tui::app_state::WorkflowStepStatus::Active;
                }
            }
            wf.current_idx += batch_len;
        }

        // Render prompts and collect everything needed for the task.
        let (tasks, context_snapshot, workspace_root, run_id, fetch_tx) = {
            let wf = app.active_workflow.as_ref().unwrap();
            let fallback_profile = wf
                .profile_override
                .clone()
                .unwrap_or_else(|| app.current_profile.clone());
            let mut tasks = Vec::new();
            for step in &parallel_batch {
                let rendered = clido_workflows::render(&step.prompt, &wf.context)
                    .unwrap_or_else(|_| step.prompt.clone());
                let profile = step
                    .profile
                    .clone()
                    .unwrap_or_else(|| fallback_profile.clone());
                tasks.push((
                    step.id.clone(),
                    rendered,
                    profile,
                    step.tools.clone(),
                    step.system_prompt.clone(),
                    step.max_turns,
                    step.outputs.clone(),
                ));
            }
            (
                tasks,
                wf.context.clone(),
                app.workspace_root.clone(),
                wf.run_id.clone(),
                app.channels.fetch_tx.clone(),
            )
        };

        let step_names: Vec<String> = parallel_batch
            .iter()
            .map(|s| s.name.clone().unwrap_or_else(|| s.id.clone()))
            .collect();
        app.push(ChatLine::Info(format!(
            "  ⇉ Running {} parallel steps: {}",
            batch_len,
            step_names.join(", ")
        )));

        let handle = tokio::spawn(async move {
            let futs: Vec<_> = tasks
                .into_iter()
                .map(|(sid, prompt, prof, tools, sp, mt, outs)| {
                    run_isolated_step(
                        sid,
                        prompt,
                        prof,
                        workspace_root.clone(),
                        run_id.clone(),
                        tools,
                        sp,
                        mt,
                        outs,
                        context_snapshot.clone(),
                    )
                })
                .collect();
            let outputs = futures::future::join_all(futs).await;
            let _ = fetch_tx
                .send(AgentEvent::WorkflowParallelBatchDone { outputs })
                .await;
        });

        app.active_workflow.as_mut().unwrap().parallel_abort = Some(handle.abort_handle());
        // Keep app.busy = true; the parallel batch will send WorkflowParallelBatchDone when done.
        return;
    }

    // Phase 3: sequential step — send its prompt to the main agent.
    let (step_id, step_name, rendered, effective_profile, tools_hint, step_system_prompt) = {
        let wf = app.active_workflow.as_ref().unwrap();
        let step = &wf.def.steps[current_idx];
        let name = step.name.clone().unwrap_or_else(|| step.id.clone());
        let rendered = match clido_workflows::render(&step.prompt, &wf.context) {
            Ok(p) => p,
            Err(e) => {
                app.push(ChatLine::Info(format!(
                    "  ✗ Failed to render step '{}': {e}",
                    step.id
                )));
                app.on_agent_done();
                app.active_workflow = None;
                return;
            }
        };
        // Tool restriction hint (best effort — main agent registry can't be changed per-step).
        let tools_hint = step.tools.as_ref().map(|t| {
            if t.is_empty() {
                "IMPORTANT: Do not use any tools for this step. Write your response directly."
                    .to_string()
            } else {
                format!(
                    "IMPORTANT: For this step, only use these tools if needed: {}.",
                    t.join(", ")
                )
            }
        });
        // Effective profile: step-level > workflow --profile= override > session default.
        let effective_profile = step.profile.clone().or_else(|| wf.profile_override.clone());
        (
            step.id.clone(),
            name,
            rendered,
            effective_profile,
            tools_hint,
            step.system_prompt.clone(),
        )
    };

    // Update step status to Active.
    {
        let wf = app.active_workflow.as_mut().unwrap();
        if let Some(e) = wf.steps.iter_mut().find(|e| e.step_id == step_id) {
            e.status = crate::tui::app_state::WorkflowStepStatus::Active;
        }
        wf.step_start_time = Some(std::time::Instant::now());
    }

    // Show a step header in chat.
    app.push(ChatLine::Info(format!(
        "  [{}/{}] ▶ {step_name}",
        current_idx + 1,
        total
    )));

    // Switch model if effective profile specifies a different model.
    if let Some(ref profile_name) = effective_profile {
        let workspace = app.workspace_root.clone();
        if let Ok(loaded) = clido_core::load_config(&workspace) {
            if let Ok(profile) = loaded.get_profile(profile_name) {
                if profile.model != app.model {
                    let prev = app.model.clone();
                    app.model = profile.model.clone();
                    let _ = app.channels.model_switch_tx.send(profile.model.clone());
                    app.active_workflow.as_mut().unwrap().step_prev_model = Some(prev);
                    app.push(ChatLine::Info(format!(
                        "  ↻ Using {} ({}) for this step",
                        profile.model, profile_name
                    )));
                }
            }
        }
    }

    // Build final prompt: prepend system_prompt persona + tool hint ahead of the rendered prompt.
    // This is a best-effort substitution since the main agent's system prompt is session-level.
    let mut prefix_parts: Vec<String> = Vec::new();
    if let Some(sp) = step_system_prompt {
        let sp = sp.trim();
        if !sp.is_empty() {
            prefix_parts.push(format!("[Step persona: {sp}]"));
        }
    }
    if let Some(hint) = tools_hint {
        prefix_parts.push(hint);
    }
    let final_prompt = if prefix_parts.is_empty() {
        rendered
    } else {
        format!("{}\n\n{rendered}", prefix_parts.join("\n"))
    };

    app.send_silent(final_prompt);
}

/// Called from the `AgentEvent::Response` handler when a workflow is active.
/// Stores the step output, applies `save_to`, reverts model override, then advances.
pub(super) fn handle_workflow_step_response(app: &mut App, text: String) {
    use clido_workflows::OnErrorPolicy;

    // Extract everything we need before borrowing mutably.
    let (step_id, step_num, total, step_name, outputs, on_error, prev_model, dur_ms) = {
        let Some(ref mut wf) = app.active_workflow else {
            return;
        };
        let step = &wf.def.steps[wf.current_idx];
        let step_id = step.id.clone();
        let step_num = wf.current_idx + 1;
        let total = wf.def.steps.len();
        let step_name = step.name.clone().unwrap_or_else(|| step.id.clone());
        let outputs = step.outputs.clone();
        let on_error = step.on_error;

        // Store output in context (all output aliases + canonical "output").
        wf.context.set_step_output(&step_id, "output", text.clone());
        for out in &outputs {
            if out.name != "output" {
                wf.context
                    .set_step_output(&step_id, &out.name, text.clone());
            }
        }

        // Mark done in display list.
        if let Some(e) = wf.steps.iter_mut().find(|e| e.step_id == step_id) {
            e.status = crate::tui::app_state::WorkflowStepStatus::Done;
        }

        let dur_ms = wf
            .step_start_time
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        wf.step_start_time = None;

        let prev_model = wf.step_prev_model.take();
        wf.retry_attempts = 0;
        wf.current_idx += 1;
        (
            step_id, step_num, total, step_name, outputs, on_error, prev_model, dur_ms,
        )
    };

    // Show step timing.
    app.push(ChatLine::Info(format!(
        "  ✓ [{step_num}/{total}] {step_name} ({dur_ms}ms)"
    )));

    // Apply save_to with aggressive retry logic.
    // Files are CRITICAL for workflow continuation - we must ensure they are written.
    let (save_results, empty_output_warning) = {
        let wf = app.active_workflow.as_ref().unwrap();
        let mut results = Vec::new();
        let mut empty_warning = None;

        for out in &outputs {
            if let Some(ref tmpl) = out.save_to {
                // Try to render the template first
                let path_str = match clido_workflows::render_save_to(tmpl, &wf.context, &step_id) {
                    Ok(ps) => ps,
                    Err(e) => {
                        let msg = format!("save_to template error for '{step_id}': {e}");
                        results.push((tmpl.clone(), Err(msg)));
                        continue;
                    }
                };

                // Check if output text is empty - this is a problem
                if text.trim().is_empty() {
                    empty_warning = Some(format!(
                        "save_to: output text is empty for '{step_id}' - nothing to write to '{}'",
                        path_str
                    ));
                    results.push((path_str.clone(), Err("empty output".to_string())));
                    continue;
                }

                results.push((path_str, Ok(())));
            }
        }
        (results, empty_warning)
    };

    // Now process the saves with retry logic (app is not borrowed here)
    let mut failed_saves = Vec::new();
    let mut successful_count = 0;

    for (path_str, _) in save_results {
        let p = std::path::Path::new(&path_str);

        // Aggressive retry with exponential backoff
        let mut last_error = None;
        let mut succeeded = false;

        for attempt in 1..=5 {
            // Create parent directories
            if let Some(parent) = p.parent() {
                if !parent.exists() {
                    match std::fs::create_dir_all(parent) {
                        Ok(()) => {
                            app.push(ChatLine::Info(format!(
                                "  📁 Created directory: {}",
                                parent.display()
                            )));
                        }
                        Err(e) => {
                            last_error = Some(format!("create_dir_all failed: {}", e));
                            std::thread::sleep(std::time::Duration::from_millis(100 * attempt));
                            continue; // Retry
                        }
                    }
                }
            }

            // Try to write the file
            match std::fs::write(p, &text) {
                Ok(()) => {
                    app.push(ChatLine::Info(format!(
                        "  💾 Saved output to: {}",
                        path_str
                    )));
                    succeeded = true;
                    break; // Success!
                }
                Err(e) => {
                    last_error = Some(format!("write failed: {}", e));
                    if attempt < 5 {
                        app.push(ChatLine::Info(format!(
                            "  ⚠ Write attempt {}/5 failed for '{}': {}. Retrying...",
                            attempt, path_str, e
                        )));
                        std::thread::sleep(std::time::Duration::from_millis(
                            100 * attempt * attempt,
                        ));
                    }
                }
            }
        }

        if succeeded {
            successful_count += 1;
        } else if let Some(err) = last_error {
            failed_saves.push(format!(
                "save_to: failed to write '{}' after 5 attempts: {}",
                path_str, err
            ));
        }
    }

    // Show empty output warning if present
    if let Some(ref warning) = empty_output_warning {
        app.push(ChatLine::Info(format!("  ⚠ {warning}")));
    }

    let total_saves = successful_count + failed_saves.len();

    if successful_count > 0 {
        app.push(ChatLine::Info(format!(
            "  ✓ Successfully saved {}/{} output files",
            successful_count, total_saves
        )));
    }

    if !failed_saves.is_empty() {
        for msg in &failed_saves {
            app.push(ChatLine::Info(format!("  ✗ {msg}")));
        }

        // CRITICAL: If save_to fails, we should abort the workflow regardless of on_error policy
        // because subsequent steps depend on these files
        app.push(ChatLine::Info(
            "  🚨 CRITICAL: Output files could not be saved. Workflow may fail in subsequent steps.".into()
        ));

        // Store save failures in workflow context so next step can see them
        {
            let wf = app.active_workflow.as_mut().unwrap();
            wf.context
                .set_step_output(&step_id, "_save_failed", failed_saves.join("; "));
        }

        // Add system message to inform the agent about the save failure
        let failure_details = failed_saves.join("\n");
        app.push(ChatLine::Info(format!(
            "  📋 [System] The following output files could not be saved:\n{}",
            failure_details
        )));

        if on_error == OnErrorPolicy::Fail {
            handle_workflow_step_error(app, failed_saves.join("; "));
            return;
        }
    } else {
        // Store success status
        let wf = app.active_workflow.as_mut().unwrap();
        wf.context
            .set_step_output(&step_id, "_save_status", "success".to_string());
    }

    // Also track empty output warning if present
    if let Some(warning) = empty_output_warning {
        let wf = app.active_workflow.as_mut().unwrap();
        wf.context
            .set_step_output(&step_id, "_save_warning", warning);
    }

    // Restore model.
    if let Some(prev) = prev_model {
        app.model = prev.clone();
        let _ = app.channels.model_switch_tx.send(prev);
    }

    advance_workflow(app);
}

/// Called when an agent error or hard failure occurs during a sequential workflow step.
/// Applies the step's `on_error` policy (Fail / Continue / Retry).
pub(super) fn handle_workflow_step_error(app: &mut App, error: String) {
    use clido_workflows::OnErrorPolicy;

    let (step_id, step_name, on_error, retry_limit, current_attempts) = {
        let Some(ref wf) = app.active_workflow else {
            app.on_agent_done();
            return;
        };
        let step = &wf.def.steps[wf.current_idx];
        let retry_limit = step.retry.as_ref().map(|r| r.max_attempts).unwrap_or(3) as usize;
        (
            step.id.clone(),
            step.name.clone().unwrap_or_else(|| step.id.clone()),
            step.on_error,
            retry_limit,
            wf.retry_attempts,
        )
    };

    // Revert any per-step model override before deciding what to do.
    let prev_model = app
        .active_workflow
        .as_mut()
        .and_then(|wf| wf.step_prev_model.take());
    if let Some(prev) = prev_model {
        app.model = prev.clone();
        let _ = app.channels.model_switch_tx.send(prev);
    }

    match on_error {
        OnErrorPolicy::Retry if current_attempts < retry_limit => {
            let attempt = current_attempts + 1;
            app.active_workflow.as_mut().unwrap().retry_attempts = attempt;
            app.push(ChatLine::Info(format!(
                "  ↻ Step '{step_name}' failed (attempt {attempt}/{retry_limit}): {error}"
            )));
            app.push(ChatLine::Info(format!("  ↻ Retrying '{step_name}'…")));
            // Re-send the same step prompt (current_idx unchanged).
            advance_workflow(app);
        }
        OnErrorPolicy::Continue => {
            app.push(ChatLine::Info(format!(
                "  ⚠ Step '{step_name}' failed (continuing): {error}"
            )));
            if let Some(ref mut wf) = app.active_workflow {
                if let Some(e) = wf.steps.iter_mut().find(|e| e.step_id == step_id) {
                    e.status = crate::tui::app_state::WorkflowStepStatus::Skipped;
                }
                // Store empty output so downstream templates don't break.
                wf.context
                    .set_step_output(&step_id, "output", String::new());
                wf.current_idx += 1;
                wf.retry_attempts = 0;
                wf.step_start_time = None;
            }
            advance_workflow(app);
        }
        _ => {
            // Fail (default) or Retry exhausted.
            let label = if on_error == OnErrorPolicy::Retry {
                format!("  ✗ Step '{step_name}' failed after {current_attempts} retries: {error}")
            } else {
                format!("  ✗ Step '{step_name}' failed: {error}")
            };
            app.push(ChatLine::Info(label));
            if let Some(ref mut wf) = app.active_workflow {
                if let Some(e) = wf.steps.iter_mut().find(|e| e.step_id == step_id) {
                    e.status = crate::tui::app_state::WorkflowStepStatus::Failed;
                }
            }
            app.push(ChatLine::Info(
                "  Workflow stopped — type a message to debug or /workflow run to restart.".into(),
            ));
            app.on_agent_done();
            app.active_workflow = None;
        }
    }
}

/// Abort the active workflow (parallel batch + main agent), called on Ctrl-C.
pub(super) fn abort_workflow(app: &mut App) {
    // Extract model to revert and abort any parallel batch.
    let prev_model = app.active_workflow.as_mut().and_then(|wf| {
        if let Some(handle) = wf.parallel_abort.take() {
            handle.abort();
        }
        wf.step_prev_model.take()
    });

    app.active_workflow = None;
    app.push(ChatLine::Info("  ✗ Workflow cancelled.".into()));

    // Revert model if a step had switched it.
    if let Some(prev) = prev_model {
        app.model = prev.clone();
        let _ = app.channels.model_switch_tx.send(prev);
    }

    app.on_agent_done();
}

/// Resolve workflow directories: platform-specific global dir and project-local `.clido/workflows/`.
/// Returns (global_dirs, local_dir, info_strings) tuple.
/// On macOS, checks both ~/Library/Application Support/clido/workflows/ (standard)
/// and ~/.config/clido/workflows/ (legacy/compatibility).
fn workflow_dirs_with_info(
    workspace_root: &std::path::Path,
) -> (
    Vec<std::path::PathBuf>,
    Option<std::path::PathBuf>,
    Vec<String>,
) {
    let mut global_dirs = Vec::new();
    let mut info = Vec::new();

    // Platform-specific global config directory (standard location)
    if let Some(global) = clido_core::global_config_dir() {
        let workflows_dir = global.join("workflows");
        global_dirs.push(workflows_dir.clone());

        // Create user-friendly path description
        let home = std::env::var("HOME").unwrap_or_default();
        let display_path = if workflows_dir.to_string_lossy().starts_with(&home) {
            format!(
                "~/{}",
                workflows_dir
                    .strip_prefix(&home)
                    .unwrap_or(&workflows_dir)
                    .display()
            )
        } else {
            workflows_dir.display().to_string()
        };
        info.push(format!("global: {}", display_path));

        // On macOS, also check legacy ~/.config/clido/workflows/ for compatibility
        #[cfg(target_os = "macos")]
        {
            let legacy_dir = std::path::PathBuf::from(&home).join(".config/clido/workflows");
            if legacy_dir != workflows_dir && legacy_dir.is_dir() {
                global_dirs.push(legacy_dir.clone());
                info.push("global (legacy): ~/.config/clido/workflows/".to_string());
            }
        }
    }

    // Project-local workflows
    let local_dir = workspace_root.join(".clido").join("workflows");
    let local_display = ".clido/workflows/".to_string();
    info.push(format!("local: {}", local_display));

    (global_dirs, Some(local_dir), info)
}

/// Scan workflow directories and return `(name, path, description, step_count)` tuples.
fn list_workflows(
    workspace_root: &std::path::Path,
) -> Vec<(String, std::path::PathBuf, String, usize)> {
    let mut results = Vec::new();
    let (global_dirs, local_dir, _) = workflow_dirs_with_info(workspace_root);

    // Check all global directories
    for dir in global_dirs {
        if !dir.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if ext != "yaml" && ext != "yml" {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(def) = serde_yaml::from_str::<clido_workflows::WorkflowDef>(&content)
                    {
                        let name = def.name.clone();
                        let desc = if def.description.is_empty() {
                            "(no description)".to_string()
                        } else {
                            def.description.clone()
                        };
                        let steps = def.steps.len();
                        results.push((name, path, desc, steps));
                    }
                }
            }
        }
    }

    // Check local directory
    if let Some(dir) = local_dir {
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if ext != "yaml" && ext != "yml" {
                        continue;
                    }
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(def) =
                            serde_yaml::from_str::<clido_workflows::WorkflowDef>(&content)
                        {
                            let name = def.name.clone();
                            let desc = if def.description.is_empty() {
                                "(no description)".to_string()
                            } else {
                                def.description.clone()
                            };
                            let steps = def.steps.len();
                            results.push((name, path, desc, steps));
                        }
                    }
                }
            }
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

/// Find a workflow by name from the workflow directories.
fn find_workflow(workspace_root: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let (global_dirs, local_dir, _) = workflow_dirs_with_info(workspace_root);

    // Helper closure to check a directory
    let check_dir = |dir: &std::path::Path| -> Option<std::path::PathBuf> {
        if !dir.is_dir() {
            return None;
        }
        // Try exact filename first
        for ext in ["yaml", "yml"] {
            let path = dir.join(format!("{name}.{ext}"));
            if path.is_file() {
                return Some(path);
            }
        }
        // Try matching by workflow name field
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if ext != "yaml" && ext != "yml" {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(def) = serde_yaml::from_str::<clido_workflows::WorkflowDef>(&content)
                    {
                        if def.name == name {
                            return Some(path);
                        }
                    }
                }
            }
        }
        None
    };

    // Check all global directories first
    for dir in global_dirs {
        if let Some(path) = check_dir(&dir) {
            return Some(path);
        }
    }

    // Check local directory
    if let Some(dir) = local_dir {
        if let Some(path) = check_dir(&dir) {
            return Some(path);
        }
    }

    None
}

/// Extract the last YAML code block from assistant messages in the chat.
pub(super) fn extract_last_yaml_from_chat(messages: &[ChatLine]) -> Option<String> {
    for msg in messages.iter().rev() {
        let text = match msg {
            ChatLine::Assistant(t) => t,
            _ => continue,
        };
        // Find ```yaml ... ``` blocks (search from the end)
        let mut last_yaml = None;
        let mut in_yaml = false;
        let mut yaml_lines: Vec<&str> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if !in_yaml && (trimmed.starts_with("```yaml") || trimmed.starts_with("```yml")) {
                in_yaml = true;
                yaml_lines.clear();
                continue;
            }
            if in_yaml && trimmed == "```" {
                in_yaml = false;
                last_yaml = Some(yaml_lines.join("\n"));
                continue;
            }
            if in_yaml {
                yaml_lines.push(line);
            }
        }
        if let Some(yaml) = last_yaml {
            return Some(yaml);
        }
    }
    None
}

/// Build the system prompt addition for guided workflow creation.
fn workflow_creation_system_prompt() -> String {
    r#"You are helping the user create a clido YAML workflow. Guide them through the process step by step.

## Full Workflow YAML Schema

```yaml
name: workflow-name          # Required: kebab-case identifier used in /workflow run <name>
version: "1"                 # Required: always "1"
description: "What it does"  # Recommended

inputs:                      # Optional: parameters the user provides at runtime via key=value args
  - name: repo_path
    description: "Human-readable description of this input"
    required: false          # true = error if not provided and no default
    default: "{{ cwd }}"    # Defaults support {{ cwd }}, {{ date }}, {{ datetime }} template vars

steps:                       # Required: at least one step
  - id: step-id              # Required: unique kebab-case ID
    name: "Human name"       # Optional: shown during execution
    prompt: |                # Required: Tera template — use {{ inputs.name }}, {{ steps.prev_id.output }}
      Analyse {{ inputs.repo_path }}.
      Previous findings: {{ steps.explore.output }}
    tools:                   # Optional allowlist; omit = all tools; [] = no tools (pure reasoning)
      - Read
      - Glob
      - Grep
      - Bash
      - Edit
      - Write
      - Git
      - WebSearch
      - WebFetch
    profile: "opus"          # Optional: profile name from config (overrides workflow default)
    system_prompt: "..."     # Optional: replaces the default agent system prompt for this step
    max_turns: 20            # Optional: cap agent iterations (default: profile setting)
    parallel: true           # Optional: group consecutive parallel:true steps into a concurrent batch
    on_error: fail           # Optional: fail (default) | continue | retry
    retry:                   # Required when on_error: retry
      max_attempts: 3
      backoff: exponential   # none | exponential
    outputs:                 # Optional: named outputs; "output" is always set automatically
      - name: output         # "output" = full step text (default); custom names are aliases
        type: text
        save_to: "{{ inputs.repo_path }}/.clido-out/{{ step_id }}.txt"
        # save_to is a Tera template: {{ inputs.* }}, {{ steps.*.output }}, {{ step_id }}, {{ cwd }}

prerequisites:               # Optional: checked before any steps run
  env:
    - SOME_API_KEY           # String = required env var
    - name: OPT_VAR
      optional: true         # optional env var (warn but don't fail)
  commands:
    - forge                  # String = required PATH command
    - name: slither
      optional: true
```

## Template variables available in prompts and save_to paths

- `{{ inputs.<name> }}` — resolved input value
- `{{ steps.<step_id>.output }}` — full text output of a completed step
- `{{ steps.<step_id>.<output_name> }}` — named output of a completed step
- `{{ cwd }}` — current working directory at workflow start
- `{{ date }}` — today's date (YYYY-MM-DD)
- `{{ datetime }}` — current date+time (YYYY-MM-DDTHH:MM:SS)
- `{{ step_id }}` — current step id (only in `save_to` templates)

## Key design rules

1. **Each step is a full agent invocation** — it gets its own LLM call, tool sandbox, and session log.
2. **Use `save_to` for large outputs** so subsequent steps can read them with the Read tool instead of embedding them in prompts (avoids token bloat).
3. **`parallel: true`** groups consecutive parallel steps into a concurrent batch; non-parallel steps run sequentially.
4. **`profile:`** per step lets you run cheap steps on a fast model and expensive steps on a powerful one.
5. **`tools: []`** (empty list) is valid — it means the step does pure reasoning/writing with no tool access.
6. **`on_error: continue`** is useful for optional steps (e.g. PoC generation) that shouldn't abort the workflow.
7. **`default: "{{ cwd }}"`** on inputs enables zero-config auto-discovery — the user can run the workflow from inside the target repository without supplying any arguments.
8. **Workflow discovery**: save workflows to `~/.config/clido/workflows/<name>.yaml` (global). Use `/workflow save` from the TUI after the agent outputs the YAML block.

## Running workflows

From the TUI:
```
/workflow run <name>
/workflow run <name> key=value key2=value2
/workflow run <name> key=value --profile=opus
```

From the CLI:
```
clido workflow run <name>
clido workflow run <name> -i key=value -i key2=value2 --profile opus
```

## Guidelines for guiding the user

1. Ask what the workflow should accomplish.
2. Ask about inputs — what varies between runs? What has a sensible default?
3. Design steps — each is a full agent turn with its own prompt, tools, and profile.
4. Suggest `save_to` for steps with large outputs that feed later steps.
5. Consider which steps can run in parallel.
6. Propose per-step profiles if some steps need a more capable model.
7. When ready, output the complete YAML in a ```yaml code block.
8. After showing the YAML, remind the user to use `/workflow save` to save it, or `/workflow edit` to tweak it.

Keep the conversation natural. Ask one thing at a time."#
        .to_string()
}

pub(super) fn cmd_workflow(app: &mut App, cmd: &str) {
    let sub = cmd.trim_start_matches("/workflow").trim().to_string();

    match sub.as_str() {
        "" | "list" => {
            // List workflows with directory info
            let (_global_dirs, _local_dir, info) = workflow_dirs_with_info(&app.workspace_root);
            let workflows = list_workflows(&app.workspace_root);

            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Workflows".into()));

            // Show search paths
            app.push(ChatLine::Info("  Search paths:".into()));
            for path_info in &info {
                app.push(ChatLine::Info(format!("    • {}", path_info)));
            }
            app.push(ChatLine::Info("".into()));

            if workflows.is_empty() {
                app.push(ChatLine::Info(
                    "  No workflows found. Use /workflow new <description> to create one.".into(),
                ));
            } else {
                app.push(ChatLine::Info(format!(
                    "  Found {} workflow(s):",
                    workflows.len()
                )));
                app.push(ChatLine::Info("".into()));
                for (name, path, desc, steps) in &workflows {
                    let loc = if path.starts_with(&app.workspace_root) {
                        "local"
                    } else {
                        "global"
                    };
                    app.push(ChatLine::Info(format!("  {name}  ({steps} steps, {loc})")));
                    app.push(ChatLine::Info(format!("    {desc}")));
                }
                app.push(ChatLine::Info("".into()));
                app.push(ChatLine::Info(
                    "  Use /workflow show <name> to view, /workflow run <name> to execute".into(),
                ));
            }
            app.push(ChatLine::Info("".into()));
        }
        "help" => {
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Workflow Commands".into()));
            app.push(ChatLine::Info(
                "  /workflow              list all saved workflows".into(),
            ));
            app.push(ChatLine::Info(
                "  /workflow new <desc>   create a workflow with AI guidance".into(),
            ));
            app.push(ChatLine::Info(
                "  /workflow list         list all saved workflows".into(),
            ));
            app.push(ChatLine::Info(
                "  /workflow show <name>  display a workflow's YAML".into(),
            ));
            app.push(ChatLine::Info(
                "  /workflow edit [name]  open in text editor".into(),
            ));
            app.push(ChatLine::Info(
                "  /workflow agent-edit <name> <desc>  edit with AI assistance".into(),
            ));
            app.push(ChatLine::Info(
                "  /workflow save [name]  save last YAML from chat".into(),
            ));
            app.push(ChatLine::Info(
                "  /workflow run <name>   run a workflow (via CLI)".into(),
            ));
            app.push(ChatLine::Info("".into()));
        }
        _ if sub.starts_with("new") => {
            let desc = sub.trim_start_matches("new").trim();
            if desc.is_empty() {
                app.push(ChatLine::Info(
                    "  Usage: /workflow new <description of what the workflow should do>".into(),
                ));
                return;
            }
            // Send to the main agent with a workflow creation system prompt
            let system_addition = workflow_creation_system_prompt();
            let msg = format!(
                "[System context: {system_addition}]\n\n\
                 I want to create a workflow: {desc}"
            );
            app.push(ChatLine::Info(
                "  ✦ Starting guided workflow creation…".into(),
            ));
            app.send_now(msg);
        }
        _ if sub.starts_with("show ") => {
            let name = sub.trim_start_matches("show").trim();
            if name.is_empty() {
                app.push(ChatLine::Info("  Usage: /workflow show <name>".into()));
                return;
            }
            match find_workflow(&app.workspace_root, name) {
                Some(path) => match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        // Show workflow as a single code block
                        let mut full_text = format!("Workflow: {}\n```yaml\n", path.display());
                        full_text.push_str(&content);
                        full_text.push_str("\n```");
                        app.push(ChatLine::Info(full_text));
                    }
                    Err(e) => {
                        app.push(ChatLine::Info(format!("  ✗ Failed to read: {e}")));
                    }
                },
                None => {
                    app.push(ChatLine::Info(format!(
                        "  ✗ Workflow '{name}' not found. Use /workflow list to see available workflows."
                    )));
                }
            }
        }
        _ if sub.starts_with("agent-edit") => {
            // Agent-driven workflow editing
            let rest = sub.trim_start_matches("agent-edit").trim();
            if rest.is_empty() {
                app.push(ChatLine::Info(
                    "  Usage: /workflow agent-edit <name> <description of changes>".into(),
                ));
                return;
            }

            // Parse: first token = workflow name, rest = description
            let mut tokens = rest.splitn(2, ' ');
            let name = tokens.next().unwrap_or("").trim();
            let description = tokens.next().unwrap_or("").trim();

            if name.is_empty() {
                app.push(ChatLine::Info(
                    "  Usage: /workflow agent-edit <name> <description of changes>".into(),
                ));
                return;
            }

            if description.is_empty() {
                app.push(ChatLine::Info(
                    "  Usage: /workflow agent-edit <name> <description of changes>".into(),
                ));
                app.push(ChatLine::Info(
                    "  Example: /workflow agent-edit sa2 'Add a step to run tests before deployment'".into(),
                ));
                return;
            }

            // Find the workflow
            match find_workflow(&app.workspace_root, name) {
                Some(path) => match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        // Store the path for later saving (after agent responds)
                        app.pending_workflow_agent_edit = Some(path.clone());

                        // Send to agent for editing
                        let prompt = format!(
                            "TASK: Edit the following workflow YAML file.\n\n\
                            WORKFLOW NAME: {name}\n\n\
                            USER'S REQUEST: {description}\n\n\
                            CURRENT WORKFLOW YAML:\n```yaml\n{content}\n```\n\n\
                            IMPORTANT INSTRUCTIONS:\n\
                            1. Your ONLY task is to edit this specific workflow based on the user's request.\n\
                            2. Do NOT do anything else - do not modify other files, do not run tools, do not ask questions.\n\
                            3. Return ONLY the complete updated workflow YAML.\n\
                            4. Preserve the workflow structure and ensure the YAML is valid.\n\
                            5. Wrap the YAML in a ```yaml code block.\n\
                            6. Do not add explanations or commentary outside the code block.",
                            name = name,
                            description = description,
                            content = content
                        );

                        app.push(ChatLine::Info(format!(
                            "  ✦ Editing workflow '{name}' with AI…"
                        )));
                        app.send_now(prompt);
                    }
                    Err(e) => {
                        app.push(ChatLine::Info(format!("  ✗ Failed to read: {e}")));
                    }
                },
                None => {
                    app.push(ChatLine::Info(format!("  ✗ Workflow '{name}' not found.")));
                }
            }
        }
        _ if sub.starts_with("edit") => {
            let name = sub.trim_start_matches("edit").trim();
            if name.is_empty() {
                // Edit last YAML from chat
                match extract_last_yaml_from_chat(&app.messages) {
                    Some(yaml) => {
                        app.workflow_editor = Some(PlanTextEditor::from_raw(&yaml));
                        app.workflow_editor_path = None;
                    }
                    None => {
                        app.push(ChatLine::Info(
                            "  ✗ No workflow YAML found in chat. Use /workflow edit <name> to edit a saved workflow.".into(),
                        ));
                    }
                }
            } else {
                match find_workflow(&app.workspace_root, name) {
                    Some(path) => match std::fs::read_to_string(&path) {
                        Ok(content) => {
                            app.workflow_editor = Some(PlanTextEditor::from_raw(&content));
                            app.workflow_editor_path = Some(path);
                        }
                        Err(e) => {
                            app.push(ChatLine::Info(format!("  ✗ Failed to read: {e}")));
                        }
                    },
                    None => {
                        app.push(ChatLine::Info(format!("  ✗ Workflow '{name}' not found.")));
                    }
                }
            }
        }
        _ if sub.starts_with("save") => {
            let name_arg = sub.trim_start_matches("save").trim();
            match extract_last_yaml_from_chat(&app.messages) {
                Some(yaml) => match serde_yaml::from_str::<clido_workflows::WorkflowDef>(&yaml) {
                    Ok(def) => {
                        let save_dir = clido_core::default_workflows_directory();
                        let save_dir = std::path::PathBuf::from(&save_dir);
                        let _ = std::fs::create_dir_all(&save_dir);
                        let file_name = if !name_arg.is_empty() {
                            name_arg.to_string()
                        } else {
                            def.name
                                .chars()
                                .map(|c| {
                                    if c.is_alphanumeric() || c == '-' || c == '_' {
                                        c
                                    } else {
                                        '-'
                                    }
                                })
                                .collect::<String>()
                        };
                        let path = save_dir.join(format!("{file_name}.yaml"));
                        match std::fs::write(&path, &yaml) {
                            Ok(()) => {
                                app.push(ChatLine::Info(format!(
                                    "  ✓ Workflow saved: {}",
                                    path.display()
                                )));
                                app.push(ChatLine::Info(format!(
                                    "  Run with: clido workflow run {file_name}"
                                )));
                            }
                            Err(e) => {
                                app.push(ChatLine::Info(format!("  ✗ Failed to save: {e}")));
                            }
                        }
                    }
                    Err(e) => {
                        app.push(ChatLine::Info(format!("  ✗ Invalid workflow YAML: {e}")));
                        app.push(ChatLine::Info(
                            "  Use /workflow edit to fix it manually.".into(),
                        ));
                    }
                },
                None => {
                    app.push(ChatLine::Info(
                        "  ✗ No workflow YAML found in chat. Use /workflow new <desc> to create one first.".into(),
                    ));
                }
            }
        }
        _ if sub.starts_with("run ") => {
            let rest = sub.trim_start_matches("run").trim();
            if rest.is_empty() {
                app.push(ChatLine::Info(
                    "  Usage: /workflow run <name> [key=value …] [--profile=<name>]".into(),
                ));
                return;
            }
            // Parse: first token = workflow name, remaining = key=value pairs or --profile=
            let mut tokens = rest.split_whitespace();
            let name = tokens.next().unwrap_or("").trim();
            let mut inputs: Vec<(String, String)> = Vec::new();
            let mut profile_override: Option<String> = None;
            for token in tokens {
                if let Some(p) = token.strip_prefix("--profile=") {
                    profile_override = Some(p.to_string());
                } else if let Some((k, v)) = token.split_once('=') {
                    inputs.push((k.to_string(), v.to_string()));
                }
            }
            match find_workflow(&app.workspace_root, name) {
                Some(path) => {
                    start_workflow(app, path, inputs, profile_override);
                }
                None => {
                    app.push(ChatLine::Info(format!(
                        "  ✗ Workflow '{name}' not found. Use /workflow list to see available."
                    )));
                }
            }
        }
        _ => {
            // Unknown subcommand — treat as /workflow new <desc>
            let desc = sub.trim();
            if !desc.is_empty() {
                let system_addition = workflow_creation_system_prompt();
                let msg = format!(
                    "[System context: {system_addition}]\n\n\
                     I want to create a workflow: {desc}"
                );
                app.push(ChatLine::Info(
                    "  ✦ Starting guided workflow creation…".into(),
                ));
                app.send_now(msg);
            } else {
                app.push(ChatLine::Info(
                    "  Usage: /workflow [new|list|show|edit|save|run|help]".into(),
                ));
            }
        }
    }
}

pub(super) fn execute_slash(app: &mut App, cmd: &str) {
    match cmd {
        "/clear" => {
            // Generate a new session ID and create a new session file
            let new_session_id = uuid::Uuid::new_v4().to_string();
            let short_id = &new_session_id[..8];

            // Create the new session in storage
            match clido_storage::SessionWriter::create(&app.workspace_root, &new_session_id) {
                Ok(_writer) => {
                    app.messages.clear();
                    app.messages.push(ChatLine::WelcomeBrand);
                    app.current_session_id = Some(new_session_id.clone());
                    app.session_title = None; // Reset title for new session
                    let _ = app.channels.resume_tx.send(new_session_id.clone());
                    app.push(ChatLine::Info(format!(
                        "  ✦ New session started (id: {}...)",
                        short_id
                    )));
                }
                Err(e) => {
                    app.push(ChatLine::Info(format!(
                        "  ✗ Failed to create new session: {}",
                        e
                    )));
                }
            }
        }
        "/help" => cmd_help(app),
        "/keys" => cmd_keys(app),
        "/fast" => cmd_fast(app),
        "/smart" => cmd_smart(app),
        _ if cmd == "/model" || cmd.starts_with("/model ") => cmd_model(app, cmd),
        "/models" => cmd_models(app),
        "/fav" => cmd_fav(app),
        _ if cmd == "/reviewer" || cmd.starts_with("/reviewer ") => cmd_reviewer(app, cmd),
        "/session" => match &app.current_session_id {
            Some(id) => app.push(ChatLine::Info(format!("  Session ID: {}", id))),
            None => app.push(ChatLine::Info("  No active session yet".into())),
        },
        "/sessions" => cmd_sessions(app),
        "/workdir" => app.push(ChatLine::Info(format!(
            "  Working directory: {}",
            app.workspace_root.display()
        ))),
        _ if cmd.starts_with("/workdir ") => cmd_workdir_arg(app, cmd),
        "/stop" => {
            if app.busy {
                app.stop_only();
            } else {
                app.push(ChatLine::Info("  ✗ No active run to stop".into()));
            }
        }
        _ if cmd.starts_with("/note") => cmd_note(app, cmd),
        _ if cmd == "/copy" || cmd.starts_with("/copy ") => cmd_copy(app, cmd),
        "/quit" => {
            app.quit = true;
        }
        _ if cmd == "/search" || cmd.starts_with("/search ") => cmd_search(app, cmd),
        "/export" => cmd_export(app),
        _ if cmd == "/memory" || cmd.starts_with("/memory ") => cmd_memory(app, cmd),
        "/cost" => cmd_cost(app),
        "/tokens" => cmd_tokens(app),
        "/compact" => {
            if app.busy {
                app.push(ChatLine::Info(
                    "  Agent is busy — try /compact when idle".into(),
                ));
            } else {
                app.push(ChatLine::Info("  ↻ Compressing context window…".into()));
                let _ = app.channels.compact_now_tx.send(());
            }
        }
        "/todo" => cmd_todo(app),
        _ if cmd == "/skills" || cmd.starts_with("/skills ") => cmd_skills(app, cmd),
        "/undo" => cmd_undo(app),
        _ if cmd == "/rollback" || cmd.starts_with("/rollback ") => cmd_rollback(app, cmd),
        _ if cmd == "/panel" || cmd.starts_with("/panel ") => cmd_panel(app, cmd),
        _ if cmd == "/tasks" || cmd.starts_with("/tasks ") => cmd_tasks(app, cmd),
        _ if cmd == "/progress" || cmd.starts_with("/progress ") => cmd_progress_strip(app, cmd),
        _ if cmd == "/plan" || cmd.starts_with("/plan ") => cmd_plan(app, cmd),
        _ if cmd == "/branch" || cmd.starts_with("/branch ") => cmd_branch(app, cmd),
        "/sync" => cmd_sync(app),
        _ if cmd == "/pr" || cmd.starts_with("/pr ") => cmd_pr(app, cmd),
        _ if cmd == "/ship" || cmd.starts_with("/ship ") => cmd_ship(app, cmd),
        _ if cmd == "/save" || cmd.starts_with("/save ") => cmd_save(app, cmd),
        "/check" => {
            app.push(ChatLine::Info("  ↻ Running project diagnostics…".into()));
            // Send a message to the agent asking it to run diagnostics on the current project.
            app.send_now("Run diagnostics on the current project".to_string());
        }
        _ if cmd == "/notify" || cmd.starts_with("/notify ") => cmd_notify(app, cmd),
        "/index" => cmd_index(app),
        "/rules" => cmd_rules(app, ""),
        _ if cmd == "/image" || cmd.starts_with("/image ") => cmd_image(app, cmd),
        _ if cmd == "/allow-path" || cmd.starts_with("/allow-path ") => cmd_allow_path(app, cmd),
        "/allowed-paths" => cmd_allowed_paths(app),
        "/agents" => cmd_agents(app),
        "/profiles" => cmd_profiles(app),
        "/profile" => cmd_profile(app),
        "/profile new" => {
            let config_path = clido_core::global_config_path()
                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
            let all_profiles = clido_core::load_config(&app.workspace_root)
                .map(|c| c.profiles)
                .unwrap_or_default();
            app.profile_overlay = Some(ProfileOverlayState::for_create(config_path, &all_profiles));
        }
        _ if cmd == "/profile edit" || cmd.starts_with("/profile edit ") => {
            cmd_profile_edit(app, cmd)
        }
        _ if cmd == "/profile delete" || cmd.starts_with("/profile delete ") => {
            cmd_profile_delete(app, cmd)
        }
        _ if cmd.starts_with("/profile ") => cmd_profile_switch(app, cmd),
        "/settings" => cmd_config(app),
        "/config" => cmd_config(app),
        _ if cmd == "/configure" || cmd.starts_with("/configure ") => cmd_configure(app, cmd),
        "/init" => cmd_init(app),

        // ── Prompt Enhancement ──────────────────────────────────────────────
        _ if cmd == "/enhance" || cmd.starts_with("/enhance ") => cmd_enhance(app, cmd),

        // ── Workflows ───────────────────────────────────────────────────────
        _ if cmd == "/workflow" || cmd.starts_with("/workflow ") => cmd_workflow(app, cmd),

        // ── Update ──────────────────────────────────────────────────────────
        "/update" => {
            use crate::update_check::spawn_do_update;
            app.push(ChatLine::Info(
                "  ↻ Checking for updates and downloading…".into(),
            ));
            spawn_do_update(None, app.channels.fetch_tx.clone());
        }

        _ => {}
    }
}
