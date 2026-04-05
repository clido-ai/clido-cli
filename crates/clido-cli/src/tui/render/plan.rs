use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use clido_planner::{Complexity, Plan, TaskStatus};

use crate::tui::*;

use super::widgets::truncate_chars;

pub(crate) fn render_plan_editor(frame: &mut Frame, app: &App, area: Rect) {
    let editor = match &app.plan.editor {
        Some(e) => e,
        None => return,
    };

    frame.render_widget(Clear, area);

    let plan = &editor.plan;
    let task_count = plan.tasks.len();
    let done_count = plan
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();
    let complexity_summary = if plan.tasks.iter().any(|t| t.complexity == Complexity::High) {
        "high"
    } else if plan
        .tasks
        .iter()
        .any(|t| t.complexity == Complexity::Medium)
    {
        "medium"
    } else {
        "low"
    };

    let title = format!(
        " Plan: {}  ({} tasks · complexity: {}) ",
        truncate_chars(&plan.meta.goal, 40),
        task_count,
        complexity_summary
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner: task list | help bar at bottom
    let [task_area, hint_area] =
        ratatui::layout::Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(inner);

    // ── Task list ──
    let mut task_lines: Vec<Line<'static>> = Vec::new();

    if let Some(ref form) = app.plan.task_editing {
        // Inline edit form for the selected task
        task_lines.push(Line::from(vec![Span::styled(
            "  Edit task",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]));
        task_lines.push(Line::raw(""));

        let desc_style = if form.focused_field == TaskEditField::Description {
            Style::default()
                .fg(TUI_TEXT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let notes_style = if form.focused_field == TaskEditField::Notes {
            Style::default()
                .fg(TUI_TEXT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let comp_style = if form.focused_field == TaskEditField::Complexity {
            Style::default()
                .fg(TUI_TEXT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        task_lines.push(Line::from(vec![
            Span::styled("  Description: ", desc_style),
            Span::styled(format!("[{}]", form.description), desc_style),
        ]));
        task_lines.push(Line::from(vec![
            Span::styled("  Notes:        ", notes_style),
            Span::styled(format!("[{}]", form.notes), notes_style),
        ]));

        let (low_style, med_style, high_style) = match form.complexity {
            Complexity::Low => (
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::DarkGray),
            ),
            Complexity::Medium => (
                Style::default().fg(Color::DarkGray),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::DarkGray),
            ),
            Complexity::High => (
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        };
        task_lines.push(Line::from(vec![
            Span::styled("  Complexity:  ", comp_style),
            Span::styled(" ● low ", low_style),
            Span::styled(" ● medium ", med_style),
            Span::styled(" ● high ", high_style),
        ]));
        task_lines.push(Line::raw(""));
        task_lines.push(Line::from(vec![Span::styled(
            "  Tab=next field  Enter=save  Esc=cancel",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));
    } else {
        // Task list with selection highlight
        let scroll_start = if app.plan.selected_task >= task_area.height as usize {
            app.plan.selected_task - task_area.height as usize + 1
        } else {
            0
        };

        for (i, task) in plan.tasks.iter().enumerate() {
            if i < scroll_start {
                continue;
            }
            let selected = i == app.plan.selected_task;
            let bg = if selected {
                TUI_SELECTION_BG
            } else {
                Color::Reset
            };
            let fg = if selected { Color::White } else { Color::Gray };

            let status_icon = match task.status {
                TaskStatus::Pending => "○",
                TaskStatus::Running => "↻",
                TaskStatus::Done => "✓",
                TaskStatus::Failed => "✗",
                TaskStatus::Skipped => "⊘",
            };

            let complexity_badge = match task.complexity {
                Complexity::Low => {
                    Span::styled(" [low] ", Style::default().fg(Color::DarkGray).bg(bg))
                }
                Complexity::Medium => {
                    Span::styled(" [med] ", Style::default().fg(Color::Yellow).bg(bg))
                }
                Complexity::High => Span::styled(" [high]", Style::default().fg(Color::Red).bg(bg)),
            };

            let skip_str = if task.skip { "⊘ " } else { "  " };
            let deps_str = if task.depends_on.is_empty() {
                String::new()
            } else {
                format!("  →{}", task.depends_on.join(","))
            };
            let marker = if selected { "▶" } else { " " };

            task_lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} {} {}", marker, status_icon, skip_str),
                    Style::default().fg(fg).bg(bg),
                ),
                Span::styled(
                    format!("{:<5}", task.id),
                    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
                ),
                complexity_badge,
                Span::styled(
                    format!("  {}{}", task.description, deps_str),
                    Style::default().fg(fg).bg(bg),
                ),
            ]));
        }

        let progress = format!("  Progress: {}/{}", done_count, task_count);
        task_lines.push(Line::raw(""));
        task_lines.push(Line::from(vec![Span::styled(
            progress,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));
    }

    frame.render_widget(
        Paragraph::new(task_lines).wrap(Wrap { trim: false }),
        task_area,
    );

    // ── Hint bar ──
    let dry_run_note = if app.plan_dry_run {
        "  [dry-run: x will not execute]"
    } else {
        ""
    };
    let hint = if app.plan.task_editing.is_some() {
        String::new()
    } else {
        format!(
            "  Enter=edit  d=delete  n=new  Space=skip  ↑↓=move  s=save  x=execute  Esc=abort{}",
            dry_run_note
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            hint,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )])),
        hint_area,
    );
}

// ── Plan text editor (nano-style) ────────────────────────────────────────────

pub(crate) fn render_plan_text_editor(frame: &mut Frame, app: &App, area: Rect) {
    let ed = match &app.plan.text_editor {
        Some(e) => e,
        None => return,
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Plan text (Ctrl+S = save · Esc / Ctrl+C = discard) ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [edit_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let visible_rows = edit_area.height as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, line) in ed.lines.iter().enumerate().skip(ed.scroll) {
        if lines.len() >= visible_rows {
            break;
        }
        if i == ed.cursor_row {
            // Render cursor inline
            let chars: Vec<char> = line.chars().collect();
            let col = ed.cursor_col.min(chars.len());
            let before: String = chars[..col].iter().collect();
            let cursor_ch: String = if col < chars.len() {
                chars[col].to_string()
            } else {
                " ".to_string()
            };
            let after: String = if col < chars.len() {
                chars[col + 1..].iter().collect()
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::raw(before),
                Span::styled(
                    cursor_ch,
                    Style::default().bg(Color::White).fg(Color::Black),
                ),
                Span::raw(after),
            ]));
        } else {
            lines.push(Line::raw(line.clone()));
        }
    }

    frame.render_widget(Paragraph::new(lines), edit_area);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("  ↑↓←→", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " navigate  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Enter", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " new line  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Ctrl+S", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " save  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Esc", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " discard  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Ctrl+C", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " discard",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
    ]));
    frame.render_widget(hint, hint_area);
}
/// Build a deterministic plan snapshot from assistant text.
/// This is the canonical path used for both saving and display.
pub(crate) fn build_plan_from_assistant_text(text: &str) -> Option<Plan> {
    let mut tasks = parse_plan_from_text(text);
    if tasks.is_empty() {
        // Deterministic fallback: every non-empty line becomes one step in order.
        tasks = text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
    }
    if tasks.is_empty() {
        return None;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let goal = tasks.first().cloned().unwrap_or_default();
    let slug: String = goal
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .take(30)
        .collect::<String>()
        .trim()
        .replace(' ', "_")
        .to_lowercase();
    Some(clido_planner::Plan {
        meta: clido_planner::PlanMeta {
            id: format!("{}_{}", slug, ts),
            goal,
            created_at: ts.to_string(),
        },
        tasks: tasks
            .iter()
            .enumerate()
            .map(|(i, t)| clido_planner::TaskNode {
                id: format!("{}", i + 1),
                description: t.clone(),
                status: clido_planner::TaskStatus::Pending,
                depends_on: vec![],
                complexity: clido_planner::Complexity::Medium,
                notes: String::new(),
                tools: None,
                skip: false,
            })
            .collect(),
    })
}

pub(crate) fn build_plan_from_tasks(tasks: &[String]) -> Option<Plan> {
    if tasks.is_empty() {
        return None;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let goal = tasks.first().cloned().unwrap_or_default();
    let slug: String = goal
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .take(30)
        .collect::<String>()
        .trim()
        .replace(' ', "_")
        .to_lowercase();
    Some(clido_planner::Plan {
        meta: clido_planner::PlanMeta {
            id: format!("{}_{}", slug, ts),
            goal,
            created_at: ts.to_string(),
        },
        tasks: tasks
            .iter()
            .enumerate()
            .map(|(i, t)| clido_planner::TaskNode {
                id: format!("{}", i + 1),
                description: t.clone(),
                status: clido_planner::TaskStatus::Pending,
                depends_on: vec![],
                complexity: clido_planner::Complexity::Medium,
                notes: String::new(),
                tools: None,
                skip: false,
            })
            .collect(),
    })
}

/// Strip leading markdown noise so plan lines like `**Step 1:**` match.
pub(crate) fn strip_plan_line_prefix(line: &str) -> String {
    let mut t = line.trim();
    loop {
        let before = t;
        t = t.trim_start_matches(['*', '#', '_', '>', '`']);
        t = t.trim_start();
        if t == before {
            break;
        }
    }
    t.to_string()
}

/// Truncate a string to at most `max_chars` characters, appending `…` if cut.
/// Parse a numbered step list out of free-form agent text.
/// Matches top-level step lines only — not sub-bullets or indented items.
/// Supported formats (at start of line, not indented):
///   "1. foo"  "1) foo"  "Step 1: foo"  "Step 1. foo"
pub(crate) fn parse_plan_from_text(text: &str) -> Vec<String> {
    let mut tasks = Vec::new();
    for line in text.lines() {
        // Skip indented lines — they are sub-bullets, not steps
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let trimmed = strip_plan_line_prefix(line);
        if trimmed.is_empty() {
            continue;
        }

        // "Step N: text" or "Step N. text"
        let step_prefix = trimmed
            .strip_prefix("Step ")
            .or_else(|| trimmed.strip_prefix("step "));
        if let Some(rest) = step_prefix {
            // consume digits
            let after_digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Some(content) = after_digits
                .strip_prefix(": ")
                .or_else(|| after_digits.strip_prefix(". "))
                .or_else(|| after_digits.strip_prefix(":**"))
                .or_else(|| after_digits.strip_prefix(':'))
            {
                let content = strip_plan_line_prefix(content);
                if !content.is_empty() {
                    tasks.push(content.to_string());
                }
                continue;
            }
        }

        // "N. text" or "N) text" or "N.text"
        if let Some(digit_end) = trimmed.find(|c: char| !c.is_ascii_digit()) {
            if digit_end > 0 {
                let rest = trimmed[digit_end..].trim_start();
                let content = rest
                    .strip_prefix(". ")
                    .or_else(|| rest.strip_prefix(") "))
                    .or_else(|| rest.strip_prefix('.'))
                    .or_else(|| rest.strip_prefix(')'))
                    .map(str::trim);
                if let Some(content) = content {
                    if !content.is_empty() {
                        tasks.push(content.to_string());
                    }
                }
            }
        }
    }
    tasks
}

/// Scan text for a "Step N: ..." line and return the full step label if found.
pub(crate) fn extract_current_step_full(text: &str) -> Option<(usize, String)> {
    for line in text.lines() {
        let t = strip_plan_line_prefix(line);
        // Match "Step N: ..." or "▶ Step N: ..."
        let rest = t.strip_prefix("▶ ").unwrap_or(t.as_str());
        if let Some(after) = rest
            .strip_prefix("Step ")
            .or_else(|| rest.strip_prefix("step "))
        {
            let after_digits = after.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Some(label) = after_digits
                .strip_prefix(": ")
                .or_else(|| after_digits.strip_prefix(". "))
                .or_else(|| after_digits.strip_prefix(":**"))
                .or_else(|| after_digits.strip_prefix(':'))
            {
                let label = label.trim();
                if !label.is_empty() {
                    let n: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(num) = n.parse::<usize>() {
                        return Some((num, format!("Step {}: {}", n, label)));
                    }
                }
            }
        }
    }
    None
}

/// Render the workflow YAML text editor (nano-style, full-screen overlay).
/// Reuses the same visual structure as `render_plan_text_editor`.
pub(crate) fn render_workflow_editor(frame: &mut Frame, app: &App, area: Rect) {
    let ed = match &app.workflow_editor {
        Some(e) => e,
        None => return,
    };

    frame.render_widget(Clear, area);

    let title = if let Some(ref p) = app.workflow_editor_path {
        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("workflow");
        format!(" Workflow: {name} (Ctrl+S = save · Esc = discard) ")
    } else {
        " New workflow (Ctrl+S = validate & save · Esc = discard) ".to_string()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [edit_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let visible_rows = edit_area.height as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, line) in ed.lines.iter().enumerate().skip(ed.scroll) {
        if lines.len() >= visible_rows {
            break;
        }
        if i == ed.cursor_row {
            let chars: Vec<char> = line.chars().collect();
            let col = ed.cursor_col.min(chars.len());
            let before: String = chars[..col].iter().collect();
            let cursor_ch: String = if col < chars.len() {
                chars[col].to_string()
            } else {
                " ".to_string()
            };
            let after: String = if col < chars.len() {
                chars[col + 1..].iter().collect()
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::raw(before),
                Span::styled(
                    cursor_ch,
                    Style::default().bg(Color::White).fg(Color::Black),
                ),
                Span::raw(after),
            ]));
        } else {
            lines.push(Line::raw(line.clone()));
        }
    }

    frame.render_widget(Paragraph::new(lines), edit_area);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("  ↑↓←→", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " navigate  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Enter", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " new line  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Ctrl+S", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " validate & save  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Esc", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " discard",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
    ]));
    frame.render_widget(hint, hint_area);
}
