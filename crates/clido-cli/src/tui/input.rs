use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Color;

use clido_planner::Complexity;

use crate::list_picker::ListPicker;
use crate::overlay::{AppAction, ErrorOverlay, OverlayKeyResult, OverlayKind};

use super::*;

// ── Scroll helpers ────────────────────────────────────────────────────────────

pub(super) fn scroll_up(app: &mut App, lines: u32) {
    if app.following {
        app.scroll = app.layout.max_scroll;
    }
    app.scroll = app.scroll.saturating_sub(lines);
    app.following = false;
}

pub(super) fn scroll_down(app: &mut App, lines: u32) {
    let new_scroll = app.scroll.saturating_add(lines);
    if new_scroll >= app.layout.max_scroll {
        app.scroll = app.layout.max_scroll;
        app.following = true;
    } else {
        app.scroll = new_scroll;
        app.following = false;
    }
}

// ── Plan text editor key handling (nano-style) ───────────────────────────────

pub(super) fn handle_plan_text_editor_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    let save_and_close = |app: &mut App| {
        if let Some(ed) = app.plan.text_editor.take() {
            let tasks = ed.to_tasks();
            if !tasks.is_empty() {
                app.plan.last_plan = Some(tasks.clone());
                app.plan.last_plan_snapshot = build_plan_from_tasks(&tasks);
            }
        }
    };

    match (event.modifiers, event.code) {
        (_, Esc) => {
            // Discard changes — close without saving.
            app.plan.text_editor = None;
        }
        (Km::CONTROL, Char('s')) => save_and_close(app),
        (Km::CONTROL, Char('c')) => {
            app.plan.text_editor = None;
        }
        (_, Up) => {
            if let Some(ed) = &mut app.plan.text_editor {
                if ed.cursor_row > 0 {
                    ed.cursor_row -= 1;
                    if ed.cursor_row < ed.scroll {
                        ed.scroll = ed.cursor_row;
                    }
                    ed.clamp_col();
                }
            }
        }
        (_, Down) => {
            if let Some(ed) = &mut app.plan.text_editor {
                if ed.cursor_row + 1 < ed.lines.len() {
                    ed.cursor_row += 1;
                    ed.clamp_col();
                }
            }
        }
        (_, Left) => {
            if let Some(ed) = &mut app.plan.text_editor {
                if ed.cursor_col > 0 {
                    ed.cursor_col -= 1;
                } else if ed.cursor_row > 0 {
                    ed.cursor_row -= 1;
                    ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
                }
            }
        }
        (_, Right) => {
            if let Some(ed) = &mut app.plan.text_editor {
                let line_len = ed.lines[ed.cursor_row].chars().count();
                if ed.cursor_col < line_len {
                    ed.cursor_col += 1;
                } else if ed.cursor_row + 1 < ed.lines.len() {
                    ed.cursor_row += 1;
                    ed.cursor_col = 0;
                }
            }
        }
        (_, Home) => {
            if let Some(ed) = &mut app.plan.text_editor {
                ed.cursor_col = 0;
            }
        }
        (_, End) => {
            if let Some(ed) = &mut app.plan.text_editor {
                ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
            }
        }
        (_, Enter) => {
            if let Some(ed) = &mut app.plan.text_editor {
                let line = ed.lines[ed.cursor_row].clone();
                let chars: Vec<char> = line.chars().collect();
                let col = ed.cursor_col.min(chars.len());
                let left: String = chars[..col].iter().collect();
                let right: String = chars[col..].iter().collect();
                ed.lines[ed.cursor_row] = left;
                ed.cursor_row += 1;
                ed.cursor_col = 0;
                ed.lines.insert(ed.cursor_row, right);
            }
        }
        (_, Backspace) => {
            if let Some(ed) = &mut app.plan.text_editor {
                if ed.cursor_col > 0 {
                    let line = &mut ed.lines[ed.cursor_row];
                    let mut chars: Vec<char> = line.chars().collect();
                    chars.remove(ed.cursor_col - 1);
                    *line = chars.iter().collect();
                    ed.cursor_col -= 1;
                } else if ed.cursor_row > 0 {
                    let current = ed.lines.remove(ed.cursor_row);
                    ed.cursor_row -= 1;
                    ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
                    ed.lines[ed.cursor_row].push_str(&current);
                }
            }
        }
        (_, Delete) => {
            if let Some(ed) = &mut app.plan.text_editor {
                let line_len = ed.lines[ed.cursor_row].chars().count();
                if ed.cursor_col < line_len {
                    let line = &mut ed.lines[ed.cursor_row];
                    let mut chars: Vec<char> = line.chars().collect();
                    chars.remove(ed.cursor_col);
                    *line = chars.iter().collect();
                } else if ed.cursor_row + 1 < ed.lines.len() {
                    let next = ed.lines.remove(ed.cursor_row + 1);
                    ed.lines[ed.cursor_row].push_str(&next);
                }
            }
        }
        (km, Char(c)) if km == Km::NONE || km == Km::SHIFT => {
            if let Some(ed) = &mut app.plan.text_editor {
                let line = &mut ed.lines[ed.cursor_row];
                let mut chars: Vec<char> = line.chars().collect();
                let col = ed.cursor_col.min(chars.len());
                chars.insert(col, c);
                *line = chars.iter().collect();
                ed.cursor_col += 1;
            }
        }
        _ => {}
    }

    // Scroll to keep cursor visible (rough: assume terminal ~30 rows for editor area)
    if let Some(ed) = &mut app.plan.text_editor {
        if ed.cursor_row < ed.scroll {
            ed.scroll = ed.cursor_row;
        } else if ed.cursor_row >= ed.scroll + 20 {
            ed.scroll = ed.cursor_row - 19;
        }
    }
}

// ── Workflow editor key handling (nano-style, reuses PlanTextEditor) ──────────

