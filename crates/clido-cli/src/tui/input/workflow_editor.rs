use super::*;

// ── Workflow editor key handling (nano-style, reuses PlanTextEditor) ──────────

pub fn handle_workflow_editor_key(app: &mut App, event: crossterm::event::KeyEvent) {
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
                        // Determine save path (global)
                        let save_dir = clido_core::default_workflows_directory();
                        let save_dir = std::path::PathBuf::from(&save_dir);
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
                        // Re-open editor preserving cursor position so user can fix.
                        let (cursor_row, cursor_col, scroll) =
                            (ed.cursor_row, ed.cursor_col, ed.scroll);
                        let mut new_ed = PlanTextEditor::from_raw(&yaml_text);
                        new_ed.cursor_row = cursor_row;
                        new_ed.cursor_col = cursor_col;
                        new_ed.scroll = scroll;
                        new_ed.clamp_col();
                        app.workflow_editor = Some(new_ed);
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

    // Scroll to keep cursor visible — compute actual editor height from terminal dimensions.
    if let Some(ed) = &mut app.workflow_editor {
        let editor_height = crossterm::terminal::size()
            .map(|(_, h)| (h.saturating_sub(10)) as usize)
            .unwrap_or(20)
            .max(1);
        if ed.cursor_row < ed.scroll {
            ed.scroll = ed.cursor_row;
        } else if ed.cursor_row >= ed.scroll + editor_height {
            ed.scroll = ed.cursor_row.saturating_sub(editor_height - 1);
        }
    }
}
