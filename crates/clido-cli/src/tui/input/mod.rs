use clido_planner::Complexity;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::list_picker::ListPicker;
use crate::overlay::{AppAction, ErrorOverlay, OverlayKeyResult, OverlayKind};

use super::*;

mod overlay;
mod plan_editor;
mod profile;
mod scroll;
mod workflow_editor;

pub(super) use overlay::handle_app_action;
pub(super) use plan_editor::{handle_plan_editor_key, handle_plan_text_editor_key};
pub(super) use profile::{
    char_byte_pos_tui, delete_char_at_cursor_pe, delete_char_before_cursor_pe,
    handle_profile_overlay_key,
};
pub(super) use scroll::{scroll_down, scroll_up};
pub(super) use workflow_editor::handle_workflow_editor_key;

pub(super) fn handle_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    // Ctrl+C: interrupt agent when busy, quit when idle.
    if matches!((event.modifiers, event.code), (Km::CONTROL, Char('c'))) {
        if app.busy || app.current_step.is_some() {
            app.stop_only();
            return;
        }
        app.quit = true;
        return;
    }
    // Ctrl+D: quit only when input is empty (prevents accidental quits)
    if matches!((event.modifiers, event.code), (Km::CONTROL, Char('d')))
        && app.session_picker.is_none()
        && app.text_input.text.is_empty()
    {
        app.quit = true;
        return;
    }

    // Ctrl+/ interrupts the current run without sending follow-up input.
    if matches!((event.modifiers, event.code), (Km::CONTROL, Char('/'))) {
        app.stop_only();
        return;
    }
    if matches!((event.modifiers, event.code), (Km::CONTROL, Char('y'))) {
        match app.last_assistant_text() {
            Some(text) => {
                if let Err(e) = copy_to_clipboard(text) {
                    app.push_toast(
                        format!("Copy failed: {}", e),
                        TUI_STATE_ERR,
                        std::time::Duration::from_secs(3),
                    );
                } else {
                    app.push_toast(
                        "Copied to clipboard",
                        TUI_STATE_OK,
                        std::time::Duration::from_secs(2),
                    );
                }
            }
            None => app.push_toast(
                "Nothing to copy yet",
                TUI_STATE_WARN,
                std::time::Duration::from_secs(2),
            ),
        }
        return;
    }

    // Ctrl+V: system clipboard → input (terminals that do not emit `Paste` for this binding).
    if matches!((event.modifiers, event.code), (Km::CONTROL, Char('v'))) && !app.selection_mode {
        match read_clipboard() {
            Ok(s) if !s.is_empty() => {
                let text = s.replace("\r\n", "\n").replace('\r', "\n");
                let byte_pos = char_byte_pos(&app.text_input.text, app.text_input.cursor);
                app.text_input.text.insert_str(byte_pos, &text);
                app.text_input.cursor += text.chars().count();
                app.selected_cmd = None;
                app.text_input.history_idx = None;
            }
            Ok(_) => {}
            Err(e) => app.push_toast(
                format!("Clipboard: {}", e),
                TUI_STATE_WARN,
                std::time::Duration::from_secs(3),
            ),
        }
        return;
    }

    // ── Plan text editor (nano-style, intercepts all keys) ───────────────────
    if app.plan.text_editor.is_some() {
        handle_plan_text_editor_key(app, event);
        return;
    }

    // ── Workflow editor (nano-style, intercepts all keys) ────────────────────
    if app.workflow_editor.is_some() {
        handle_workflow_editor_key(app, event);
        return;
    }

    // ── Plan editor (full-screen modal — intercepts all keys) ────────────────
    if app.plan.editor.is_some() {
        handle_plan_editor_key(app, event);
        return;
    }

    // ── Profile overlay (overview / field editor / create wizard) ────────────
    if app.profile_overlay.is_some() {
        handle_profile_overlay_key(app, event);
        return;
    }

    // ── Pending path permission request ─────────────────────────────────────
    if let Some(ref path) = app.pending_path_permission {
        match event.code {
            Char('y') | Char('Y') => {
                // Allow this attempt: add the smallest sensible scope to session allow-list
                // so PathGuard permits the retry (same as a one-shot "yes" for this path).
                if !path.as_os_str().is_empty() {
                    let scope = std::fs::canonicalize(path).ok().map(|c| {
                        if c.is_dir() {
                            c
                        } else {
                            c.parent().map(|p| p.to_path_buf()).unwrap_or(c)
                        }
                    });
                    if let Some(scope) = scope {
                        if !app.allowed_external_paths.contains(&scope) {
                            app.allowed_external_paths.push(scope.clone());
                        }
                        let _ = app
                            .channels
                            .allowed_paths_tx
                            .send(app.allowed_external_paths.clone());
                    }
                }
                let _ = app.channels.path_permission_tx.send(path.clone());
                app.push(ChatLine::Info(format!(
                    "  ✓ Allowed access to: {}",
                    path.display()
                )));
                app.pending_path_permission = None;
                return;
            }
            Char('n') | Char('N') | Esc => {
                // Deny - send empty path to signal denial
                let _ = app
                    .channels
                    .path_permission_tx
                    .send(std::path::PathBuf::new());
                app.push(ChatLine::Info(format!(
                    "  ✗ Denied access to: {}",
                    path.display()
                )));
                app.pending_path_permission = None;
                return;
            }
            Char('a') | Char('A') => {
                // Always allow - add to allowed paths for this session
                app.allowed_external_paths.push(path.clone());
                let _ = app
                    .channels
                    .allowed_paths_tx
                    .send(app.allowed_external_paths.clone());
                let _ = app.channels.path_permission_tx.send(path.clone());
                app.push(ChatLine::Info(format!(
                    "  ✓ Added to allowed paths: {} (and granted access)",
                    path.display()
                )));
                app.pending_path_permission = None;
                return;
            }
            _ => {
                // Ignore other keys while waiting for permission response
                return;
            }
        }
    }

    // ── Overlay stack (new system) ───────────────────────────────────────────
    match app.overlay_stack.handle_key(event) {
        OverlayKeyResult::Consumed => return,
        OverlayKeyResult::Action(action) => {
            handle_app_action(app, action);
            return;
        }
        OverlayKeyResult::NotHandled | OverlayKeyResult::NoOverlay => {}
    }

    // ── Model picker (modal) ─────────────────────────────────────────────────
    if app.model_picker.is_some() {
        const VISIBLE: usize = 14;
        // Ctrl+S: save default · Ctrl+F: toggle favorite (letters go to filter)
        if event.modifiers == Km::CONTROL {
            match event.code {
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    if let Some(picker) = &app.model_picker {
                        let filtered = picker.filtered();
                        if !filtered.is_empty() {
                            let model_id = filtered[picker.selected].id.clone();
                            drop(filtered);
                            let config_path = clido_core::global_config_path()
                                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                            match save_default_model_to_config(
                                &config_path,
                                &model_id,
                                &app.current_profile,
                            ) {
                                Ok(()) => {
                                    app.push(ChatLine::Info(format!(
                                        "  ✓ {} saved as default model",
                                        model_id
                                    )));
                                }
                                Err(e) => {
                                    app.push(ChatLine::Info(format!("  ✗ could not save: {}", e)));
                                }
                            }
                            app.model_picker = None;
                        }
                    }
                    return;
                }
                KeyCode::Char('f') | KeyCode::Char('F') => {
                    if let Some(picker) = &mut app.model_picker {
                        let filtered = picker.filtered();
                        if !filtered.is_empty() {
                            let model_id = filtered[picker.selected].id.clone();
                            drop(filtered);
                            app.model_prefs.toggle_favorite(&model_id);
                            app.model_prefs.save();
                            let (pricing, _) = clido_core::load_pricing();
                            app.known_models = build_model_list(&pricing, &app.model_prefs);
                            picker.models = app.known_models.clone();
                            picker.clamp();
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        match event.code {
            Up => {
                if let Some(picker) = &mut app.model_picker {
                    let n = picker.filtered().len();
                    if n > 0 && picker.selected > 0 {
                        picker.selected -= 1;
                        if picker.selected < picker.scroll_offset {
                            picker.scroll_offset = picker.selected;
                        }
                    }
                }
            }
            Down => {
                if let Some(picker) = &mut app.model_picker {
                    let n = picker.filtered().len();
                    if n > 0 && picker.selected + 1 < n {
                        picker.selected += 1;
                        if picker.selected >= picker.scroll_offset + VISIBLE {
                            picker.scroll_offset = picker.selected - VISIBLE + 1;
                        }
                    }
                }
            }
            Enter => {
                if let Some(picker) = app.model_picker.take() {
                    let filtered = picker.filtered();
                    if !filtered.is_empty() {
                        let entry = filtered[picker.selected].clone();
                        // Switch model.
                        app.model = entry.id.clone();
                        let _ = app.channels.model_switch_tx.send(entry.id.clone());
                        // Update recency.
                        app.model_prefs.push_recent(&entry.id);
                        app.model_prefs.save();
                        app.push(ChatLine::Info(format!("  ✓ Model: {}", entry.id)));
                    }
                }
            }
            Esc => {
                app.model_picker = None;
            }
            KeyCode::Backspace => {
                if let Some(picker) = &mut app.model_picker {
                    picker.filter.pop();
                    picker.clamp();
                }
            }
            KeyCode::Char(c) => {
                if let Some(picker) = &mut app.model_picker {
                    picker.filter.push(c);
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            KeyCode::Home => {
                // Jump to first result
                if let Some(picker) = &mut app.model_picker {
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            KeyCode::End => {
                // Jump to last result
                if let Some(picker) = &mut app.model_picker {
                    let n = picker.filtered().len();
                    if n > 0 {
                        picker.selected = n - 1;
                        picker.scroll_offset = picker.selected.saturating_sub(VISIBLE - 1);
                    }
                }
            }
            _ => {}
        }
        return;
    }

    // ── Session picker (modal) ────────────────────────────────────────────────
    if app.session_picker.is_some() {
        if event.modifiers == Km::CONTROL {
            if let KeyCode::Char('d') | KeyCode::Char('D') = event.code {
                if let Some(picker) = &mut app.session_picker {
                    // If no multi-selection, select the current item
                    if picker.selected.is_empty() {
                        if let Some(s) = picker.picker.selected_item() {
                            picker.selected.insert(s.session_id.clone());
                        }
                    }
                    // Delete selected sessions
                    let to_delete: Vec<String> = picker.selected.iter().cloned().collect();
                    let mut deleted = 0;
                    let mut skipped = 0;
                    for sid in to_delete {
                        if app.current_session_id.as_deref() == Some(&sid) {
                            skipped += 1;
                            continue; // Skip active session
                        }
                        if clido_storage::delete_session(&app.workspace_root, &sid).is_ok() {
                            deleted += 1;
                        }
                    }
                    if deleted > 0 {
                        app.push(ChatLine::Info(format!("  Deleted {} sessions", deleted)));
                    }
                    if skipped > 0 {
                        app.push(ChatLine::Info(format!(
                            "  Skipped {} active session(s)",
                            skipped
                        )));
                    }
                    // Refresh the picker
                    crate::tui::commands::cmd_sessions(app);
                }
                return;
            }
        }
        match event.code {
            Enter => {
                if let Some(picker) = app.session_picker.take() {
                    if let Some(s) = picker.picker.selected_item() {
                        app.text_input.text.clear();
                        app.text_input.cursor = 0;
                        let id = s.session_id.clone();
                        if app.current_session_id.as_deref() == Some(&id) {
                            app.push(ChatLine::Info("  Already in this session".into()));
                        } else {
                            let _ = app.channels.resume_tx.send(id);
                        }
                    }
                }
            }
            Esc => {
                app.session_picker = None;
                app.text_input.text.clear();
                app.text_input.cursor = 0;
            }
            KeyCode::Char(' ') => {
                // Toggle multi-selection
                if let Some(picker) = &mut app.session_picker {
                    if let Some(s) = picker.picker.selected_item() {
                        let sid = s.session_id.clone();
                        if picker.selected.contains(&sid) {
                            picker.selected.remove(&sid);
                        } else {
                            picker.selected.insert(sid);
                        }
                    }
                }
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                // Clear selection
                if let Some(picker) = &mut app.session_picker {
                    picker.selected.clear();
                }
            }
            _ => {
                if let Some(picker) = &mut app.session_picker {
                    picker.picker.handle_key(event);
                }
            }
        }
        return;
    }

    // ── Profile picker (modal) ────────────────────────────────────────────────
    if app.profile_picker.is_some() {
        if event.modifiers == Km::CONTROL {
            match event.code {
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    app.profile_picker = None;
                    let config_path = clido_core::global_config_path()
                        .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                    let all_profiles = clido_core::load_config(&app.workspace_root)
                        .map(|c| c.profiles)
                        .unwrap_or_default();
                    app.profile_overlay =
                        Some(ProfileOverlayState::for_create(config_path, &all_profiles));
                    return;
                }
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    if let Some(picker) = app.profile_picker.take() {
                        if let Some((name, entry)) = picker.picker.selected_item() {
                            let name = name.clone();
                            let entry_clone = entry.clone();
                            let config_path = clido_core::global_config_path()
                                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                            let all_profiles = clido_core::load_config(&app.workspace_root)
                                .map(|c| c.profiles)
                                .unwrap_or_default();
                            app.profile_overlay = Some(ProfileOverlayState::for_edit(
                                name,
                                &entry_clone,
                                config_path,
                                &all_profiles,
                            ));
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        match event.code {
            Enter => {
                if let Some(picker) = app.profile_picker.take() {
                    if let Some((name, _)) = picker.picker.selected_item() {
                        if name == &picker.active {
                            app.push(ChatLine::Info(format!(
                                "  profile '{}' is already active.",
                                name
                            )));
                        } else {
                            super::commands::switch_profile_seamless(app, name);
                        }
                    }
                }
            }
            Esc => {
                app.profile_picker = None;
            }
            _ => {
                if let Some(picker) = &mut app.profile_picker {
                    picker.picker.handle_key(event);
                }
            }
        }
        return;
    }

    // ── Permission popup (modal — arrow keys select, Enter confirms) ─────────
    if app.pending_perm.is_some() {
        const PERM_OPTIONS: usize = 5;

        // ── Feedback input mode ──────────────────────────────────────────
        if app.perm_feedback_input.is_some() {
            match event.code {
                Enter => {
                    if let (Some(perm), Some(fb)) =
                        (app.pending_perm.take(), app.perm_feedback_input.take())
                    {
                        let _ = perm.reply.send(PermGrant::DenyWithFeedback(fb));
                        app.perm_selected = 0;
                    }
                }
                Esc => {
                    // Go back to option selection without sending
                    app.perm_feedback_input = None;
                }
                Backspace => {
                    if let Some(ref mut fb) = app.perm_feedback_input {
                        fb.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(ref mut fb) = app.perm_feedback_input {
                        fb.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        // ── Normal option selection ──────────────────────────────────────
        match event.code {
            Up => {
                if app.perm_selected == 0 {
                    app.perm_selected = PERM_OPTIONS - 1;
                } else {
                    app.perm_selected -= 1;
                }
            }
            Down => {
                app.perm_selected = (app.perm_selected + 1) % PERM_OPTIONS;
            }
            Enter => {
                match app.perm_selected {
                    4 => {
                        // Deny with feedback — switch to feedback input mode
                        app.perm_feedback_input = Some(String::new());
                    }
                    _ => {
                        if let Some(perm) = app.pending_perm.take() {
                            let grant = match app.perm_selected {
                                0 => PermGrant::Once,
                                1 => PermGrant::Session,
                                2 => PermGrant::Workdir,
                                _ => PermGrant::Deny,
                            };
                            // Track AllowAll grants on the App so the UI can reflect the state
                            // and so we can reset it on workdir changes.
                            if matches!(grant, PermGrant::Session | PermGrant::Workdir) {
                                app.permission_mode_override = Some(PermissionMode::AcceptAll);
                            }
                            let _ = perm.reply.send(grant);
                            app.perm_selected = 0;
                        }
                    }
                }
            }
            Esc => {
                if let Some(perm) = app.pending_perm.take() {
                    let _ = perm.reply.send(PermGrant::Deny);
                    app.perm_selected = 0;
                }
            }
            // Number shortcuts: 1-5 for quick selection.
            Char('1') => app.perm_selected = 0,
            Char('2') => app.perm_selected = 1,
            Char('3') => app.perm_selected = 2,
            Char('4') => app.perm_selected = 3,
            Char('5') => app.perm_selected = 4,
            _ => {} // all other keys ignored while popup is active
        }
        return;
    }

    // ── Global shortcuts (no overlay active) ───────────────────────────────
    match (event.modifiers, event.code) {
        // Ctrl+M → open model picker
        (Km::CONTROL, Char('m')) => {
            let models = app.known_models.clone();
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
            return;
        }
        // Ctrl+P → open profile picker
        (Km::CONTROL, Char('p')) => {
            if let Ok(loaded) = clido_core::load_config(&app.workspace_root) {
                let active = loaded.default_profile.clone();
                let mut profiles: Vec<(String, clido_core::ProfileEntry)> =
                    loaded.profiles.into_iter().collect();
                profiles.sort_by(|a, b| a.0.cmp(&b.0));
                let selected = profiles.iter().position(|(n, _)| n == &active).unwrap_or(0);
                let mut picker = ListPicker::new(profiles, 12);
                picker.selected = selected;
                app.profile_picker = Some(ProfilePickerState { picker, active });
            }
            return;
        }
        // Ctrl+K → open /keys overlay
        (Km::CONTROL, Char('k')) => {
            execute_slash(app, "/keys");
            return;
        }
        _ => {}
    }

    // ── Slash-command popup navigation ──────────────────────────────────────
    let completions = if app.slash_popup_dismissed {
        Vec::new()
    } else {
        slash_completions(&app.text_input.text)
    };
    if !completions.is_empty() {
        // Clamp selection in case completions shrunk.
        if let Some(sel) = app.selected_cmd {
            if sel >= completions.len() {
                app.selected_cmd = Some(completions.len() - 1);
            }
        }
        match (event.modifiers, event.code) {
            (_, Up) => {
                let sel = match app.selected_cmd {
                    None | Some(0) => completions.len() - 1,
                    Some(i) => i - 1,
                };
                app.selected_cmd = Some(sel);
                return;
            }
            (_, Down) => {
                let sel = match app.selected_cmd {
                    None => 0,
                    Some(i) => (i + 1) % completions.len(),
                };
                app.selected_cmd = Some(sel);
                return;
            }
            (_, Tab) => {
                let idx = app.selected_cmd.unwrap_or(0);
                if let Some((cmd, _)) = completions.get(idx) {
                    app.text_input.text = cmd.to_string();
                    app.text_input.cursor = app.text_input.text.chars().count();
                }
                app.selected_cmd = None;
                return;
            }
            (_, Enter) => {
                if let Some(idx) = app.selected_cmd {
                    let cmd = completions[idx].0.to_string();
                    app.selected_cmd = None;
                    let needs_arg = crate::command_registry::COMMANDS
                        .iter()
                        .find(|c| c.name == cmd)
                        .map(|c| c.takes_args)
                        .unwrap_or(false);
                    if needs_arg {
                        // Populate input with the command + space so user can type the argument.
                        app.text_input.text = format!("{} ", cmd);
                        app.text_input.cursor = app.text_input.text.chars().count();
                        // Dismiss popup until user types another /
                        app.slash_popup_dismissed = true;
                    } else {
                        app.text_input.text.clear();
                        app.text_input.cursor = 0;
                        execute_slash(app, &cmd);
                    }
                    return;
                }
                // No item selected → fall through to normal Enter handling.
            }
            (_, Esc) => {
                app.selected_cmd = None;
                return;
            }
            (Km::NONE, Char(d)) if d.is_ascii_digit() => {
                let n = d.to_digit(10).unwrap_or(0) as usize;
                if n > 0 && n <= completions.len().min(9) {
                    app.selected_cmd = Some(n - 1);
                    return;
                }
            }
            _ => {}
        }
    } else {
        app.selected_cmd = None;
    }

    // ── Selection Mode: handle keys first ────────────────────────────────
    if app.selection_mode {
        match (event.modifiers, event.code) {
            (Km::NONE, Esc) => {
                app.selection_mode = false;
                app.selection.clear();
                return;
            }
            (Km::NONE, Char('y')) => {
                // Copy selection to clipboard
                if app.selection.active {
                    let text = app.get_selected_text();
                    if !text.is_empty() {
                        match app.copy_to_clipboard(&text) {
                            Ok(()) => {
                                app.push_toast(
                                    "Copied to clipboard".to_string(),
                                    TUI_STATE_OK,
                                    std::time::Duration::from_secs(2),
                                );
                            }
                            Err(e) => {
                                app.push_toast(
                                    format!("Copy failed: {}", e),
                                    TUI_STATE_ERR,
                                    std::time::Duration::from_secs(3),
                                );
                            }
                        }
                    }
                }
                app.selection_mode = false;
                app.selection.clear();
                return;
            }
            (Km::NONE, Up) => {
                let (row, col) = app.selection.focus;
                if row > 0 {
                    app.selection.update(row.saturating_sub(1), col);
                }
                return;
            }
            (Km::NONE, Down) => {
                let (row, col) = app.selection.focus;
                let max_row = app.wrapped_lines.len().saturating_sub(1);
                app.selection.update((row + 1).min(max_row), col);
                return;
            }
            (Km::NONE, Left) => {
                let (row, col) = app.selection.focus;
                if col > 0 {
                    app.selection.update(row, col - 1);
                }
                return;
            }
            (Km::NONE, Right) => {
                let (row, col) = app.selection.focus;
                let max_row = app.wrapped_lines.len().saturating_sub(1);
                let line = app
                    .wrapped_lines
                    .get(row.min(max_row))
                    .map(|l| l.plain_text())
                    .unwrap_or_default();
                let max_col = line.chars().count().saturating_sub(1);
                if col < max_col {
                    app.selection.update(row.min(max_row), col + 1);
                } else if row < max_row {
                    app.selection.update(row + 1, 0);
                }
                return;
            }
            (Km::NONE, Char(' ')) => {
                let (row, col) = app.selection.focus;
                let max_row = app.wrapped_lines.len().saturating_sub(1);
                app.selection.start(row.min(max_row), col);
                return;
            }
            _ => {}
        }
    }

    match (event.modifiers, event.code) {
        // Esc: cancel rate-limit auto-resume if pending, discard queue item if editing, otherwise clear input.
        (_, Esc) => {
            if app.rate_limit_resume_at.is_some() && !app.rate_limit_cancelled {
                app.rate_limit_cancelled = true;
                app.rate_limit_resume_at = None;
                app.push(ChatLine::Info(
                    "  ✗ Auto-resume cancelled. Use /profile <name> to switch provider or just type to continue manually.".into(),
                ));
                // If a workflow was waiting on rate-limit recovery, it's now stranded.
                // Apply the step's on_error policy so it doesn't zombify.
                if app.active_workflow.is_some() {
                    crate::tui::commands::handle_workflow_step_error(
                        app,
                        "rate limit auto-resume cancelled by user".into(),
                    );
                }
            } else if app.rate_limit_pinging && !app.rate_limit_cancelled {
                app.rate_limit_cancelled = true;
                app.rate_limit_pinging = false;
                app.rate_limit_next_ping = None;
                app.rate_limit_ping_count = 0;
                app.push(ChatLine::Info(
                    "  ✗ Background rate-limit checks cancelled — retry manually when ready."
                        .into(),
                ));
                if app.active_workflow.is_some() {
                    crate::tui::commands::handle_workflow_step_error(
                        app,
                        "rate limit auto-resume cancelled by user".into(),
                    );
                }
            } else if app.editing_queued_item.is_some() {
                // Discard the queue item being edited
                app.editing_queued_item = None;
                app.queue_nav_idx = None;
                app.text_input.text = app.text_input.history_draft.clone();
                app.text_input.cursor = app.text_input.text.chars().count();
                app.selected_cmd = None;
                app.push_toast(
                    "Queue edit discarded".to_string(),
                    TUI_STATE_WARN,
                    std::time::Duration::from_secs(2),
                );
            } else {
                app.text_input.text.clear();
                app.text_input.cursor = 0;
                app.selected_cmd = None;
                app.text_input.history_idx = None;
            }
        }
        // Enter: submit the message.
        (Km::NONE, Enter) => app.submit(),
        // Shift+Enter: insert a newline without sending (multiline input).
        // Requires Kitty keyboard protocol support (kitty, WezTerm, Ghostty, foot).
        // Ctrl+J: reliable fallback for inserting a newline on all terminals.
        (Km::SHIFT, Enter) | (Km::CONTROL, Char('j')) | (Km::ALT, Enter) => {
            let byte_pos = char_byte_pos(&app.text_input.text, app.text_input.cursor);
            app.text_input.text.insert(byte_pos, '\n');
            app.text_input.cursor += 1;
            app.selected_cmd = None;
            app.text_input.history_idx = None;
        }
        // Ctrl+Enter: interrupt current run and force-send immediately.
        (Km::CONTROL, Enter) => app.force_send(),
        // Ctrl+Shift+C: enter copy mode for selecting chat lines.
        (Km::CONTROL, Char('c')) if event.modifiers.contains(Km::SHIFT) => {
            app.selection_mode = !app.selection_mode;
            if app.selection_mode {
                app.selection.clear();
                let max_row = app.wrapped_lines.len().saturating_sub(1);
                let start_row = (app.scroll as usize).min(max_row);
                app.selection.start(start_row, 0);
                app.push_toast(
                    "Copy mode: ↑↓←→ move · Space toggle · y copy · Esc exit".to_string(),
                    TUI_STATE_WARN,
                    std::time::Duration::from_secs(5),
                );
            }
        }
        (_, Backspace) => {
            if app.text_input.cursor > 0 {
                let byte_pos = char_byte_pos(&app.text_input.text, app.text_input.cursor - 1);
                app.text_input.text.remove(byte_pos);
                app.text_input.cursor -= 1;
                app.selected_cmd = None;
                app.text_input.history_idx = None;
            }
        }
        (_, Delete) => {
            if app.text_input.cursor < app.text_input.text.chars().count() {
                let byte_pos = char_byte_pos(&app.text_input.text, app.text_input.cursor);
                app.text_input.text.remove(byte_pos);
                app.selected_cmd = None;
                app.text_input.history_idx = None;
            }
        }
        // Alt+Left: move cursor to start of previous word.
        (Km::ALT, Left) => {
            if app.text_input.cursor > 0 {
                let chars: Vec<char> = app.text_input.text.chars().collect();
                let mut new_cursor = app.text_input.cursor;
                // Skip spaces
                while new_cursor > 0 && chars[new_cursor - 1] == ' ' {
                    new_cursor -= 1;
                }
                // Skip word characters
                while new_cursor > 0 && chars[new_cursor - 1] != ' ' {
                    new_cursor -= 1;
                }
                app.text_input.cursor = new_cursor;
            }
        }
        // Alt+Right: move cursor to end of next word.
        (Km::ALT, Right) => {
            let chars: Vec<char> = app.text_input.text.chars().collect();
            let len = chars.len();
            let mut new_cursor = app.text_input.cursor;
            // Skip spaces
            while new_cursor < len && chars[new_cursor] == ' ' {
                new_cursor += 1;
            }
            // Skip word characters
            while new_cursor < len && chars[new_cursor] != ' ' {
                new_cursor += 1;
            }
            app.text_input.cursor = new_cursor;
        }
        (_, Left) => {
            if app.text_input.cursor > 0 {
                app.text_input.cursor -= 1;
            }
        }
        (_, Right) => {
            if app.text_input.cursor < app.text_input.text.chars().count() {
                app.text_input.cursor += 1;
            }
        }
        // ── Jump to top / bottom of chat ─────────────────────────────────────
        (Km::CONTROL, Home) => {
            app.scroll = 0;
            app.following = false;
        }
        (Km::CONTROL, End) => {
            app.following = true;
        }
        (_, Home) => app.text_input.cursor = 0,
        (_, End) => app.text_input.cursor = app.text_input.text.chars().count(),
        // ── Up: queue nav (editing removes from queue) OR multiline cursor movement OR input history ────
        // Chat scrolling is separate (PageUp/PageDown/mouse wheel only).
        (_, Up)
            if app.pending_perm.is_none() && slash_completions(&app.text_input.text).is_empty() =>
        {
            // First: cycle through queued items (newest first) before history
            // When selecting a queue item, it gets REMOVED from queue for editing
            if !app.queued.is_empty() && app.text_input.history_idx.is_none() {
                let queue_len = app.queued.len();
                let new_idx = match app.queue_nav_idx {
                    None => queue_len - 1,    // Start with newest (back of queue)
                    Some(0) => queue_len - 1, // Wrap around to newest
                    Some(i) => i - 1,         // Move to older item
                };

                // If we're already editing a queue item, save current text back to the same slot
                if let Some(edit_idx) = app.queue_nav_idx {
                    if !app.text_input.text.trim().is_empty() {
                        app.queued[edit_idx] = app.text_input.text.clone();
                    }
                } else {
                    // Save current draft when first entering queue nav
                    app.text_input.history_draft = app.text_input.text.clone();
                }

                app.queue_nav_idx = Some(new_idx);
                // Remove item from queue to edit it
                if let Some(item) = app.queued.remove(new_idx) {
                    app.editing_queued_item = Some(item.clone());
                    app.text_input.text = item;
                    app.text_input.cursor = app.text_input.text.chars().count(); // Place cursor at end
                    app.selected_cmd = None;
                    return;
                }
            }

            if app.text_input.text.contains('\n') && app.text_input.history_idx.is_none() {
                if let Some(new_cursor) =
                    move_cursor_line_up(&app.text_input.text, app.text_input.cursor)
                {
                    app.text_input.cursor = new_cursor;
                    return;
                }
            }
            // Navigate input history (works with empty input or when already browsing).
            if !app.text_input.history.is_empty() {
                // Clear queue nav when switching to history
                app.queue_nav_idx = None;
                let new_idx = match app.text_input.history_idx {
                    None => {
                        app.text_input.history_draft = app.text_input.text.clone();
                        app.text_input.history.len() - 1
                    }
                    Some(0) => 0,
                    Some(i) => i - 1,
                };
                app.text_input.history_idx = Some(new_idx);
                app.text_input.text = app.text_input.history[new_idx].clone();
                // Place cursor at end for easier editing
                app.text_input.cursor = app.text_input.text.chars().count();
                app.selected_cmd = None;
            }
        }
        // ── Down: queue nav (put current back in queue) OR multiline cursor movement OR input history ─
        (_, Down)
            if app.pending_perm.is_none() && slash_completions(&app.text_input.text).is_empty() =>
        {
            // Handle queue navigation (moving forward through queue)
            if let Some(idx) = app.queue_nav_idx {
                // Save current text back to the queue slot
                if !app.text_input.text.trim().is_empty() {
                    // Re-insert at the same position
                    app.queued.insert(idx, app.text_input.text.clone());
                }
                app.editing_queued_item = None;

                // Now move to next item
                if idx >= app.queued.len() {
                    // Exhausted queue, return to draft and switch to history
                    app.queue_nav_idx = None;
                    app.text_input.text = app.text_input.history_draft.clone();
                    app.text_input.cursor = app.text_input.text.chars().count();
                    app.selected_cmd = None;
                } else if let Some(item) = app.queued.remove(idx) {
                    // Get next item from queue
                    app.editing_queued_item = Some(item.clone());
                    app.text_input.text = item;
                    app.text_input.cursor = app.text_input.text.chars().count();
                    app.selected_cmd = None;
                    // Keep same idx since we removed current and next shifted up
                }
                return;
            }

            if app.text_input.text.contains('\n') && app.text_input.history_idx.is_none() {
                if let Some(new_cursor) =
                    move_cursor_line_down(&app.text_input.text, app.text_input.cursor)
                {
                    app.text_input.cursor = new_cursor;
                    return;
                }
            }
            // Navigate input history forward.
            if let Some(i) = app.text_input.history_idx {
                if i + 1 >= app.text_input.history.len() {
                    app.text_input.history_idx = None;
                    app.text_input.text = app.text_input.history_draft.clone();
                    app.text_input.cursor = app.text_input.text.chars().count();
                    app.selected_cmd = None;
                } else {
                    let new_idx = i + 1;
                    app.text_input.history_idx = Some(new_idx);
                    app.text_input.text = app.text_input.history[new_idx].clone();
                    app.text_input.cursor = app.text_input.text.chars().count();
                    app.selected_cmd = None;
                }
            }
        }
        // ── Chat scroll (PageUp/PageDown) or status rail scroll (Alt+PgUp/PgDn) ─
        (_, PageUp) => {
            if app.layout.status_rail_active && event.modifiers.contains(Km::ALT) {
                app.status_panel_scroll = app.status_panel_scroll.saturating_sub(3);
            } else {
                scroll_up(app, 3);
            }
        }
        (_, PageDown) => {
            if app.layout.status_rail_active && event.modifiers.contains(Km::ALT) {
                app.status_panel_scroll =
                    (app.status_panel_scroll + 3).min(app.layout.status_panel_max_scroll);
            } else {
                scroll_down(app, 3);
            }
        }
        (Km::CONTROL, Char('u')) => {
            app.text_input.text.clear();
            app.text_input.cursor = 0;
            app.selected_cmd = None;
            // Don't clear history_idx — user may want to navigate back
        }
        // Ctrl+W: delete word backward (to previous word boundary).
        (Km::CONTROL, Char('w')) => {
            if app.text_input.cursor > 0 {
                let chars: Vec<char> = app.text_input.text.chars().collect();
                let mut new_cursor = app.text_input.cursor;
                // Skip trailing spaces
                while new_cursor > 0 && chars[new_cursor - 1] == ' ' {
                    new_cursor -= 1;
                }
                // Skip word characters
                while new_cursor > 0 && chars[new_cursor - 1] != ' ' {
                    new_cursor -= 1;
                }
                let removed: String = chars[new_cursor..app.text_input.cursor].iter().collect();
                let end_byte = char_byte_pos(&app.text_input.text, app.text_input.cursor);
                let start_byte = end_byte - removed.len();
                app.text_input.text.drain(start_byte..end_byte);
                app.text_input.cursor = new_cursor;
                app.selected_cmd = None;
                app.text_input.history_idx = None;
            }
        }
        (_, Char(c)) => {
            let byte_pos = char_byte_pos(&app.text_input.text, app.text_input.cursor);
            app.text_input.text.insert(byte_pos, c);
            app.text_input.cursor += 1;
            app.selected_cmd = None;
            // Any manual edit breaks out of history navigation.
            app.text_input.history_idx = None;
            // Reset slash popup dismissed when user types /
            if c == '/' {
                app.slash_popup_dismissed = false;
            }
        }
        _ => {}
    }
}

/// Return the byte position of the n-th character boundary in `s`.
pub(super) fn char_byte_pos(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Move cursor up one visual line within a multiline input.
/// Returns `Some(new_cursor)` when the cursor is not on the first line,
/// `None` when it is (caller should fall through to history navigation).
pub(super) fn move_cursor_line_up(input: &str, cursor: usize) -> Option<usize> {
    if !input.contains('\n') {
        return None;
    }
    let chars: Vec<char> = input.chars().collect();
    // Find the start of the current line and the column position.
    let mut line_start = 0usize;
    for (i, &ch) in chars[..cursor].iter().enumerate() {
        if ch == '\n' {
            line_start = i + 1;
        }
    }
    if line_start == 0 {
        return None; // Already on first line.
    }
    let col = cursor - line_start;
    // Find the start of the previous line.
    let prev_newline = line_start - 1; // index of the '\n' before current line
    let prev_line_start = chars[..prev_newline]
        .iter()
        .enumerate()
        .rfind(|(_, &c)| c == '\n')
        .map(|(i, _)| i + 1)
        .unwrap_or(0);
    let prev_line_len = prev_newline - prev_line_start;
    Some(prev_line_start + col.min(prev_line_len))
}

/// Move cursor down one visual line within a multiline input.
/// Returns `Some(new_cursor)` when the cursor is not on the last line,
/// `None` when it is (caller should fall through to history/scroll).
pub(super) fn move_cursor_line_down(input: &str, cursor: usize) -> Option<usize> {
    if !input.contains('\n') {
        return None;
    }
    let chars: Vec<char> = input.chars().collect();
    let total = chars.len();
    // Find start of current line.
    let mut line_start = 0usize;
    for (i, &ch) in chars[..cursor].iter().enumerate() {
        if ch == '\n' {
            line_start = i + 1;
        }
    }
    let col = cursor - line_start;
    // Find the next newline at or after cursor.
    let next_newline = chars[cursor..]
        .iter()
        .position(|&c| c == '\n')
        .map(|p| cursor + p);
    match next_newline {
        None => None, // Already on last line.
        Some(nl) => {
            let next_line_start = nl + 1;
            let next_line_end = chars[next_line_start..]
                .iter()
                .position(|&c| c == '\n')
                .map(|p| next_line_start + p)
                .unwrap_or(total);
            let next_line_len = next_line_end - next_line_start;
            Some(next_line_start + col.min(next_line_len))
        }
    }
}