pub(super) fn handle_workflow_editor_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    match (event.modifiers, event.code) {
        (_, Esc) | (Km::CONTROL, Char('c')) => {
            app.workflow_editor = None;
            app.workflow_editor_path = None;
        }
        (Km::CONTROL, Char('s')) => {
            if let Some(ed) = app.workflow_editor.take() {
                let yaml_text = ed.lines.join("\n");

                // Validate YAML
                match serde_yaml::from_str::<clido_workflows::WorkflowDef>(&yaml_text) {
                    Ok(def) => {
                        // Determine save path
                        let save_dir = app.workspace_root.join(".clido").join("workflows");
                        let _ = std::fs::create_dir_all(&save_dir);
                        let path = if let Some(p) = app.workflow_editor_path.take() {
                            p
                        } else {
                            let safe_name = def
                                .name
                                .chars()
                                .map(|c| {
                                    if c.is_alphanumeric() || c == '-' || c == '_' {
                                        c
                                    } else {
                                        '-'
                                    }
                                })
                                .collect::<String>();
                            save_dir.join(format!("{safe_name}.yaml"))
                        };
                        match std::fs::write(&path, &yaml_text) {
                            Ok(()) => {
                                app.push(ChatLine::Info(format!(
                                    "  ✓ Workflow saved: {}",
                                    path.display()
                                )));
                            }
                            Err(e) => {
                                app.push(ChatLine::Info(format!(
                                    "  ✗ Failed to save workflow: {e}"
                                )));
                            }
                        }
                    }
                    Err(e) => {
                        app.push(ChatLine::Info(format!("  ✗ Invalid workflow YAML: {e}")));
                        // Re-open editor so user can fix
                        app.workflow_editor = Some(PlanTextEditor::from_raw(&yaml_text));
                    }
                }
            }
        }
        // Delegate all other keys to the same editing logic as PlanTextEditor
        _ => {
            // Re-use the PlanTextEditor editing helpers directly on workflow_editor
            if let Some(ed) = &mut app.workflow_editor {
                match (event.modifiers, event.code) {
                    (_, Up) => {
                        if ed.cursor_row > 0 {
                            ed.cursor_row -= 1;
                            if ed.cursor_row < ed.scroll {
                                ed.scroll = ed.cursor_row;
                            }
                            ed.clamp_col();
                        }
                    }
                    (_, Down) => {
                        if ed.cursor_row + 1 < ed.lines.len() {
                            ed.cursor_row += 1;
                            ed.clamp_col();
                        }
                    }
                    (_, Left) => {
                        if ed.cursor_col > 0 {
                            ed.cursor_col -= 1;
                        } else if ed.cursor_row > 0 {
                            ed.cursor_row -= 1;
                            ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
                        }
                    }
                    (_, Right) => {
                        let line_len = ed.lines[ed.cursor_row].chars().count();
                        if ed.cursor_col < line_len {
                            ed.cursor_col += 1;
                        } else if ed.cursor_row + 1 < ed.lines.len() {
                            ed.cursor_row += 1;
                            ed.cursor_col = 0;
                        }
                    }
                    (_, Home) => ed.cursor_col = 0,
                    (_, End) => {
                        ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
                    }
                    (_, Enter) => {
                        let line = ed.lines[ed.cursor_row].clone();
                        let chars: Vec<char> = line.chars().collect();
                        let col = ed.cursor_col.min(chars.len());
                        let left: String = chars[..col].iter().collect();
                        let right: String = chars[col..].iter().collect();
                        ed.lines[ed.cursor_row] = left;
                        ed.cursor_row += 1;
                        ed.cursor_col = 0;
                        ed.lines.insert(ed.cursor_row, right);
                    }
                    (_, Backspace) => {
                        if ed.cursor_col > 0 {
                            let line = &mut ed.lines[ed.cursor_row];
                            let mut chars: Vec<char> = line.chars().collect();
                            chars.remove(ed.cursor_col - 1);
                            *line = chars.iter().collect();
                            ed.cursor_col -= 1;
                        } else if ed.cursor_row > 0 {
                            let current = ed.lines.remove(ed.cursor_row);
                            ed.cursor_row -= 1;
                            ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
                            ed.lines[ed.cursor_row].push_str(&current);
                        }
                    }
                    (_, Delete) => {
                        let line_len = ed.lines[ed.cursor_row].chars().count();
                        if ed.cursor_col < line_len {
                            let line = &mut ed.lines[ed.cursor_row];
                            let mut chars: Vec<char> = line.chars().collect();
                            chars.remove(ed.cursor_col);
                            *line = chars.iter().collect();
                        } else if ed.cursor_row + 1 < ed.lines.len() {
                            let next = ed.lines.remove(ed.cursor_row + 1);
                            ed.lines[ed.cursor_row].push_str(&next);
                        }
                    }
                    (km, Char(c)) if km == Km::NONE || km == Km::SHIFT => {
                        let line = &mut ed.lines[ed.cursor_row];
                        let mut chars: Vec<char> = line.chars().collect();
                        let col = ed.cursor_col.min(chars.len());
                        chars.insert(col, c);
                        *line = chars.iter().collect();
                        ed.cursor_col += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    // Scroll to keep cursor visible
    if let Some(ed) = &mut app.workflow_editor {
        if ed.cursor_row < ed.scroll {
            ed.scroll = ed.cursor_row;
        } else if ed.cursor_row >= ed.scroll + 20 {
            ed.scroll = ed.cursor_row - 19;
        }
    }
}

// ── Plan editor key handling ──────────────────────────────────────────────────

pub(super) fn handle_plan_editor_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;

    // Ctrl+C always quits.
    if event.modifiers == crossterm::event::KeyModifiers::CONTROL && event.code == Char('c') {
        app.quit = true;
        return;
    }

    // If the inline edit form is active, handle form keys.
    if app.plan.task_editing.is_some() {
        match event.code {
            Tab => {
                if let Some(ref mut form) = app.plan.task_editing {
                    form.focused_field = match form.focused_field {
                        TaskEditField::Description => TaskEditField::Notes,
                        TaskEditField::Notes => TaskEditField::Complexity,
                        TaskEditField::Complexity => TaskEditField::Description,
                    };
                }
            }
            Left | Right => {
                if let Some(ref mut form) = app.plan.task_editing {
                    if form.focused_field == TaskEditField::Complexity {
                        form.complexity = match (&form.complexity, event.code) {
                            (Complexity::Low, Right) => Complexity::Medium,
                            (Complexity::Medium, Right) => Complexity::High,
                            (Complexity::High, Right) => Complexity::Low,
                            (Complexity::High, Left) => Complexity::Medium,
                            (Complexity::Medium, Left) => Complexity::Low,
                            (Complexity::Low, Left) => Complexity::High,
                            _ => form.complexity.clone(),
                        };
                    }
                }
            }
            Backspace => {
                if let Some(ref mut form) = app.plan.task_editing {
                    match form.focused_field {
                        TaskEditField::Description => {
                            form.description.pop();
                        }
                        TaskEditField::Notes => {
                            form.notes.pop();
                        }
                        TaskEditField::Complexity => {}
                    }
                }
            }
            Char(c) => {
                if let Some(ref mut form) = app.plan.task_editing {
                    match form.focused_field {
                        TaskEditField::Description => form.description.push(c),
                        TaskEditField::Notes => form.notes.push(c),
                        TaskEditField::Complexity => {}
                    }
                }
            }
            Enter => {
                // Save the form edits back to the plan.
                if let (Some(form), Some(ref mut editor)) =
                    (app.plan.task_editing.take(), app.plan.editor.as_mut())
                {
                    let _ = editor.rename_task(&form.task_id, &form.description);
                    let _ = editor.set_notes(&form.task_id, &form.notes);
                    let _ = editor.set_complexity(&form.task_id, form.complexity.clone());
                }
            }
            Esc => {
                app.plan.task_editing = None;
            }
            _ => {}
        }
        return;
    }

    // Task list navigation.
    let task_count = app
        .plan
        .editor
        .as_ref()
        .map(|e| e.plan.tasks.len())
        .unwrap_or(0);

    match event.code {
        Up => {
            if app.plan.selected_task > 0 {
                app.plan.selected_task -= 1;
            }
        }
        Down => {
            if app.plan.selected_task + 1 < task_count {
                app.plan.selected_task += 1;
            }
        }
        Enter => {
            // Open edit form for selected task.
            if let Some(ref editor) = app.plan.editor {
                if let Some(task) = editor.plan.tasks.get(app.plan.selected_task) {
                    app.plan.task_editing = Some(TaskEditState::new(
                        &task.id,
                        &task.description,
                        &task.notes,
                        task.complexity.clone(),
                    ));
                }
            }
        }
        Char('d') => {
            // Delete selected task.
            if let Some(ref mut editor) = app.plan.editor {
                if let Some(task) = editor.plan.tasks.get(app.plan.selected_task) {
                    let id = task.id.clone();
                    if editor.delete_task(&id).is_ok()
                        && app.plan.selected_task >= editor.plan.tasks.len()
                        && app.plan.selected_task > 0
                    {
                        app.plan.selected_task -= 1;
                    }
                }
            }
        }
        Char('n') => {
            // Add a new empty task and open edit form.
            if let Some(ref mut editor) = app.plan.editor {
                let new_id = format!("t{}", editor.plan.tasks.len() + 1);
                let _ = editor.add_task(new_id.clone(), "New task".to_string(), vec![]);
                app.plan.selected_task = editor.plan.tasks.len() - 1;
                app.plan.task_editing =
                    Some(TaskEditState::new(&new_id, "New task", "", Complexity::Low));
            }
        }
        Char(' ') => {
            // Toggle skip.
            if let Some(ref mut editor) = app.plan.editor {
                if let Some(task) = editor.plan.tasks.get(app.plan.selected_task) {
                    let id = task.id.clone();
                    let _ = editor.toggle_skip(&id);
                }
            }
        }
        Char('r') => {
            // Move selected task up (reorder).
            if let Some(ref mut editor) = app.plan.editor {
                if editor.move_up(app.plan.selected_task).is_ok() {
                    app.plan.selected_task -= 1;
                }
            }
        }
        Char('s') => {
            // Save plan to .clido/plans/.
            if let Some(ref editor) = app.plan.editor {
                match clido_planner::save_plan(&app.workspace_root, &editor.plan) {
                    Ok(path) => {
                        app.push(ChatLine::Info(format!(
                            "  ✓ Plan saved: {}",
                            path.display()
                        )));
                    }
                    Err(e) => {
                        app.overlay_stack
                            .push(OverlayKind::Error(ErrorOverlay::from_message(format!(
                                "Could not save plan: {}",
                                e
                            ))));
                    }
                }
            }
        }
        Char('x') => {
            // Execute the plan (or just close if dry-run).
            if let Some(editor) = app.plan.editor.take() {
                app.plan.task_editing = None;
                if app.plan_dry_run {
                    app.push(ChatLine::Info(
                        "  [dry-run] plan would execute now (--plan-dry-run active)".into(),
                    ));
                } else {
                    // Build a combined prompt from non-skipped tasks.
                    let tasks: Vec<String> = editor
                        .plan
                        .tasks
                        .iter()
                        .filter(|t| !t.skip)
                        .map(|t| {
                            if t.notes.is_empty() {
                                format!("{}: {}", t.id, t.description)
                            } else {
                                format!("{}: {}  (note: {})", t.id, t.description, t.notes)
                            }
                        })
                        .collect();
                    let prompt = format!(
                        "Goal: {}\n\nPlease execute the following plan in order:\n{}",
                        editor.plan.meta.goal,
                        tasks.join("\n")
                    );
                    app.send_now(prompt);
                }
            }
        }
        Esc => {
            // Abort plan — close editor without executing.
            app.plan.editor = None;
            app.plan.task_editing = None;
            app.push(ChatLine::Info("  ✗ Plan aborted".into()));
            app.busy = false;
        }
        _ => {}
    }
}

// ── Profile overlay keyboard handler ─────────────────────────────────────────

pub(super) fn handle_profile_overlay_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    let st = match app.profile_overlay.as_mut() {
        Some(s) => s,
        None => return,
    };
    st.status = None;

    match &st.mode.clone() {
        // ── Creating: step-by-step wizard ─────────────────────────────────
        ProfileOverlayMode::Creating { step } => {
            match event.code {
                Esc => {
                    app.profile_overlay = None;
                    app.push(ChatLine::Info("  Profile creation cancelled.".into()));
                }
                Enter => {
                    let value = st.input.trim().to_string();
                    match step {
                        ProfileCreateStep::Name => {
                            // Optional: if user typed a name, validate uniqueness.
                            // If blank, auto-generate later from provider.
                            if !value.is_empty() {
                                let dup = clido_core::load_config(&app.workspace_root)
                                    .ok()
                                    .map(|c| c.profiles.contains_key(&value))
                                    .unwrap_or(false);
                                if dup {
                                    st.status =
                                        Some(format!("  ✗ Profile '{}' already exists", value));
                                    return;
                                }
                                st.name = value;
                            }
                            st.input.clear();
                            st.input_cursor = 0;
                            st.provider_picker = ProviderPickerState::new();
                            st.provider_picker.clamp();
                            st.mode = ProfileOverlayMode::Creating {
                                step: ProfileCreateStep::Provider,
                            };
                        }
                        ProfileCreateStep::Provider => {
                            if let Some(id) = st.provider_picker.selected_id() {
                                st.provider = id.to_string();
                                // Auto-generate profile name only if user left it blank.
                                if st.name.is_empty() {
                                    let existing = clido_core::load_config(&app.workspace_root)
                                        .ok()
                                        .map(|c| c.profiles.keys().cloned().collect::<Vec<_>>())
                                        .unwrap_or_default();
                                    let base = id.to_string();
                                    if !existing.contains(&base) {
                                        st.name = base;
                                    } else {
                                        let mut n = 2u32;
                                        loop {
                                            let candidate = format!("{base}-{n}");
                                            if !existing.contains(&candidate) {
                                                st.name = candidate;
                                                break;
                                            }
                                            n += 1;
                                        }
                                    }
                                }
                                let needs_key = st.provider_picker.selected_requires_key();
                                let models = app.known_models.clone();
                                let mut picker = ModelPickerState {
                                    models,
                                    filter: String::new(),
                                    selected: 0,
                                    scroll_offset: 0,
                                };
                                picker.clamp();
                                st.profile_model_picker = Some(picker);
                                st.input.clear();
                                st.input_cursor = 0;
                                // Check credentials file for an existing key for this provider.
                                let saved_key = if needs_key {
                                    crate::setup::read_credential(&st.config_path, id)
                                } else {
                                    None
                                };
                                let next_step = if let Some(key) = saved_key {
                                    // Pre-fill the key and skip to model selection.
                                    st.api_key = key.clone();
                                    // Trigger model fetch with the saved key.
                                    spawn_model_fetch(
                                        st.provider.clone(),
                                        key,
                                        if st.base_url.is_empty() {
                                            None
                                        } else {
                                            Some(st.base_url.clone())
                                        },
                                        app.channels.fetch_tx.clone(),
                                    );
                                    app.models_loading = true;
                                    ProfileCreateStep::Model
                                } else if needs_key {
                                    ProfileCreateStep::ApiKey
                                } else {
                                    st.api_key.clear();
                                    ProfileCreateStep::Model
                                };
                                st.mode = ProfileOverlayMode::Creating { step: next_step };
                            } else {
                                st.status = Some("  ✗ Select a provider from the list".into());
                            }
                        }
                        ProfileCreateStep::ApiKey => {
                            // API key may be empty for local providers
                            st.api_key = value.clone();
                            st.input.clear();
                            st.input_cursor = 0;
                            // Trigger a live model fetch for this provider + key so the model
                            // picker is populated when the user reaches the model selection step.
                            let provider_for_fetch = st.provider.clone();
                            let base_url_for_fetch = if st.base_url.is_empty() {
                                None
                            } else {
                                Some(st.base_url.clone())
                            };
                            if !value.is_empty() {
                                spawn_model_fetch(
                                    provider_for_fetch,
                                    value,
                                    base_url_for_fetch,
                                    app.channels.fetch_tx.clone(),
                                );
                                app.models_loading = true;
                            }
                            st.mode = ProfileOverlayMode::Creating {
                                step: ProfileCreateStep::Model,
                            };
                        }
                        ProfileCreateStep::Model => {
                            let model_id = st.profile_model_picker.as_ref().and_then(|p| {
                                let filtered = p.filtered();
                                filtered.get(p.selected).map(|m| m.id.clone())
                            });
                            if let Some(id) = model_id {
                                st.model = id;
                                st.input.clear();
                                st.input_cursor = 0;
                                st.mode = ProfileOverlayMode::Overview;
                                let st = app
                                    .profile_overlay
                                    .as_mut()
                                    .expect("profile overlay active");
                                st.save();
                                let name = st.name.clone();
                                let msg = st
                                    .status
                                    .clone()
                                    .unwrap_or_else(|| format!("  ✓ Profile '{}' created", name));
                                app.push(ChatLine::Info(msg));
                                app.profile_overlay = None;
                                super::commands::switch_profile_seamless(app, &name);
                            } else {
                                st.status = Some("  ✗ Select a model from the list".into());
                            }
                        }
                    }
                }
                Backspace => {
                    match step {
                        ProfileCreateStep::Provider => {
                            st.provider_picker.filter.pop();
                            st.provider_picker.clamp();
                        }
                        ProfileCreateStep::Model => {
                            if let Some(ref mut picker) = st.profile_model_picker {
                                picker.filter.pop();
                                picker.clamp();
                            }
                        }
                        _ => {
                            if !st.input.is_empty() {
                                if event.modifiers.contains(Km::CONTROL) {
                                    // Ctrl+Backspace: delete word
                                    while st.input_cursor > 0 {
                                        let b = char_byte_pos_tui(&st.input, st.input_cursor - 1);
                                        let ch = st.input[..b].chars().last();
                                        if ch.map(|c| c == ' ').unwrap_or(false)
                                            && st.input_cursor > 1
                                        {
                                            break;
                                        }
                                        st.input_cursor -= 1;
                                        let pos = char_byte_pos_tui(&st.input, st.input_cursor);
                                        st.input.remove(pos);
                                    }
                                } else {
                                    delete_char_before_cursor_pe(st);
                                }
                            }
                        }
                    }
                }
                Delete => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        delete_char_at_cursor_pe(st);
                    }
                },
                Left => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        if st.input_cursor > 0 {
                            st.input_cursor -= 1;
                        }
                    }
                },
                Right => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        if st.input_cursor < st.input.chars().count() {
                            st.input_cursor += 1;
                        }
                    }
                },
                Home => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        st.input_cursor = 0;
                    }
                },
                End => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        st.input_cursor = st.input.chars().count();
                    }
                },
                Up => match step {
                    ProfileCreateStep::Provider => {
                        if st.provider_picker.selected > 0 {
                            st.provider_picker.selected -= 1;
                            if st.provider_picker.selected < st.provider_picker.scroll_offset {
                                st.provider_picker.scroll_offset = st.provider_picker.selected;
                            }
                        }
                    }
                    ProfileCreateStep::Model => {
                        if let Some(ref mut picker) = st.profile_model_picker {
                            if picker.selected > 0 {
                                picker.selected -= 1;
                                if picker.selected < picker.scroll_offset {
                                    picker.scroll_offset = picker.selected;
                                }
                            }
                        }
                    }
                    _ => {
                        st.input_cursor = 0;
                    }
                },
                Down => match step {
                    ProfileCreateStep::Provider => {
                        let vis = crossterm::terminal::size()
                            .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                            .unwrap_or(20);
                        let n = st.provider_picker.filtered().len();
                        if n > 0 && st.provider_picker.selected + 1 < n {
                            st.provider_picker.selected += 1;
                            if st.provider_picker.selected >= st.provider_picker.scroll_offset + vis
                            {
                                st.provider_picker.scroll_offset =
                                    st.provider_picker.selected + 1 - vis;
                            }
                        }
                    }
                    ProfileCreateStep::Model => {
                        let vis = crossterm::terminal::size()
                            .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                            .unwrap_or(20);
                        if let Some(ref mut picker) = st.profile_model_picker {
                            let n = picker.filtered().len();
                            if n > 0 && picker.selected + 1 < n {
                                picker.selected += 1;
                                if picker.selected >= picker.scroll_offset + vis {
                                    picker.scroll_offset = picker.selected + 1 - vis;
                                }
                            }
                        }
                    }
                    _ => {
                        st.input_cursor = st.input.chars().count();
                    }
                },
                Char(c) => match step {
                    ProfileCreateStep::Provider => {
                        st.provider_picker.filter.push(c);
                        st.provider_picker.clamp();
                    }
                    ProfileCreateStep::Model => {
                        if let Some(ref mut picker) = st.profile_model_picker {
                            picker.filter.push(c);
                            picker.clamp();
                        }
                    }
                    ProfileCreateStep::ApiKey if c == 'k' && !st.saved_keys.is_empty() => {
                        st.mode = ProfileOverlayMode::PickingSavedKey { selected: 0 };
                        st.status = None;
                    }
                    _ => {
                        let b = char_byte_pos_tui(&st.input, st.input_cursor);
                        st.input.insert(b, c);
                        st.input_cursor += 1;
                    }
                },
                _ => {}
            }
        }

        // ── PickingProvider: structured provider picker ─────────────────────
        ProfileOverlayMode::PickingProvider { .. } => match event.code {
            Esc => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                st.provider_picker = ProviderPickerState::new();
                st.mode = ProfileOverlayMode::Overview;
            }
            Enter => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                st.commit_provider_pick();
                st.save();
                let name = st.name.clone();
                let provider = st.provider.clone();
                let model = st.model.clone();
                if app.current_profile == name {
                    app.provider = provider;
                    app.model = model;
                }
            }
            Up => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                if st.provider_picker.selected > 0 {
                    st.provider_picker.selected -= 1;
                    if st.provider_picker.selected < st.provider_picker.scroll_offset {
                        st.provider_picker.scroll_offset = st.provider_picker.selected;
                    }
                }
            }
            Down => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                let n = st.provider_picker.filtered().len();
                if n > 0 && st.provider_picker.selected + 1 < n {
                    st.provider_picker.selected += 1;
                    let vis = crossterm::terminal::size()
                        .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                        .unwrap_or(20);
                    if st.provider_picker.selected >= st.provider_picker.scroll_offset + vis {
                        st.provider_picker.scroll_offset = st.provider_picker.selected + 1 - vis;
                    }
                }
            }
            Char(c) => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                st.provider_picker.filter.push(c);
                st.provider_picker.clamp();
            }
            Backspace => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                st.provider_picker.filter.pop();
                st.provider_picker.clamp();
            }
            _ => {}
        },

        // ── PickingModel: structured model picker ────────────────────────────
        ProfileOverlayMode::PickingModel { .. } => match event.code {
            Esc => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                st.profile_model_picker = None;
                st.mode = ProfileOverlayMode::Overview;
            }
            Enter => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                st.commit_model_pick();
                st.save();
                let name = st.name.clone();
                let provider = st.provider.clone();
                let model = st.model.clone();
                if app.current_profile == name {
                    app.provider = provider;
                    app.model = model;
                }
            }
            Up => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                if let Some(ref mut picker) = st.profile_model_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                        if picker.selected < picker.scroll_offset {
                            picker.scroll_offset = picker.selected;
                        }
                    }
                }
            }
            Down => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                if let Some(ref mut picker) = st.profile_model_picker {
                    let vis = crossterm::terminal::size()
                        .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                        .unwrap_or(20);
                    let n = picker.filtered().len();
                    if n > 0 && picker.selected + 1 < n {
                        picker.selected += 1;
                        if picker.selected >= picker.scroll_offset + vis {
                            picker.scroll_offset = picker.selected + 1 - vis;
                        }
                    }
                }
            }
            Char(c) => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                if let Some(ref mut picker) = st.profile_model_picker {
                    picker.filter.push(c);
                    picker.clamp();
                }
            }
            Backspace => {
                let st = app
                    .profile_overlay
                    .as_mut()
                    .expect("profile overlay active");
                if let Some(ref mut picker) = st.profile_model_picker {
                    picker.filter.pop();
                    picker.clamp();
                }
            }
            _ => {}
        },

        // ── PickingSavedKey: choose a saved API key ─────────────────────────
        ProfileOverlayMode::PickingSavedKey { selected } => {
            match event.code {
                Esc => {
                    st.mode =
                        ProfileOverlayMode::Creating { step: ProfileCreateStep::ApiKey };
                    st.status = None;
                }
                Enter => {
                    // Select the currently highlighted saved key
                    let idx = *selected;
                    if idx < st.saved_keys.len() {
                        let offer = &st.saved_keys[idx];
                        let all_profiles = clido_core::load_config(&app.workspace_root)
                            .map(|c| c.profiles)
                            .unwrap_or_default();
                        if let Some(src_entry) = all_profiles.get(&offer.source_profile) {
                            let real_key =
                                crate::setup::read_credential(&st.config_path, &offer.provider_id)
                                    .or_else(|| {
                                        src_entry.api_key_env.as_ref().and_then(|e| {
                                            std::env::var(e).ok()
                                        })
                                    })
                                    .or_else(|| src_entry.api_key.clone())
                                    .unwrap_or_default();
                            if !real_key.is_empty() {
                                st.api_key = real_key;
                                // Trigger a live model fetch for this provider + key
                                spawn_model_fetch(
                                    st.provider.clone(),
                                    st.api_key.clone(),
                                    if st.base_url.is_empty() {
                                        None
                                    } else {
                                        Some(st.base_url.clone())
                                    },
                                    app.channels.fetch_tx.clone(),
                                );
                                app.models_loading = true;
                                st.mode = ProfileOverlayMode::Creating {
                                    step: ProfileCreateStep::Model,
                                };
                                st.status = None;
                            } else {
                                st.status =
                                    Some("  ✗ Could not retrieve saved key".into());
                            }
                        } else {
                            st.status =
                                Some("  ✗ Source profile not found".into());
                        }
                    }
                }
                Up => {
                    if *selected > 0 {
                        if let ProfileOverlayMode::PickingSavedKey { selected: s } = &mut st.mode {
                            *s -= 1;
                        }
                    }
                }
                Down => {
                    if *selected + 1 < st.saved_keys.len() {
                        if let ProfileOverlayMode::PickingSavedKey { selected: s } = &mut st.mode {
                            *s += 1;
                        }
                    }
                }
                _ => {}
            }
        }

        // ── Overview: navigate fields ──────────────────────────────────────
        ProfileOverlayMode::Overview => {
            match event.code {
                Esc => {
                    app.profile_overlay = None;
                }
                Up => {
                    if st.cursor > 0 {
                        st.cursor -= 1;
                    }
                }
                Down => {
                    if st.cursor + 1 < ProfileOverlayState::field_count() {
                        st.cursor += 1;
                    }
                }
                Enter => {
                    let models: Vec<ModelEntry> = app.known_models.clone();
                    app.profile_overlay
                        .as_mut()
                        .expect("profile overlay active")
                        .begin_edit(&models);
                }
                KeyCode::Char('s') if event.modifiers.contains(Km::CONTROL) => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    st.save();
                    // Update live app state if it's the active profile
                    let name = st.name.clone();
                    let new_provider = st.provider.clone();
                    let new_model = st.model.clone();
                    let new_api_key = st.api_key.clone();
                    if app.current_profile == name {
                        let provider_changed = app.provider != new_provider;
                        let key_changed = app.api_key != new_api_key;
                        app.provider = new_provider;
                        app.model = new_model.clone();
                        app.api_key = new_api_key;
                        if provider_changed || key_changed {
                            app.profile_overlay = None;
                            super::commands::reload_active_profile_in_agent(app, &name);
                            app.push(ChatLine::Info(
                                "  ↻ Profile updated — agent reloaded with new credentials".into(),
                            ));
                        } else {
                            // Only model changed — live-switch
                            let _ = app.channels.model_switch_tx.send(new_model);
                        }
                    }
                }
                _ => {}
            }
        }

        // ── EditField: typing into a single field ──────────────────────────
        ProfileOverlayMode::EditField(_) => {
            match event.code {
                Esc => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    st.cancel_edit();
                }
                Enter => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    st.commit_edit();
                    // Auto-save on field commit
                    st.save();
                    let name = st.name.clone();
                    let new_provider = st.provider.clone();
                    let new_model = st.model.clone();
                    let new_api_key = st.api_key.clone();
                    if app.current_profile == name {
                        let provider_changed = app.provider != new_provider;
                        let key_changed = app.api_key != new_api_key;
                        app.provider = new_provider;
                        app.model = new_model.clone();
                        app.api_key = new_api_key;
                        if provider_changed || key_changed {
                            app.profile_overlay = None;
                            super::commands::reload_active_profile_in_agent(app, &name);
                            app.push(ChatLine::Info(
                                "  ↻ Profile updated — agent reloaded with new credentials".into(),
                            ));
                        } else {
                            let _ = app.channels.model_switch_tx.send(new_model);
                        }
                    }
                }
                Backspace => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    delete_char_before_cursor_pe(st);
                }
                Delete => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    delete_char_at_cursor_pe(st);
                }
                Left => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    if st.input_cursor > 0 {
                        st.input_cursor -= 1;
                    }
                }
                Right => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    if st.input_cursor < st.input.chars().count() {
                        st.input_cursor += 1;
                    }
                }
                Home => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    st.input_cursor = 0;
                }
                End => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    st.input_cursor = st.input.chars().count();
                }
                Char(c) => {
                    let st = app
                        .profile_overlay
                        .as_mut()
                        .expect("profile overlay active");
                    let b = char_byte_pos_tui(&st.input, st.input_cursor);
                    st.input.insert(b, c);
                    st.input_cursor += 1;
                }
                _ => {}
            }
        }
    }
}

