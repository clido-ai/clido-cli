use clido_index::RepoIndex;
use clido_memory::MemoryStore;

use crate::list_picker::ListPicker;
use crate::overlay::{ErrorOverlay, OverlayKind, ReadOnlyOverlay};
use crate::prompt_enhance::{
    project_rules_path, project_settings_path, save_prompt_mode, save_rules, PromptMode,
    PromptRules, RuleEntry,
};

use super::*;

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
        "↑↓ (empty input)   scroll conversation".into(),
    ));
    app.push(ChatLine::Info(
        "↑↓ (multiline)     move cursor between lines".into(),
    ));
    app.push(ChatLine::Info(
        "↑↓ (with text)     history navigation".into(),
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
            Type               filter items (model, provider pickers)\n\
            Backspace          remove filter char\n\
            f                  toggle favorite (model picker)\n\
            Ctrl+S             save as default (model picker)\n\
            n                  new (profile picker)\n\
            e                  edit (profile picker)"
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
        .config_roles
        .get("fast")
        .cloned()
        .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string());
    app.model = new_model.clone();
    let _ = app.model_switch_tx.send(new_model.clone());
    app.model_prefs.push_recent(&new_model);
    app.model_prefs.save();
    app.push(ChatLine::Info(format!("  ✓ Model: {} (fast)", new_model)));
}

pub(super) fn cmd_smart(app: &mut App) {
    let new_model = app
        .config_roles
        .get("reasoning")
        .cloned()
        .unwrap_or_else(|| "claude-opus-4-6".to_string());
    app.model = new_model.clone();
    let _ = app.model_switch_tx.send(new_model.clone());
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
                app.fetch_tx.clone(),
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
        let _ = app.model_switch_tx.send(new_model.clone());
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
            app.fetch_tx.clone(),
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

pub(super) fn cmd_role(app: &mut App, cmd: &str) {
    let role = cmd.trim_start_matches("/role").trim();
    if role.is_empty() || role == "list" {
        // No name given or "/role list" → open interactive role picker.
        let mut roles: Vec<(String, String)> = app
            .config_roles
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        roles.sort_by(|a, b| a.0.cmp(&b.0));
        if roles.is_empty() {
            app.push(ChatLine::Info(
                "  No roles configured — use /role add <name> <model> to create one".into(),
            ));
            app.push(ChatLine::Info(
                "  Roles let you quickly switch between models  (e.g. fast, smart, review)".into(),
            ));
        } else {
            app.role_picker = Some(RolePickerState {
                picker: ListPicker::new(roles, 10),
            });
        }
    } else if role.starts_with("add ") {
        let args = role.trim_start_matches("add ").trim();
        let parts: Vec<&str> = args.splitn(2, ' ').collect();
        if parts.len() < 2 || parts[1].trim().is_empty() {
            app.push(ChatLine::Info(
                "  usage: /role add <name> <model_id>".into(),
            ));
        } else {
            let name = parts[0].trim().to_string();
            let model = parts[1].trim().to_string();
            app.config_roles.insert(name.clone(), model.clone());
            let config_path = clido_core::global_config_path()
                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
            let roles_vec: Vec<(String, String)> = app
                .config_roles
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            match save_roles_to_config(&config_path, &roles_vec) {
                Ok(()) => {
                    let (pricing, _) = clido_core::load_pricing();
                    app.known_models =
                        build_model_list(&pricing, &app.config_roles, &app.model_prefs);
                    app.push(ChatLine::Info(format!(
                        "  role '{name}' → {model}  (saved)"
                    )));
                }
                Err(e) => {
                    app.config_roles.remove(&name);
                    app.push(ChatLine::Info(format!("  ✗ failed to save role: {e}")));
                }
            }
        }
    } else if role.starts_with("delete ") || role.starts_with("remove ") {
        let name = role
            .trim_start_matches("delete ")
            .trim_start_matches("remove ")
            .trim();
        if name.is_empty() {
            app.push(ChatLine::Info("  usage: /role delete <name>".into()));
        } else if app.config_roles.remove(name).is_some() {
            let config_path = clido_core::global_config_path()
                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
            let roles_vec: Vec<(String, String)> = app
                .config_roles
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            match save_roles_to_config(&config_path, &roles_vec) {
                Ok(()) => {
                    let (pricing, _) = clido_core::load_pricing();
                    app.known_models =
                        build_model_list(&pricing, &app.config_roles, &app.model_prefs);
                    app.push(ChatLine::Info(format!("  role '{name}' deleted")));
                }
                Err(e) => {
                    app.push(ChatLine::Info(format!("  ✗ failed to save: {e}")));
                }
            }
        } else {
            app.push(ChatLine::Info(format!("  role '{name}' not found")));
        }
    } else {
        // Resolve: prefs override config.
        let model_id = app
            .model_prefs
            .resolve_role(role)
            .map(|s| s.to_string())
            .or_else(|| app.config_roles.get(role).cloned());
        match model_id {
            Some(id) => {
                app.model = id.clone();
                let _ = app.model_switch_tx.send(id.clone());
                app.model_prefs.push_recent(&id);
                app.model_prefs.save();
                app.push(ChatLine::Info(format!(
                    "  role '{}' → model switched to {}",
                    role, id
                )));
            }
            None => {
                app.push(ChatLine::Info(format!(
                    "  role '{}' not found — use /role to list, /role add <name> <model> to create",
                    role
                )));
            }
        }
    }
}

pub(super) fn cmd_fav(app: &mut App) {
    let model_id = app.model.clone();
    app.model_prefs.toggle_favorite(&model_id);
    app.model_prefs.save();
    // Rebuild model list with updated favorites.
    let (pricing, _) = clido_core::load_pricing();
    app.known_models = build_model_list(&pricing, &app.config_roles, &app.model_prefs);
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
            let selected = sessions
                .iter()
                .position(|s| app.current_session_id.as_deref() == Some(&s.session_id))
                .unwrap_or(0);
            let mut picker = ListPicker::new(sessions, 12);
            picker.selected = selected;
            app.session_picker = Some(SessionPickerState { picker });
        }
    }
}

