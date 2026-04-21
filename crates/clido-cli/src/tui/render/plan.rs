use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use clido_harness::{read_state, reconcile_order, TaskPassState};
use clido_planner::{Complexity, Plan, TaskStatus};
use clido_tools::TodoStatus;

use crate::tui::state::PlanPanelVisibility;
use crate::tui::*;

use super::widgets::truncate_chars;

// ── Task strip (main layout, above status): todos, planner snapshot, harness, live step ──

/// One row in the plan/todo panel (todos, planner snapshot, or status hint).
#[derive(Debug, Clone)]
pub(crate) struct PlanPanelStep {
    pub status: PlanPanelStepStatus,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanPanelStepStatus {
    Pending,
    Active,
    Completed,
    Blocked,
}

fn gather_harness_panel_steps(workspace_root: &std::path::Path) -> Vec<PlanPanelStep> {
    let Ok(mut state) = read_state(workspace_root) else {
        return vec![PlanPanelStep {
            status: PlanPanelStepStatus::Blocked,
            text: "Harness: cannot read .clido/harness/tasks.json".to_string(),
        }];
    };
    reconcile_order(&mut state);
    if state.tasks.is_empty() {
        return vec![PlanPanelStep {
            status: PlanPanelStepStatus::Pending,
            text: "Harness: no tasks — use HarnessControl planner_append_tasks".to_string(),
        }];
    }
    let focus = state.meta.current_focus_task_id.clone();
    let mut out = Vec::new();
    for id in &state.task_order {
        let Some(t) = state.tasks.iter().find(|x| x.id == *id) else {
            continue;
        };
        let st = match t.status {
            TaskPassState::Pass => PlanPanelStepStatus::Completed,
            TaskPassState::Fail => {
                if focus.as_deref() == Some(t.id.as_str()) {
                    PlanPanelStepStatus::Active
                } else {
                    PlanPanelStepStatus::Pending
                }
            }
        };
        out.push(PlanPanelStep {
            status: st,
            text: format!("{} — {}", t.id, t.description),
        });
    }
    out
}

/// Collect steps to show: harness tasks (when harness mode), else live `TodoWrite`, planner snapshot, fallbacks.
pub(crate) fn gather_plan_panel_steps(app: &App) -> Vec<PlanPanelStep> {
    // If a workflow is running, show its steps in the panel.
    if let Some(ref wf) = app.active_workflow {
        if !wf.steps.is_empty() {
            return wf
                .steps
                .iter()
                .map(|s| PlanPanelStep {
                    status: match s.status {
                        crate::tui::app_state::WorkflowStepStatus::Pending => {
                            PlanPanelStepStatus::Pending
                        }
                        crate::tui::app_state::WorkflowStepStatus::Active => {
                            PlanPanelStepStatus::Active
                        }
                        crate::tui::app_state::WorkflowStepStatus::Done => {
                            PlanPanelStepStatus::Completed
                        }
                        crate::tui::app_state::WorkflowStepStatus::Failed => {
                            PlanPanelStepStatus::Blocked
                        }
                        crate::tui::app_state::WorkflowStepStatus::Skipped => {
                            PlanPanelStepStatus::Completed
                        }
                    },
                    text: s.name.clone(),
                })
                .collect();
        }
    }
    if app.harness_mode {
        return gather_harness_panel_steps(&app.workspace_root);
    }
    if let Ok(todos) = app.todo_store.try_lock() {
        if !todos.is_empty() {
            return todos
                .iter()
                .map(|t| PlanPanelStep {
                    status: match t.status {
                        TodoStatus::Pending => PlanPanelStepStatus::Pending,
                        TodoStatus::InProgress => PlanPanelStepStatus::Active,
                        TodoStatus::Done => PlanPanelStepStatus::Completed,
                        TodoStatus::Blocked => PlanPanelStepStatus::Blocked,
                    },
                    text: t.content.clone(),
                })
                .collect();
        }
    }

    if let Some(ref plan) = app.plan.last_plan_snapshot {
        if !plan.tasks.is_empty() {
            return plan
                .tasks
                .iter()
                .map(|t| PlanPanelStep {
                    status: match t.status {
                        TaskStatus::Pending => PlanPanelStepStatus::Pending,
                        TaskStatus::Running => PlanPanelStepStatus::Active,
                        TaskStatus::Done => PlanPanelStepStatus::Completed,
                        TaskStatus::Failed => PlanPanelStepStatus::Blocked,
                        TaskStatus::Skipped => PlanPanelStepStatus::Completed,
                    },
                    text: t.description.clone(),
                })
                .collect();
        }
    }

    if let Some(ref tasks) = app.plan.last_plan {
        if !tasks.is_empty() {
            return tasks
                .iter()
                .map(|s| PlanPanelStep {
                    status: PlanPanelStepStatus::Pending,
                    text: s.clone(),
                })
                .collect();
        }
    }

    if app.plan.awaiting_plan_response {
        return vec![PlanPanelStep {
            status: PlanPanelStepStatus::Active,
            text: "Waiting for structured plan…".to_string(),
        }];
    }

    if app.busy {
        if let Some(ref s) = app.current_step {
            return vec![PlanPanelStep {
                status: PlanPanelStepStatus::Active,
                text: s.clone(),
            }];
        }
        // Fallback: show that agent is working even if no step was extracted.
        // Use a spinner character so the panel feels alive.
        let spinner = match app.spinner_tick % 4 {
            0 => "⠋",
            1 => "⠙",
            2 => "⠹",
            _ => "⠸",
        };
        return vec![PlanPanelStep {
            status: PlanPanelStepStatus::Active,
            text: format!("{spinner} Working..."),
        }];
    }

    Vec::new()
}

fn plan_panel_content_row_count(step_count: usize) -> u16 {
    const MAX_STEP_LINES: usize = 8;
    if step_count == 0 {
        return 0;
    }
    if step_count <= MAX_STEP_LINES {
        step_count as u16
    } else {
        // Reserve one row for "+N more".
        MAX_STEP_LINES as u16
    }
}

/// Vertical space for the progress strip (0 = hidden).
/// Always shows at least 2 rows when the agent is busy in Auto mode,
/// so the user sees "⠋ Working..." and knows progress is happening.
pub(crate) fn plan_panel_height_for_layout(
    vis: PlanPanelVisibility,
    term_w: u16,
    term_h: u16,
    steps: &[PlanPanelStep],
    harness_mode: bool,
    busy: bool,
) -> u16 {
    if matches!(vis, PlanPanelVisibility::Off) {
        return 0;
    }

    /// Need enough width for gutter + marker + reasonable text.
    const MIN_W: u16 = 52;
    /// Auto: only on larger terminals so chat + input stay usable.
    const MIN_TERM_H_AUTO: u16 = 28;
    /// On: hide only when the terminal is too short to be usable.
    const MIN_TERM_H_ON: u16 = 18;
    const HEADER_ROWS: u16 = 1;
    /// When visibility is On and there is nothing to list, still reserve this many body rows
    /// so the strip stays visibly present (user asked to always see the panel when toggled on).
    const ON_EMPTY_BODY_ROWS: u16 = 2;

    if term_w < MIN_W {
        return 0;
    }

    let empty = steps.is_empty();
    let body_rows = plan_panel_content_row_count(steps.len());

    match vis {
        PlanPanelVisibility::Off => 0,
        PlanPanelVisibility::Auto => {
            let min_term_h = if harness_mode {
                MIN_TERM_H_AUTO.saturating_sub(4).max(20)
            } else {
                MIN_TERM_H_AUTO
            };
            if term_h < min_term_h {
                return 0;
            }
            // Always show when busy — even if empty, show "⠋ Working..."
            if busy {
                HEADER_ROWS + body_rows.max(1)
            } else if empty {
                0
            } else {
                HEADER_ROWS + body_rows
            }
        }
        PlanPanelVisibility::On => {
            if term_h < MIN_TERM_H_ON {
                return 0;
            }
            if empty {
                HEADER_ROWS + ON_EMPTY_BODY_ROWS
            } else {
                HEADER_ROWS + body_rows
            }
        }
    }
}

/// Build wrapped lines for the progress strip (`plan_h` rows from [`plan_panel_height_for_layout`]).
/// `max_step_lines` caps listed steps (stacked layout uses 8; status rail uses 8).
/// When `with_strip_title_row` is false (status rail), omit the "Tasks · auto" title row — the rail
/// draws its own section header.
pub(crate) fn build_plan_todo_strip_lines(
    app: &App,
    steps: &[PlanPanelStep],
    width: u16,
    max_step_lines: usize,
    with_strip_title_row: bool,
    scroll_offset: u16,
) -> Vec<Line<'static>> {
    let w = width as usize;
    let dim = Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM);
    let header_note = match app.plan_panel_visibility {
        PlanPanelVisibility::Auto => "auto",
        PlanPanelVisibility::On => "on",
        PlanPanelVisibility::Off => "off",
    };
    let header_title = if app.harness_mode { "Harness" } else { "Tasks" };
    let mut out: Vec<Line<'static>> = Vec::new();
    if with_strip_title_row {
        out.push(Line::from(vec![
            Span::styled(
                format!("{TUI_GUTTER}{header_title}"),
                dim.add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  ·  {header_note}"), dim),
        ]));
    }

    if steps.is_empty() {
        if app.harness_mode {
            out.push(Line::from(vec![Span::styled(
                format!("{TUI_GUTTER}No harness tasks yet"),
                dim,
            )]));
            out.push(Line::from(vec![Span::styled(
                format!(
                    "{TUI_GUTTER}.clido/harness/tasks.json  ·  HarnessControl planner_append_tasks"
                ),
                dim,
            )]));
        } else if matches!(app.plan_panel_visibility, PlanPanelVisibility::On) {
            out.push(Line::from(vec![Span::styled(
                format!("{TUI_GUTTER}Nothing listed yet"),
                dim,
            )]));
            out.push(Line::from(vec![Span::styled(
                format!("{TUI_GUTTER}Todos, planner steps, or live agent step"),
                dim,
            )]));
            out.push(Line::from(vec![Span::styled(
                format!("{TUI_GUTTER}/plan <task>  ·  /tasks on|off|auto"),
                dim,
            )]));
        } else {
            out.push(Line::from(vec![Span::styled(
                format!("{TUI_GUTTER}No rows"),
                dim,
            )]));
            out.push(Line::from(vec![Span::styled(
                format!("{TUI_GUTTER}/plan <task>  ·  or agent todos"),
                dim,
            )]));
        }
        return out;
    }

    // Apply scroll offset: skip first `scroll_offset` steps.
    let skip = scroll_offset as usize;
    let scrolled = if skip < steps.len() {
        &steps[skip..]
    } else {
        &[]
    };

    let cap = max_step_lines.max(1);
    let show_more = scrolled.len() > cap;
    let take = if show_more {
        cap - 1
    } else {
        scrolled.len().min(cap)
    };

    let prefix_cols = 4usize;
    let text_budget = w.saturating_sub(prefix_cols).max(12);

    for step in scrolled.iter().take(take) {
        let (icon, icon_style, text_style) = match step.status {
            PlanPanelStepStatus::Pending => (
                "○",
                Style::default().fg(TUI_MUTED),
                Style::default().fg(TUI_ROW_DIM),
            ),
            PlanPanelStepStatus::Active => (
                "›",
                Style::default()
                    .fg(TUI_SOFT_ACCENT)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(TUI_TEXT),
            ),
            PlanPanelStepStatus::Completed => (
                "✓",
                Style::default().fg(TUI_STATE_OK),
                Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
            ),
            PlanPanelStepStatus::Blocked => (
                "!",
                Style::default().fg(TUI_STATE_WARN),
                Style::default().fg(TUI_STATE_WARN),
            ),
        };
        let truncated = truncate_chars(&step.text, text_budget);
        out.push(Line::from(vec![
            Span::styled(format!("{TUI_GUTTER}{icon} "), icon_style),
            Span::styled(truncated, text_style),
        ]));
    }

    if show_more {
        let visible_left = scrolled.len().saturating_sub(take);
        let total_more = visible_left + skip;
        out.push(Line::from(vec![Span::styled(
            format!("{TUI_GUTTER}…  +{total_more} more"),
            dim,
        )]));
    }

    out
}

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
                .fg(TUI_SOFT_ACCENT)
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
                .fg(TUI_STATE_WARN)
                .add_modifier(Modifier::BOLD),
        )]));
        task_lines.push(Line::raw(""));

        let desc_style = if form.focused_field == TaskEditField::Description {
            Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TUI_MUTED)
        };
        let notes_style = if form.focused_field == TaskEditField::Notes {
            Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TUI_MUTED)
        };
        let comp_style = if form.focused_field == TaskEditField::Complexity {
            Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TUI_MUTED)
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
                    .fg(TUI_STATE_OK)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(TUI_MUTED),
                Style::default().fg(TUI_MUTED),
            ),
            Complexity::Medium => (
                Style::default().fg(TUI_MUTED),
                Style::default()
                    .fg(TUI_STATE_WARN)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(TUI_MUTED),
            ),
            Complexity::High => (
                Style::default().fg(TUI_MUTED),
                Style::default().fg(TUI_MUTED),
                Style::default()
                    .fg(TUI_STATE_ERR)
                    .add_modifier(Modifier::BOLD),
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
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
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
            let fg = if selected { TUI_TEXT } else { TUI_ROW_DIM };

            let status_icon = match task.status {
                TaskStatus::Pending => "○",
                TaskStatus::Running => "↻",
                TaskStatus::Done => "✓",
                TaskStatus::Failed => "✗",
                TaskStatus::Skipped => "⊘",
            };

            let complexity_badge = match task.complexity {
                Complexity::Low => Span::styled(" [low] ", Style::default().fg(TUI_MUTED).bg(bg)),
                Complexity::Medium => {
                    Span::styled(" [med] ", Style::default().fg(TUI_STATE_WARN).bg(bg))
                }
                Complexity::High => {
                    Span::styled(" [high]", Style::default().fg(TUI_STATE_ERR).bg(bg))
                }
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
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
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
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
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
                .fg(TUI_SOFT_ACCENT)
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
                    Style::default()
                        .bg(TUI_SELECTION_BG)
                        .fg(TUI_BRAND_TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(after),
            ]));
        } else {
            lines.push(Line::raw(line.clone()));
        }
    }

    frame.render_widget(Paragraph::new(lines), edit_area);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("  ↑↓←→", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " navigate  ",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
        Span::styled("Enter", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " new line  ",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
        Span::styled("Ctrl+S", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " save  ",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
        Span::styled("Esc", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " discard  ",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
        Span::styled("Ctrl+C", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " discard",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
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
                .fg(TUI_SOFT_ACCENT)
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
                    Style::default()
                        .bg(TUI_SELECTION_BG)
                        .fg(TUI_BRAND_TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(after),
            ]));
        } else {
            lines.push(Line::raw(line.clone()));
        }
    }

    frame.render_widget(Paragraph::new(lines), edit_area);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("  ↑↓←→", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " navigate  ",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
        Span::styled("Enter", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " new line  ",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
        Span::styled("Ctrl+S", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " validate & save  ",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
        Span::styled("Esc", Style::default().fg(TUI_MUTED)),
        Span::styled(
            " discard",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
    ]));
    frame.render_widget(hint, hint_area);
}

#[cfg(test)]
mod plan_panel_tests {
    use super::*;

    fn sample_steps(n: usize) -> Vec<PlanPanelStep> {
        (0..n)
            .map(|i| PlanPanelStep {
                status: PlanPanelStepStatus::Pending,
                text: format!("step {i}"),
            })
            .collect()
    }

    #[test]
    fn panel_off_always_zero_height() {
        let steps = sample_steps(3);
        assert_eq!(
            plan_panel_height_for_layout(PlanPanelVisibility::Off, 80, 40, &steps, false, false),
            0
        );
    }

    #[test]
    fn auto_requires_tall_terminal_and_content() {
        let steps = sample_steps(1);
        assert_eq!(
            plan_panel_height_for_layout(PlanPanelVisibility::Auto, 80, 27, &steps, false, false),
            0
        );
        assert!(
            plan_panel_height_for_layout(PlanPanelVisibility::Auto, 80, 28, &steps, false, false)
                > 0
        );
        assert_eq!(
            plan_panel_height_for_layout(PlanPanelVisibility::Auto, 80, 40, &[], false, false),
            0
        );
    }

    #[test]
    fn on_shows_placeholder_when_empty() {
        assert_eq!(
            plan_panel_height_for_layout(PlanPanelVisibility::On, 80, 22, &[], false, false),
            3
        );
        assert_eq!(
            plan_panel_height_for_layout(PlanPanelVisibility::On, 80, 17, &[], false, false),
            0
        );
    }

    #[test]
    fn narrow_terminal_hides_panel() {
        let steps = sample_steps(2);
        assert_eq!(
            plan_panel_height_for_layout(PlanPanelVisibility::On, 50, 30, &steps, false, false),
            0
        );
    }

    #[test]
    fn busy_shows_panel_even_when_empty_in_auto_mode() {
        // In Auto mode with no steps, the panel is normally hidden.
        // But when busy, it should show at least "⠋ Working...".
        assert_eq!(
            plan_panel_height_for_layout(PlanPanelVisibility::Auto, 80, 30, &[], false, false),
            0
        );
        assert!(
            plan_panel_height_for_layout(PlanPanelVisibility::Auto, 80, 30, &[], false, true) > 0,
            "panel should show when busy in Auto mode"
        );
    }

    #[test]
    fn caps_at_eight_steps_in_auto_mode() {
        // With 15 steps, panel should show only 8 + header = 9 rows.
        let steps = sample_steps(15);
        let h = plan_panel_height_for_layout(
            PlanPanelVisibility::Auto,
            80,
            40,
            &steps,
            false,
            false,
        );
        assert_eq!(h, 9); // 1 header + 8 steps
    }
}