/// Delete the character before the cursor in `ProfileOverlayState.input`.
pub(super) fn delete_char_before_cursor_pe(st: &mut ProfileOverlayState) {
    if st.input_cursor == 0 || st.input.is_empty() {
        return;
    }
    st.input_cursor -= 1;
    let b = char_byte_pos_tui(&st.input, st.input_cursor);
    st.input.remove(b);
}

/// Delete the character at the cursor in `ProfileOverlayState.input`.
pub(super) fn delete_char_at_cursor_pe(st: &mut ProfileOverlayState) {
    if st.input_cursor >= st.input.chars().count() {
        return;
    }
    let b = char_byte_pos_tui(&st.input, st.input_cursor);
    st.input.remove(b);
}

/// char_byte_pos for ProfileOverlayState (same logic as the TUI char_byte_pos helper).
pub(super) fn char_byte_pos_tui(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

// ── Overlay stack action handler ──────────────────────────────────────────────

/// Process app-level actions returned by overlays.
pub(super) fn handle_app_action(app: &mut App, action: AppAction) {
    match action {
        AppAction::SwitchModel { model_id, save } => {
            let _ = app.channels.model_switch_tx.send(model_id.clone());
            app.model = model_id;
            if save {
                // persist to config
            }
        }
        AppAction::SwitchProfile { profile_name } => {
            super::commands::switch_profile_seamless(app, &profile_name);
        }
        AppAction::ResumeSession { session_id } => {
            let _ = app.channels.resume_tx.send(session_id);
        }
        AppAction::GrantPermission(_grant) => {
            // TODO: wire when permission overlay is migrated
        }
        AppAction::ShowError(msg) => {
            app.overlay_stack
                .push(OverlayKind::Error(ErrorOverlay::new(msg)));
        }
        AppAction::RunCommand(cmd) => {
            execute_slash(app, &cmd);
        }
        AppAction::Quit => {
            app.quit = true;
        }
    }
}

// ── Input handling ────────────────────────────────────────────────────────────

pub(super) fn handle_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    // Ctrl+C / Ctrl+D always quits
    if matches!(
        (event.modifiers, event.code),
        (Km::CONTROL, Char('c')) | (Km::CONTROL, Char('d'))
    ) {
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
                        Color::Red,
                        std::time::Duration::from_secs(3),
                    );
                } else {
                    app.push_toast(
                        "Copied to clipboard",
                        Color::Green,
                        std::time::Duration::from_secs(2),
                    );
                }
            }
            None => app.push_toast(
                "Nothing to copy yet",
                Color::Yellow,
                std::time::Duration::from_secs(2),
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
                app.push(ChatLine::Info(format!("  ✓ Allowed access to: {}", path.display())));
                app.pending_path_permission = None;
                return;
            }
            Char('n') | Char('N') | Esc => {
                // Deny - send empty path to signal denial
                let _ = app.channels.path_permission_tx.send(std::path::PathBuf::new());
                app.push(ChatLine::Info(format!("  ✗ Denied access to: {}", path.display())));
                app.pending_path_permission = None;
                return;
            }
            Char('a') | Char('A') => {
                // Always allow - add to allowed paths for this session
                app.allowed_external_paths.push(path.clone());
                let _ = app.channels.allowed_paths_tx.send(app.allowed_external_paths.clone());
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
        // Ctrl+S: save selected model as default in config
        if event.modifiers == Km::CONTROL {
            if let KeyCode::Char('s') = event.code {
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
            KeyCode::Char('f') | KeyCode::Char('F') => {
                if let Some(picker) = &mut app.model_picker {
                    let filtered = picker.filtered();
                    if !filtered.is_empty() {
                        let model_id = filtered[picker.selected].id.clone();
                        drop(filtered);
                        app.model_prefs.toggle_favorite(&model_id);
                        app.model_prefs.save();
                        // Rebuild known_models with updated favorites.
                        let (pricing, _) = clido_core::load_pricing();
                        app.known_models = build_model_list(&pricing, &app.model_prefs);
                        picker.models = app.known_models.clone();
                        picker.clamp();
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
            Char('d') | Char('D') => {
                if let Some(picker) = &mut app.session_picker {
                    if let Some(orig_idx) = picker.picker.selected_original_index() {
                        let sid = picker.picker.items()[orig_idx].session_id.clone();
                        if app.current_session_id.as_deref() == Some(&sid) {
                            app.push(ChatLine::Info("  Cannot delete the active session".into()));
                        } else if clido_storage::delete_session(&app.workspace_root, &sid).is_ok() {
                            picker.picker.items_mut().remove(orig_idx);
                            picker.picker.apply_filter();
                            if picker.picker.items().is_empty() {
                                app.session_picker = None;
                            }
                        }
                    }
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
            KeyCode::Char('n') => {
                app.profile_picker = None;
                let config_path = clido_core::global_config_path()
                    .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                let all_profiles = clido_core::load_config(&app.workspace_root)
                    .map(|c| c.profiles)
                    .unwrap_or_default();
                app.profile_overlay = Some(ProfileOverlayState::for_create(config_path, &all_profiles));
            }
            KeyCode::Char('e') => {
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
    let completions = slash_completions(&app.text_input.text);
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
                                    Color::Green,
                                    std::time::Duration::from_secs(2),
                                );
                            }
                            Err(e) => {
                                app.push_toast(
                                    format!("Copy failed: {}", e),
                                    Color::Red,
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
                let max_row = app.rendered_line_texts.len().saturating_sub(1);
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
                let max_row = app.rendered_line_texts.len().saturating_sub(1);
                let line = app
                    .rendered_line_texts
                    .get(row.min(max_row))
                    .map(|l| l.as_str())
                    .unwrap_or("");
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
                let max_row = app.rendered_line_texts.len().saturating_sub(1);
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
            } else if app.editing_queued_item.is_some() {
                // Discard the queue item being edited (don't put it back)
                app.editing_queued_item = None;
                app.queue_nav_idx = None;
                app.text_input.text = app.text_input.history_draft.clone();
                app.text_input.cursor = 0;
                app.selected_cmd = None;
            } else {
                app.text_input.text.clear();
                app.text_input.cursor = 0;
                app.selected_cmd = None;
                app.text_input.history_idx = None;
            }
        }
        // Ctrl+Enter: interrupt current run and send immediately.
        (Km::CONTROL, Enter) => app.force_send(),
        // Alt+Enter: also interrupt (for terminals where Ctrl+Enter doesn't work)
        (Km::ALT, Enter) => app.force_send(),
        // Ctrl+Shift+C: enter copy mode for selecting chat lines.
        (Km::CONTROL | Km::SHIFT, Char('c')) => {
            app.selection_mode = !app.selection_mode;
            if app.selection_mode {
                app.selection.clear();
                let max_row = app.rendered_line_texts.len().saturating_sub(1);
                let start_row = (app.scroll as usize).min(max_row);
                app.selection.start(start_row, 0);
                app.push_toast(
                    "Copy mode: ↑↓←→ move · Space toggle · y copy · Esc exit".to_string(),
                    Color::Yellow,
                    std::time::Duration::from_secs(5),
                );
            }
        }
        // Shift+Enter or Ctrl+J: insert a newline without sending (multiline input).
        (Km::SHIFT, Enter) | (Km::CONTROL, Char('j')) => {
            let byte_pos = char_byte_pos(&app.text_input.text, app.text_input.cursor);
            app.text_input.text.insert(byte_pos, '\n');
            app.text_input.cursor += 1;
            app.selected_cmd = None;
            app.text_input.history_idx = None;
        }
        (_, Enter) => app.submit(),
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

                // If we're already editing a queue item, put current input back in queue first
                if app.editing_queued_item.is_some() {
                    if !app.text_input.text.trim().is_empty() {
                        app.queued.push_back(app.text_input.text.clone());
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
                    app.text_input.cursor = 0; // Reset cursor to start
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
                // Reset cursor to start of prompt for better UX
                app.text_input.cursor = 0;
                app.selected_cmd = None;
            }
        }
        // ── Down: queue nav (put current back in queue) OR multiline cursor movement OR input history ─
        (_, Down)
            if app.pending_perm.is_none() && slash_completions(&app.text_input.text).is_empty() =>
        {
            // Handle queue navigation (moving forward through queue)
            // Put current input back in queue before moving
            if app.queue_nav_idx.is_some() {
                let queue_len = app.queued.len();
                // Put current text back in queue (if not empty)
                if !app.text_input.text.trim().is_empty() {
                    app.queued.push_back(app.text_input.text.clone());
                }
                app.editing_queued_item = None;

                // Now move to next item
                let idx = app.queue_nav_idx.unwrap();
                if idx >= queue_len {
                    // Exhausted queue, return to draft and switch to history
                    app.queue_nav_idx = None;
                    app.text_input.text = app.text_input.history_draft.clone();
                    app.text_input.cursor = 0;
                    app.selected_cmd = None;
                } else if let Some(item) = app.queued.remove(idx) {
                    // Get next item from queue
                    app.editing_queued_item = Some(item.clone());
                    app.text_input.text = item;
                    app.text_input.cursor = 0;
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
                    app.text_input.cursor = 0;
                    app.selected_cmd = None;
                } else {
                    let new_idx = i + 1;
                    app.text_input.history_idx = Some(new_idx);
                    app.text_input.text = app.text_input.history[new_idx].clone();
                    app.text_input.cursor = 0;
                    app.selected_cmd = None;
                }
            }
        }
        // ── Chat scroll (PageUp/PageDown — larger jumps) ─────────────────────
        (_, PageUp) => {
            scroll_up(app, 3);
        }
        (_, PageDown) => {
            scroll_down(app, 3);
        }
        (Km::CONTROL, Char('u')) => {
            app.text_input.text.clear();
            app.text_input.cursor = 0;
            app.selected_cmd = None;
            app.text_input.history_idx = None;
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