pub(super) fn cmd_workdir_arg(app: &mut App, cmd: &str) {
    let arg = cmd.trim_start_matches("/workdir").trim();
    match resolve_workdir_arg(arg) {
        Ok(path) => {
            let _ = app.workdir_tx.send(path.clone());
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
                buf.push_str("Assistant: ");
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
                md.push_str(&format!("## Turn {} — Assistant\n\n{}\n\n", turn, text));
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
    if app.session_total_cost_usd == 0.0 {
        app.push(ChatLine::Info(
            "  Session cost: $0.0000 (no API calls yet)".into(),
        ));
    } else {
        app.push(ChatLine::Info(format!(
            "  Session cost: ${:.4}",
            app.session_total_cost_usd
        )));
    }
}

pub(super) fn cmd_tokens(app: &mut App) {
    let total = app.session_total_input_tokens + app.session_total_output_tokens;
    let total_str = if total >= 1000 {
        format!("{:.1}k", total as f64 / 1000.0)
    } else {
        total.to_string()
    };
    let ctx_pct = if app.context_max_tokens > 0 && app.session_input_tokens > 0 {
        let pct =
            (app.session_input_tokens as f64 / app.context_max_tokens as f64 * 100.0).min(100.0);
        format!(
            "  Context window: {:.0}% used ({} / {} tokens)",
            pct, app.session_input_tokens, app.context_max_tokens
        )
    } else {
        String::new()
    };
    app.push(ChatLine::Info(
        "  ── Session Token Usage ──────────────────────".into(),
    ));
    app.push(ChatLine::Info(format!(
        "  Input tokens:   {}",
        app.session_total_input_tokens
    )));
    app.push(ChatLine::Info(format!(
        "  Output tokens:  {}",
        app.session_total_output_tokens
    )));
    app.push(ChatLine::Info(format!("  Total tokens:   {}", total_str)));
    app.push(ChatLine::Info(format!(
        "  Estimated cost: ${:.6}",
        app.session_total_cost_usd
    )));
    if !ctx_pct.is_empty() {
        app.push(ChatLine::Info(ctx_pct));
    }
    if app.session_turn_count > 0 {
        app.push(ChatLine::Info(format!(
            "  Turns completed: {}",
            app.session_turn_count
        )));
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

pub(super) fn cmd_plan(app: &mut App, cmd: &str) {
    let sub = cmd.trim_start_matches("/plan").trim().to_string();
    match sub.as_str() {
        "edit" => {
            if let Some(raw) = app.last_plan_raw.clone() {
                app.plan_text_editor = Some(PlanTextEditor::from_raw(&raw));
            } else if let Some(plan) = app.last_plan_snapshot.clone() {
                let raw = plan
                    .tasks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("Step {}: {}", i + 1, t.description))
                    .collect::<Vec<_>>()
                    .join("\n");
                app.plan_text_editor = Some(PlanTextEditor::from_raw(&raw));
            } else if let Some(tasks) = app.last_plan.clone() {
                // fallback for plans from --plan mode (no raw text available)
                let raw = tasks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("Step {}: {}", i + 1, t))
                    .collect::<Vec<_>>()
                    .join("\n");
                app.plan_text_editor = Some(PlanTextEditor::from_raw(&raw));
            } else {
                app.push(ChatLine::Info(
                    "  ✗ No plan yet — use /plan <task> to create one".into(),
                ));
            }
        }
        "save" => {
            if let Some(ref editor) = app.plan_editor {
                app.last_plan_snapshot = Some(editor.plan.clone());
                app.last_plan = Some(
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
            } else if let Some(ref plan) = app.last_plan_snapshot {
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
        "" => {
            // /plan with no task — show existing plan if any
            if let Some(plan) = app.last_plan_snapshot.clone() {
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
                match app.last_plan.clone() {
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
            app.awaiting_plan_response = true;
            let prompt = format!(
                "Create a detailed step-by-step plan for the following task. \
                 Number each top-level step as \"Step N: description\". \
                 You may add sub-bullets or notes under each step for clarity. \
                 Present the complete plan and then STOP — do not execute anything \
                 until the user explicitly confirms.\n\nTask: {task}"
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
         `Co-Authored-By: Claude <noreply@clido.dev>`"
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
         `Co-Authored-By: Claude <noreply@clido.dev>`"
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

pub(super) fn cmd_rules(app: &mut App) {
    let active = app.prompt_rules.active_rules();
    let mut overlay_content: Vec<(String, String)> = Vec::new();
    if active.is_empty() {
        overlay_content.push((
            "  No active rules.".to_string(),
            "Use /prompt-rules add <text> to define prompt enhancement rules.".to_string(),
        ));
    } else {
        for rule in active {
            overlay_content.push((rule.id.clone(), rule.text.clone()));
        }
    }
    app.overlay_stack
        .push(OverlayKind::ReadOnly(ReadOnlyOverlay::new(
            "Active Rules",
            overlay_content,
        )));
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

pub(super) fn cmd_agents(app: &mut App) {
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
        Ok(loaded) => {
            app.push(ChatLine::Info("  Agent configuration:".into()));
            if let Some(main) = &loaded.agents.main {
                app.push(ChatLine::Info(format!(
                    "  main      {} / {}",
                    main.provider, main.model
                )));
            } else {
                app.push(ChatLine::Info(
                    "  main      (using [profile.default])".into(),
                ));
            }
            if let Some(worker) = &loaded.agents.worker {
                app.push(ChatLine::Info(format!(
                    "  worker    {} / {}",
                    worker.provider, worker.model
                )));
            } else {
                app.push(ChatLine::Info(
                    "  worker    not set  (uses main agent)".into(),
                ));
            }
            if let Some(reviewer) = &loaded.agents.reviewer {
                app.push(ChatLine::Info(format!(
                    "  reviewer  {} / {}",
                    reviewer.provider, reviewer.model
                )));
            } else {
                app.push(ChatLine::Info("  reviewer  not set  (disabled)".into()));
            }
            app.push(ChatLine::Info("  Run /init to reconfigure.".into()));
        }
    }
}

pub(super) fn cmd_profiles(app: &mut App) {
    match clido_core::load_config(&app.workspace_root) {
        Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
        Ok(loaded) => {
            app.push(ChatLine::Info("  Profiles:".into()));
            let mut names: Vec<&String> = loaded.profiles.keys().collect();
            names.sort();
            for name in names {
                let entry = &loaded.profiles[name];
                let is_active = name == &loaded.default_profile;
                let marker = if is_active { "▶" } else { " " };
                app.push(ChatLine::Info(format!(
                    "  {} {}  {} / {}",
                    marker, name, entry.provider, entry.model
                )));
                if let Some(ref w) = entry.worker {
                    app.push(ChatLine::Info(format!(
                        "       worker    {} / {}",
                        w.provider, w.model
                    )));
                }
                if let Some(ref r) = entry.reviewer {
                    app.push(ChatLine::Info(format!(
                        "       reviewer  {} / {}",
                        r.provider, r.model
                    )));
                }
            }
            app.push(ChatLine::Info(
                "  /profile → pick & switch  |  /profile new → create  |  /profile edit → edit"
                    .into(),
            ));
        }
    }
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
                app.profile_overlay =
                    Some(ProfileOverlayState::for_edit(name, &entry, config_path));
            }
        },
    }
}

pub(super) fn cmd_profile_switch(app: &mut App, cmd: &str) {
    let name = cmd.trim_start_matches("/profile ").trim();
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
                app.push(ChatLine::Info(format!(
                    "  switching to profile '{}'…",
                    name
                )));
                app.restart_resume_session = app.current_session_id.clone();
                app.wants_profile_switch = Some(name.to_string());
                app.quit = true;
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
                    let worker_s = if let Some(ref w) = p.worker {
                        format!("  worker: {}/{}", w.provider, w.model)
                    } else {
                        String::new()
                    };
                    let reviewer_s = if let Some(ref r) = p.reviewer {
                        format!("  reviewer: {}/{}", r.provider, r.model)
                    } else {
                        String::new()
                    };
                    app.push(ChatLine::Info(format!(
                        "  {} {:<14}  {}/{}{}{}",
                        marker, name, p.provider, p.model, worker_s, reviewer_s
                    )));
                }
            }

            // Roles.
            let roles_map = loaded.roles.as_map();
            if !roles_map.is_empty() {
                app.push(ChatLine::Info("".into()));
                app.push(ChatLine::Section("Roles".into()));
                app.push(ChatLine::Info(
                    "  (use /role <name> to switch, /fast = fast role, /smart = reasoning role)"
                        .into(),
                ));
                let mut role_names: Vec<&String> = roles_map.keys().collect();
                role_names.sort();
                for name in role_names {
                    app.push(ChatLine::Info(format!(
                        "  {:<14}  {}",
                        name, roles_map[name]
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
            if app.session_input_tokens > 0 {
                let used = app.session_input_tokens;
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

            // Agent slots (global).
            let agents = &loaded.agents;
            if agents.main.is_some() || agents.worker.is_some() || agents.reviewer.is_some() {
                app.push(ChatLine::Info("".into()));
                app.push(ChatLine::Section("Agent Slots (global)".into()));
                if let Some(ref m) = agents.main {
                    app.push(ChatLine::Info(format!(
                        "  main      {}/{}",
                        m.provider, m.model
                    )));
                }
                if let Some(ref w) = agents.worker {
                    app.push(ChatLine::Info(format!(
                        "  worker    {}/{}",
                        w.provider, w.model
                    )));
                }
                if let Some(ref r) = agents.reviewer {
                    app.push(ChatLine::Info(format!(
                        "  reviewer  {}/{}",
                        r.provider, r.model
                    )));
                }
            }

            // Config file path.
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
            # Per-profile sub-agents (override global [agents.*]):\n\
            # [profile.default.worker]\n\
            # provider = \"anthropic\"\n\
            # model    = \"claude-haiku-4-5-20251001\"\n\
            \n\
            [agent]\n\
            max-turns            = 200     # maximum tool-use turns per run\n\
            max-budget-usd       = 5.0     # cost cap per run (omit for unlimited)\n\
            max-concurrent-tools = 4       # parallel tool calls\n\
            max-output-tokens    = 8192    # max tokens per LLM response\n\
            quiet                = false   # suppress spinner / cost footer\n\
            notify               = false   # desktop notification on completion\n\
            auto-checkpoint      = true    # checkpoint before file-mutating turns\n\
            no-rules             = false   # ignore CLIDO.md rules files\n\
            \n\
            [context]\n\
            compaction-threshold = 0.75    # compress context when 75% full\n\
            max-context-tokens   = 100000  # optional hard cap on context size\n\
            \n\
            [roles]\n\
            fast      = \"claude-haiku-4-5-20251001\"  # /fast role\n\
            reasoning = \"claude-opus-4-6\"            # /smart role\n\
            # any extra role name = \"model-id\"       # /role <name>\n\
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
            match loaded.profiles.get(&name).cloned() {
                Some(entry) => {
                    app.profile_overlay =
                        Some(ProfileOverlayState::for_edit(name, &entry, config_path));
                }
                None => {
                    // No matching profile — open a create flow for the default profile
                    app.profile_overlay = Some(ProfileOverlayState::for_create(config_path));
                }
            }
        }
    }
}

pub(super) fn cmd_prompt_mode(app: &mut App, cmd: &str) {
    let arg = cmd.trim_start_matches("/prompt-mode").trim();
    match arg {
        "auto" => {
            app.prompt_mode = PromptMode::Auto;
            let path = project_settings_path(&app.workspace_root);
            if let Err(e) = save_prompt_mode(&path, PromptMode::Auto) {
                app.push(ChatLine::Info(format!("  ⚠ Could not save: {e}")));
            }
            app.push(ChatLine::Info(
                "  ✓ Prompt enhancement: auto — prompts will be enhanced automatically".into(),
            ));
        }
        "off" => {
            app.prompt_mode = PromptMode::Off;
            let path = project_settings_path(&app.workspace_root);
            if let Err(e) = save_prompt_mode(&path, PromptMode::Off) {
                app.push(ChatLine::Info(format!("  ⚠ Could not save: {e}")));
            }
            app.push(ChatLine::Info(
                "  ✓ Prompt enhancement: off — raw input sent unchanged".into(),
            ));
        }
        "" | "status" => {
            let n_active = app.prompt_rules.active_rules().len();
            let n_total = app.prompt_rules.rules.len();
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Prompt Enhancement".into()));
            app.push(ChatLine::Info(format!(
                "  mode     {}",
                app.prompt_mode.as_str()
            )));
            app.push(ChatLine::Info(format!(
                "  rules    {n_active} active / {n_total} total"
            )));
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Info(
                "  /prompt-mode auto      enable automatic enhancement".into(),
            ));
            app.push(ChatLine::Info(
                "  /prompt-mode off       send raw input unchanged".into(),
            ));
            app.push(ChatLine::Info(
                "  /prompt-rules          view and manage rules".into(),
            ));
            app.push(ChatLine::Info(
                "  /prompt-preview        preview enhanced prompt before sending".into(),
            ));
            app.push(ChatLine::Info("".into()));
        }
        _ => {
            app.push(ChatLine::Info(
                "  Usage: /prompt-mode [auto|off|status]".into(),
            ));
        }
    }
}

pub(super) fn cmd_prompt_rules(app: &mut App, cmd: &str) {
    let arg = cmd.trim_start_matches("/prompt-rules").trim();
    if arg.is_empty() || arg == "list" {
        // Collect before any mutable borrows.
        let (active_lines, total): (Vec<String>, usize) = {
            let active = app.prompt_rules.active_rules();
            let total = app.prompt_rules.rules.len();
            let lines: Vec<String> = active
                .iter()
                .map(|r| {
                    let badge = if r.source == "inferred" {
                        "inferred"
                    } else {
                        "manual"
                    };
                    format!("  [{}]  {}  ({})", r.id, r.text, badge)
                })
                .collect();
            (lines, total)
        };
        app.push(ChatLine::Info("".into()));
        app.push(ChatLine::Section("Prompt Rules".into()));
        if active_lines.is_empty() {
            app.push(ChatLine::Info(
                "  No active rules.  Use /prompt-rules add <text> to add one.".into(),
            ));
            if total > 0 {
                app.push(ChatLine::Info(format!(
                    "  ({total} rules below confidence threshold — not yet applied)"
                )));
            }
        } else {
            for line in active_lines {
                app.push(ChatLine::Info(line));
            }
        }
        app.push(ChatLine::Info("".into()));
        app.push(ChatLine::Info(
            "  /prompt-rules add <text>     add a new rule".into(),
        ));
        app.push(ChatLine::Info(
            "  /prompt-rules remove <id>    remove a rule by id".into(),
        ));
        app.push(ChatLine::Info(
            "  /prompt-rules reset          clear all rules".into(),
        ));
        app.push(ChatLine::Info("".into()));
    } else if let Some(text) = arg.strip_prefix("add ") {
        let text = text.trim();
        if text.is_empty() {
            app.push(ChatLine::Info(
                "  Usage: /prompt-rules add <rule text>".into(),
            ));
        } else {
            let id = text
                .to_lowercase()
                .split_whitespace()
                .take(4)
                .collect::<Vec<_>>()
                .join("-");
            let rule = RuleEntry::new_manual(id, text);
            app.prompt_rules.upsert(rule);
            let path = project_rules_path(&app.workspace_root);
            if let Err(e) = save_rules(&path, &app.prompt_rules) {
                app.push(ChatLine::Info(format!("  ⚠ Could not save rules: {e}")));
            }
            app.push(ChatLine::Info(format!("  ✓ Rule added: \"{text}\"")));
        }
    } else if let Some(id) = arg.strip_prefix("remove ") {
        let id = id.trim();
        if app.prompt_rules.remove(id) {
            let path = project_rules_path(&app.workspace_root);
            let _ = save_rules(&path, &app.prompt_rules);
            app.push(ChatLine::Info(format!("  ✓ Rule removed: {id}")));
        } else {
            app.push(ChatLine::Info(format!("  ✗ No rule with id: {id}")));
        }
    } else if arg == "reset" {
        app.prompt_rules = PromptRules::default();
        let path = project_rules_path(&app.workspace_root);
        let _ = save_rules(&path, &app.prompt_rules);
        app.push(ChatLine::Info("  ✓ All rules cleared".into()));
    } else {
        app.push(ChatLine::Info(
            "  Usage: /prompt-rules [list|add <text>|remove <id>|reset]".into(),
        ));
    }
}

pub(super) fn execute_slash(app: &mut App, cmd: &str) {
    match cmd {
        "/clear" => {
            app.messages.clear();
            app.messages.push(ChatLine::WelcomeBrand);
            app.push(ChatLine::Info(
                "  Conversation cleared — new session started".into(),
            ));
        }
        "/help" => cmd_help(app),
        "/keys" => cmd_keys(app),
        "/fast" => cmd_fast(app),
        "/smart" => cmd_smart(app),
        _ if cmd == "/model" || cmd.starts_with("/model ") => cmd_model(app, cmd),
        "/models" => cmd_models(app),
        _ if cmd == "/role" || cmd.starts_with("/role ") => cmd_role(app, cmd),
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
                let _ = app.compact_now_tx.send(());
            }
        }
        "/todo" => cmd_todo(app),
        "/undo" => cmd_undo(app),
        _ if cmd == "/rollback" || cmd.starts_with("/rollback ") => cmd_rollback(app, cmd),
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
        "/rules" => cmd_rules(app),
        _ if cmd == "/image" || cmd.starts_with("/image ") => cmd_image(app, cmd),
        "/agents" => cmd_agents(app),
        "/profiles" => cmd_profiles(app),
        "/profile" => cmd_profile(app),
        "/profile new" => {
            let config_path = clido_core::global_config_path()
                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
            app.profile_overlay = Some(ProfileOverlayState::for_create(config_path));
        }
        _ if cmd == "/profile edit" || cmd.starts_with("/profile edit ") => {
            cmd_profile_edit(app, cmd)
        }
        _ if cmd.starts_with("/profile ") => cmd_profile_switch(app, cmd),
        "/settings" => {
            // /settings now redirects to /role list
            execute_slash(app, "/role list");
        }
        "/config" => cmd_config(app),
        _ if cmd == "/configure" || cmd.starts_with("/configure ") => cmd_configure(app, cmd),
        "/init" => cmd_init(app),

        // ── Prompt Enhancement ──────────────────────────────────────────────
        _ if cmd == "/prompt-mode" || cmd.starts_with("/prompt-mode ") => cmd_prompt_mode(app, cmd),

        "/prompt-preview" => {
            app.prompt_preview_text = Some(String::new());
            app.push(ChatLine::Info(
                "  ✦ Preview mode — next message will be shown enhanced but not sent. Press Enter to send or Esc to cancel.".into(),
            ));
        }

        _ if cmd == "/prompt-rules" || cmd.starts_with("/prompt-rules ") => {
            cmd_prompt_rules(app, cmd)
        }

        _ => {}
    }
}
