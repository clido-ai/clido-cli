//! Right-hand **status rail** (IDE-style): session and repo context first, then agent/queue, then
//! task strip and tool activity (sections that grow are lower). Shown when the terminal is wide
//! enough; narrow terminals keep the stacked strips.

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::tui::app_state::AppRunState;
use crate::tui::state::{PlanPanelVisibility, StatusRailVisibility};
use crate::tui::*;

use super::plan::{build_plan_todo_strip_lines, gather_plan_panel_steps};
use super::widgets::{status_strip_lines, truncate_chars};
use super::SPINNER;

/// Minimum terminal width (columns) to show the status rail beside the transcript (`/panel auto`).
pub(crate) const STATUS_RAIL_MIN_TERM_WIDTH: u16 = 118;
/// Lower threshold when `/panel on` — show the rail a bit earlier than auto.
pub(crate) const STATUS_RAIL_MIN_TERM_WIDTH_ON: u16 = 108;

/// Whether the layout should allocate the right-hand status rail for this terminal width.
pub(crate) fn status_rail_wanted(vis: StatusRailVisibility, term_width: u16) -> bool {
    match vis {
        StatusRailVisibility::Off => false,
        StatusRailVisibility::Auto => term_width >= STATUS_RAIL_MIN_TERM_WIDTH,
        StatusRailVisibility::On => term_width >= STATUS_RAIL_MIN_TERM_WIDTH_ON,
    }
}
/// Minimum rail width (columns); [`status_rail_width`] may grow this on wider terminals.
pub(crate) const STATUS_RAIL_WIDTH_MIN: u16 = 28;
/// Upper cap so the rail does not dominate ultra-wide layouts.
pub(crate) const STATUS_RAIL_WIDTH_MAX: u16 = 48;

/// Pick rail width from the horizontal slice under the header (chat+rail), before the split.
pub(crate) fn status_rail_width(mid_area_width: u16) -> u16 {
    let pct = (mid_area_width as u32 * 26 / 100)
        .clamp(STATUS_RAIL_WIDTH_MIN as u32, STATUS_RAIL_WIDTH_MAX as u32) as u16;
    pct.min(mid_area_width.saturating_sub(52))
        .max(STATUS_RAIL_WIDTH_MIN)
}

fn rail_section_title(name: &'static str) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("▎{name}"),
        Style::default()
            .fg(TUI_SOFT_ACCENT)
            .add_modifier(Modifier::BOLD | Modifier::DIM),
    )])
}

fn shorten_workdir(path: &std::path::Path) -> String {
    let raw = path.display().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && raw.starts_with(&home) {
        format!("~{}", &raw[home.len()..])
    } else {
        raw
    }
}

