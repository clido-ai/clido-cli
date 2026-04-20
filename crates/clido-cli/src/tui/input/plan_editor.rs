use super::*;

// ── Plan text editor key handling (nano-style) ───────────────────────────────

pub fn handle_plan_text_editor_key(app: &mut App, event: crossterm::event::KeyEvent) {
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

// ── Plan editor key handling ──────────────────────────────────────────────────

pub fn handle_plan_editor_key(app: &mut App, event: crossterm::event::KeyEvent) {
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
                app.plan.pending_delete = None;
            }
        }
        Down => {
            if app.plan.selected_task + 1 < task_count {
                app.plan.selected_task += 1;
                app.plan.pending_delete = None;
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
            // Delete selected task — requires pressing `d` twice to confirm.
            if let Some(ref mut editor) = app.plan.editor {
                // If there's a pending delete and it matches the current selection, execute it.
                if let Some((pending_idx, _)) = app.plan.pending_delete {
                    if pending_idx == app.plan.selected_task {
                        if let Some(task) = editor.plan.tasks.get(app.plan.selected_task) {
                            let id = task.id.clone();
                            let desc = task.description.clone();
                            if editor.delete_task(&id).is_ok()
                                && app.plan.selected_task >= editor.plan.tasks.len()
                                && app.plan.selected_task > 0
                            {
                                app.plan.selected_task -= 1;
                            }
                            app.plan.pending_delete = None;
                            app.push_toast(
                                format!("Deleted: {}", desc),
                                TUI_STATE_OK,
                                std::time::Duration::from_secs(2),
                            );
                        }
                        return;
                    }
                }
                // First press: set pending delete and show confirmation toast.
                if let Some(task) = editor.plan.tasks.get(app.plan.selected_task) {
                    let desc = task.description.clone();
                    app.plan.pending_delete = Some((app.plan.selected_task, desc.clone()));
                    app.push_toast(
                        format!("Delete '{}'? Press d again to confirm", desc),
                        TUI_STATE_WARN,
                        std::time::Duration::from_secs(3),
                    );
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
            app.agent_run_state = super::app_state::AppRunState::Idle;
        }
        _ => {}
    }
}

// ── Profile overlay keyboard handler ─────────────────────────────────────────