/// All lines for the rail (before vertical scroll).
pub(crate) fn build_status_rail_lines(
    app: &App,
    inner_w: u16,
    spinner: &str,
) -> Vec<Line<'static>> {
    let w = inner_w.max(8);
    let dim = Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // ── SESSION (compact; header still has full model/profile) ──────────────
    lines.push(rail_section_title("SESSION"));
    if let Some(ref sid) = app.current_session_id {
        let short = sid[..sid.len().min(8)].to_string();
        let turns = if app.stats.session_turn_count > 0 {
            format!(" · {} turns", app.stats.session_turn_count)
        } else {
            String::new()
        };
        lines.push(Line::from(vec![
            Span::styled(" #", Style::default().fg(TUI_MARK)),
            Span::styled(short, Style::default().fg(TUI_BRAND_TEXT)),
            Span::styled(turns, dim),
        ]));
        if let Some(ref title) = app.session_title {
            lines.push(Line::from(vec![Span::styled(
                format!(" {}", truncate_chars(title, w.saturating_sub(2) as usize)),
                dim,
            )]));
        }
    } else {
        lines.push(Line::from(vec![Span::styled(" (no session)", dim)]));
    }

    // ── CONTEXT ────────────────────────────────────────────────────────────
    lines.push(Line::raw(""));
    lines.push(rail_section_title("CONTEXT"));
    // Workdir first
    let wd = shorten_workdir(&app.workspace_root);
    lines.push(Line::from(vec![Span::styled(
        format!(" {}", truncate_chars(&wd, w.saturating_sub(2) as usize)),
        dim,
    )]));
    // Branch below workdir
    if let Some(ref git) = app.tui_git_snapshot {
        let dirty = if git.status_short.is_empty() {
            "clean"
        } else {
            "dirty"
        };
        lines.push(Line::from(vec![
            Span::styled(" ⎇ ", Style::default().fg(TUI_MARK)),
            Span::styled(
                truncate_chars(&git.branch, w.saturating_sub(8) as usize),
                Style::default().fg(TUI_MUTED),
            ),
            Span::styled(format!(" · {dirty}"), dim),
        ]));
    } else {
        lines.push(Line::from(vec![Span::styled(" (not a git repo)", dim)]));
    }

    // ── AGENT ───────────────────────────────────────────────────────────────
    lines.push(Line::raw(""));
    lines.push(rail_section_title("AGENT"));
    let agent_line = if app.pending_perm.is_some() {
        Line::from(vec![
            Span::styled(" ⏸ ", Style::default().fg(TUI_STATE_WARN)),
            Span::styled("Waiting — approve in dialog", dim),
        ])
    } else if app.rate_limit_resume_at.is_some() && !app.rate_limit_cancelled {
        Line::from(vec![
            Span::styled(" ⏳ ", Style::default().fg(TUI_STATE_WARN)),
            Span::styled("Rate limited — see input dock", dim),
        ])
    } else if app.enhancing {
        Line::from(vec![
            Span::styled(" ✦ ", Style::default().fg(TUI_SOFT_ACCENT)),
            Span::styled("Enhancing prompt", dim),
        ])
    } else if app.busy {
        let phase = match app.agent_run_state {
            AppRunState::RunningTools => "Running tools",
            AppRunState::Generating => "Generating",
            AppRunState::Idle => "Agent running",
        };
        Line::from(vec![
            Span::styled(
                format!(" {} ", spinner),
                Style::default().fg(TUI_STATE_BUSY),
            ),
            Span::styled(phase, dim),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ● ", Style::default().fg(TUI_STATE_OK)),
            Span::styled("Idle", dim),
        ])
    };
    lines.push(agent_line);
    if let Some(ref step) = app.current_step {
        lines.push(Line::from(vec![
            Span::styled(" › ", Style::default().fg(TUI_SOFT_ACCENT)),
            Span::styled(
                truncate_chars(step, w.saturating_sub(4) as usize),
                Style::default().fg(TUI_TEXT),
            ),
        ]));
    }

    // ── QUEUE ───────────────────────────────────────────────────────────────
    lines.push(Line::raw(""));
    lines.push(rail_section_title("QUEUE"));
    if app.queued.is_empty() && app.current_step.is_none() {
        lines.push(Line::from(vec![Span::styled(" —", dim)]));
    } else {
        if !app.queued.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                format!(" {} pending", app.queued.len()),
                Style::default()
                    .fg(TUI_STATE_WARN)
                    .add_modifier(Modifier::DIM),
            )]));
            for (i, item) in app.queued.iter().take(6).enumerate() {
                let first = item.lines().next().unwrap_or(item.as_str());
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        " {:>1}. {}",
                        i + 1,
                        truncate_chars(first, w.saturating_sub(5) as usize)
                    ),
                    dim,
                )]));
            }
            if app.queued.len() > 6 {
                lines.push(Line::from(vec![Span::styled(
                    format!(" …+{}", app.queued.len() - 6),
                    dim,
                )]));
            }
        } else if app.current_step.is_some() {
            lines.push(Line::from(vec![Span::styled(
                " (agent busy — see AGENT)",
                dim,
            )]));
        }
    }

    // ── TASK (todos / planner / harness; often many lines) ─────────────────
    lines.push(Line::raw(""));
    lines.push(rail_section_title("TASK"));
    let plan_steps = gather_plan_panel_steps(app);
    if matches!(app.plan_panel_visibility, PlanPanelVisibility::Off) && !app.harness_mode {
        lines.push(Line::from(vec![Span::styled(" Strip off", dim)]));
        lines.push(Line::from(vec![Span::styled(" /tasks on", dim)]));
    } else {
        lines.extend(build_plan_todo_strip_lines(
            app,
            &plan_steps,
            w,
            10,
            false,
            0,
        ));
    }

    // ── TOOLS (activity; grows with tool calls) ────────────────────────────
    lines.push(Line::raw(""));
    lines.push(rail_section_title("TOOLS"));
    if app.status_log.is_empty() {
        lines.push(Line::from(vec![Span::styled(" —", dim)]));
    } else {
        // Cap rows so SESSION/AGENT stay visible without excessive scrolling.
        lines.extend(status_strip_lines(&app.status_log, w, spinner, Some(14)));
    }

    lines
}

/// Paint the status rail into `area` (full cell rect including left border).
pub(crate) fn render_status_rail(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .style(Style::default().bg(TUI_SURFACE_RAIL))
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(TUI_DIVIDER));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let spinner = SPINNER[app.spinner_tick];
    let lines = build_status_rail_lines(app, inner.width, spinner);
    let total = lines.len();
    let h = inner.height as usize;

    // Reserve one row for a scroll hint when content does not fit (IDE-style position readout).
    let (content_h, show_footer) = if h <= 1 {
        (h, false)
    } else if total > h {
        (h - 1, true)
    } else {
        (h, false)
    };

    let visible = content_h;
    let max_scroll = total.saturating_sub(visible);
    app.layout.status_panel_max_scroll = max_scroll as u16;
    app.status_panel_scroll = app
        .status_panel_scroll
        .min(app.layout.status_panel_max_scroll);

    let start = app.status_panel_scroll as usize;
    let end = (start + visible).min(total);
    let mut chunk: Vec<Line<'static>> = if start < end {
        lines[start..end].to_vec()
    } else {
        vec![]
    };
    while chunk.len() < visible {
        chunk.push(Line::raw(""));
    }

    let footer_style = Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM);
    let [body_rect, footer_rect] = Layout::vertical([
        Constraint::Length(content_h as u16),
        Constraint::Length(if show_footer { 1 } else { 0 }),
    ])
    .areas(inner);

    frame.render_widget(Paragraph::new(chunk), body_rect);

    if show_footer && max_scroll > 0 {
        let start_ln = start + 1;
        let end_ln = (start + visible).min(total).max(start_ln);
        let hint = Line::from(vec![Span::styled(
            format!("{start_ln}–{end_ln}/{total} · Alt+Pg"),
            footer_style,
        )]);
        frame.render_widget(
            Paragraph::new(hint).alignment(Alignment::Right),
            footer_rect,
        );
    }
}
