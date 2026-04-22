mod diff;
pub(crate) mod plan;
mod profile;
mod status_panel;
mod surfaces;
mod welcome;
mod widgets;

#[allow(unused_imports)]
use crate::tui::config::*;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;

pub(super) use plan::*;
pub(super) use profile::*;
pub(super) use welcome::*;
pub(super) use widgets::*;

use diff::render_diff;

use pulldown_cmark::Parser;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::overlay::OverlayKind;
use crate::tui::app_state::AppRunState;
use crate::tui::state::{ContentLine, LineSource};

use super::*;

// ── Chat input (multiline scroll/cursor) ────────────────────────────────────────

/// Build visible lines for a multiline input and the cursor position inside the widget.
/// `input_visible_w` matches single-line horizontal budget (inner width minus chrome).
fn multiline_input_paragraph(
    input: &crate::text_input::TextInput,
    max_visible_content_rows: usize,
    input_visible_w: usize,
) -> (Vec<Line<'static>>, u16, u16) {
    let text = &input.text;
    let cursor = input.cursor;
    let max_line_chars = input_visible_w.saturating_sub(1).max(1);

    let byte_at_cursor = char_byte_pos(text, cursor);
    let before_cursor = &text[..byte_at_cursor];
    let cursor_line_idx = before_cursor.matches('\n').count();
    let col_on_line = before_cursor
        .rfind('\n')
        .map(|p| text[p + 1..byte_at_cursor].chars().count())
        .unwrap_or_else(|| before_cursor.chars().count());

    let all_lines: Vec<&str> = text.split('\n').collect();

    let v_scroll = if cursor_line_idx >= max_visible_content_rows {
        cursor_line_idx - max_visible_content_rows + 1
    } else {
        0
    };
    let display_row = (cursor_line_idx - v_scroll) as u16;

    let h_skip = if col_on_line >= max_line_chars {
        col_on_line + 1 - max_line_chars
    } else {
        0
    };
    let col_display = col_on_line.saturating_sub(h_skip);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let last = v_scroll + max_visible_content_rows;
    for line_idx in v_scroll..last {
        let Some(line) = all_lines.get(line_idx).copied() else {
            break;
        };
        let slice: String = if line_idx == cursor_line_idx {
            line.chars().skip(h_skip).take(max_line_chars).collect()
        } else {
            line.chars().take(max_line_chars).collect()
        };
        lines.push(Line::raw(format!(" {}", slice)));
    }

    (lines, display_row, col_display as u16)
}

// ── Render ────────────────────────────────────────────────────────────────────

pub(super) fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // ── Plan text editor (nano-style) full-screen overlay ───────────────────
    if app.plan.text_editor.is_some() {
        render_plan_text_editor(frame, app, area);
        return;
    }

    // ── Workflow editor (nano-style) full-screen overlay ─────────────────────
    if app.workflow_editor.is_some() {
        render_workflow_editor(frame, app, area);
        return;
    }

    // ── Plan editor full-screen overlay ─────────────────────────────────────
    if app.plan.editor.is_some() {
        render_plan_editor(frame, app, area);
        return;
    }

    surfaces::paint_app_canvas(frame, area);

    // ── Header spans (built before layout so we can measure and pick height) ──
    let version = env!("CARGO_PKG_VERSION");
    let dim = Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM);

    // Separator tokens for consistent spacing throughout the header.
    let sep = "  │ "; // main section separator
    let dot = " · "; // lightweight inline separator

    // Line 1: brand · version  │  provider/model  │  profile  │  session  │  title
    let mut hline1: Vec<Span<'static>> = vec![
        Span::styled(
            "cli",
            Style::default()
                .fg(TUI_BRAND_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            ";",
            Style::default().fg(TUI_MARK).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "do",
            Style::default()
                .fg(TUI_BRAND_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    hline1.push(Span::raw(" "));
    hline1.push(Span::styled(format!("v{}", version), dim));

    if !app.provider.is_empty() || !app.model.is_empty() {
        let model_str = if app.per_turn_prev_model.is_some() {
            format!("{}{}{}⁺", app.provider, TUI_SEP, app.model)
        } else {
            format!("{}{}{}", app.provider, TUI_SEP, app.model)
        };
        hline1.push(Span::styled(sep, Style::default().fg(TUI_DIVIDER)));
        hline1.push(Span::styled(model_str, dim));
    }
    hline1.push(Span::styled(sep, Style::default().fg(TUI_DIVIDER)));
    hline1.push(Span::styled(
        app.current_profile.clone(),
        Style::default()
            .fg(TUI_SOFT_ACCENT)
            .add_modifier(Modifier::BOLD),
    ));
    // Session info with lightweight dot separator.
    if let Some(ref session_id) = app.current_session_id {
        let short_id = &session_id[..session_id.len().min(8)];
        hline1.push(Span::styled(
            format!("{dot}session #{short_id}"),
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ));
    }
    if let Some(ref title) = app.session_title {
        hline1.push(Span::styled(
            format!("{dot}{title}"),
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ));
    }
    if app.reviewer_configured {
        let reviewer_on = app.reviewer_enabled.load(Ordering::Relaxed);
        let (rdot, color) = if reviewer_on {
            ("●", TUI_STATE_OK)
        } else {
            ("○", TUI_MUTED)
        };
        hline1.push(Span::styled(sep, Style::default().fg(TUI_DIVIDER)));
        hline1.push(Span::styled(
            format!("reviewer {rdot}"),
            Style::default().fg(color).add_modifier(Modifier::DIM),
        ));
    }

    // Line 2: dir · cost/tokens
    let mut hline2: Vec<Span<'static>> = vec![Span::styled(
        {
            let home = std::env::var("HOME").unwrap_or_default();
            let raw = app.workspace_root.display().to_string();
            let short = if !home.is_empty() && raw.starts_with(&home) {
                format!("~{}", &raw[home.len()..])
            } else {
                raw
            };
            format!("{TUI_GUTTER}{short}")
        },
        dim,
    )];
    if app.stats.session_total_cost_usd > 0.0 || app.stats.session_total_input_tokens > 0 {
        // Format token count (combined in+out for this session)
        let sum_tokens =
            app.stats.session_total_input_tokens + app.stats.session_total_output_tokens;
        let tok_str = if sum_tokens >= 1_000_000 {
            format!("{:.2}M tok", sum_tokens as f64 / 1_000_000.0)
        } else if sum_tokens >= 1000 {
            format!("{:.1}k tok", sum_tokens as f64 / 1000.0)
        } else {
            format!("{} tok", sum_tokens)
        };

        // Context window usage — use last-turn input as proxy
        let ctx_str = if app.context_max_tokens > 0 && app.stats.session_input_tokens > 0 {
            let pct = (app.stats.session_input_tokens as f64 / app.context_max_tokens as f64
                * 100.0)
                .min(100.0);
            format!("{TUI_SEP}{:.0}% context", pct)
        } else {
            String::new()
        };

        let is_subscription = clido_providers::is_subscription_provider(&app.provider);

        if is_subscription {
            // Subscription providers: only show turns (token counts may be unreliable)
            let turn_str = if app.stats.session_turn_count > 0 {
                format!("{TUI_SEP}{} turns", app.stats.session_turn_count)
            } else {
                String::new()
            };
            hline2.push(Span::styled(format!("session: {turn_str}{ctx_str}"), dim));
        } else {
            // On-demand providers: show cost in USD
            let cost_str = if let Some(budget) = app.max_budget_usd {
                // 5 decimal places for precision on small amounts, cap at $999.
                let cost = app.stats.session_total_cost_usd;
                if cost >= 1.0 {
                    format!("${:.2} / ${:.2}", cost.min(999.0), budget)
                } else {
                    format!("${:.5} / ${:.2}", cost, budget)
                }
            } else {
                let cost = app.stats.session_total_cost_usd;
                if cost >= 1.0 {
                    format!("${:.2}", cost.min(999.0))
                } else {
                    format!("${:.5}", cost)
                }
            };
            hline2.push(Span::styled(
                format!("{TUI_SEP}session: {cost_str}  {tok_str}{ctx_str}"),
                dim,
            ));
        }
        if app.ui_emit_unhealthy.load(Ordering::Relaxed) {
            hline2.push(Span::styled(
                format!("{TUI_SEP}UI channel issue"),
                Color::Yellow,
            ));
        }
    } else if let Some(budget) = app.max_budget_usd {
        if !clido_providers::is_subscription_provider(&app.provider) {
            hline2.push(Span::styled(format!("{TUI_SEP}budget ${:.2}", budget), dim));
        }
    }

    // Decide header height: 1 line if everything fits side-by-side, else 2.
    // When the terminal is very narrow, use a single minimal header.
    let w = area.width as usize;
    let is_narrow = area.width < NARROW_WIDTH;
    let line1_w: usize = hline1.iter().map(|s| s.content.chars().count()).sum();
    let line2_w: usize = hline2.iter().map(|s| s.content.chars().count()).sum();
    let header_h: u16 = if is_narrow || line1_w + line2_w <= w {
        1
    } else {
        2
    };

    // Layout: wide terminals use a right **status rail** (IDE-style); narrow keeps stacked strips.
    // Input grows with content: 1 line of text = 3 rows (2 borders + 1), capped at 8.
    let input_line_count = app.text_input.text.matches('\n').count() + 1;
    let input_h = (input_line_count as u16 + 2).clamp(INPUT_MIN_HEIGHT, INPUT_MAX_HEIGHT);
    let (hint_h, status_h) = if area.width < STATUS_MIN_WIDTH {
        (0, 0)
    } else {
        (HINT_HEIGHT, STATUS_STRIP_HEIGHT)
    };
    let plan_steps = gather_plan_panel_steps(app);
    // Auto-scroll plan panel to show the latest tasks.
    // Must match the `max_step_lines` cap passed to `build_plan_todo_strip_lines`.
    let total_steps = plan_steps.len();
    if total_steps > PLAN_MAX_VISIBLE_STEPS {
        let target_scroll = (total_steps.saturating_sub(PLAN_MAX_VISIBLE_STEPS)) as u16;
        if app.plan_scroll < target_scroll {
            app.plan_scroll = target_scroll;
        }
    } else if app.plan_scroll > 0 {
        app.plan_scroll = 0;
    }
    let use_rail = status_panel::status_rail_wanted(app.status_rail_visibility, area.width);
    app.layout.status_rail_active = use_rail;

    // Plan panel height (used in stacked layout; always computed for auto-scroll).
    let stacked_plan_h: u16 = plan_panel_height_for_layout(
        app.plan_panel_visibility,
        area.width,
        area.height,
        &plan_steps,
        app.harness_mode,
        app.busy,
    );

    // Prompt banner: shows the active prompt (max 2 lines) below the header while busy.
    let banner_h: u16 = if !is_narrow {
        if let Some(ref prompt) = app.active_prompt {
            let first_line = prompt.lines().next().unwrap_or("");
            let available_w = area.width.saturating_sub(4) as usize;
            if first_line.chars().count() > available_w {
                2
            } else {
                1
            }
        } else {
            0
        }
    } else {
        0
    };

    let (
        header_area,
        banner_area,
        chat_area,
        plan_area,
        status_area,
        queue_area,
        hint_area,
        input_area,
        rail_area_opt,
    ) = if use_rail {
        let [h_area, banner_a, below_banner] = Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Length(banner_h),
            Constraint::Min(0),
        ])
        .areas(area);
        let [mid_area, hint_a, input_a] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(hint_h),
            Constraint::Length(input_h),
        ])
        .areas(below_banner);
        let rail_w = status_panel::status_rail_width(mid_area.width);
        let [chat_a, rail_a] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(rail_w)]).areas(mid_area);
        (
            h_area,
            banner_a,
            chat_a,
            Rect::default(),
            Rect::default(),
            Rect::default(),
            hint_a,
            input_a,
            Some(rail_a),
        )
    } else {
        let min_chat_h = 10u16;
        let reserved_h = header_h + banner_h + stacked_plan_h + status_h + hint_h + input_h + 2;
        let available_for_queue = area.height.saturating_sub(min_chat_h + reserved_h);
        let queue_h = if app.current_step.is_some() && !app.queued.is_empty() {
            let total = 1 + 1 + app.queued.len();
            total.min(available_for_queue.max(3) as usize) as u16
        } else if app.current_step.is_some() {
            1
        } else if !app.queued.is_empty() {
            let total = 1 + app.queued.len() + 1;
            total.min(available_for_queue.max(3) as usize) as u16
        } else {
            0
        };
        let [h_area, banner_a, ch_area, pl_area, st_area, qu_area, hi_area, inp_area] =
            Layout::vertical([
                Constraint::Length(header_h),
                Constraint::Length(banner_h),
                Constraint::Min(0),
                Constraint::Length(stacked_plan_h),
                Constraint::Length(status_h),
                Constraint::Length(queue_h),
                Constraint::Length(hint_h),
                Constraint::Length(input_h),
            ])
            .areas(area);
        (
            h_area, banner_a, ch_area, pl_area, st_area, qu_area, hi_area, inp_area, None,
        )
    };

    // ── Header render ──
    let header_para = if is_narrow {
        // Narrow terminal: session title takes priority, then session ID, then model.
        let mut spans = Vec::new();
        if let Some(ref t) = app.session_title {
            spans.push(Span::styled(
                truncate_chars(t, w),
                Style::default()
                    .fg(TUI_SOFT_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if let Some(ref id) = app.current_session_id {
            let short = &id[..id.len().min(8)];
            spans.push(Span::styled(
                format!("session #{short}"),
                Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
            ));
        } else {
            spans.push(Span::styled(
                truncate_chars(&app.model, w),
                Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
            ));
        }
        Paragraph::new(Line::from(spans))
    } else if header_h == 1 {
        // Everything on one line — append line2 to line1 and fit to width.
        hline1.extend(hline2);
        Paragraph::new(Line::from(fit_spans(hline1, w)))
    } else {
        // Two lines: fit each independently.
        let l1 = fit_spans(hline1, w);
        let l2 = fit_spans(hline2, w);
        Paragraph::new(vec![Line::from(l1), Line::from(l2)])
    };
    render_header_bar(
        frame,
        surfaces::header_zone_block(),
        header_para,
        header_area,
    );

    // ── Prompt banner ──
    // Shows the active prompt (max 2 lines) below the header while the agent is busy.
    if banner_h > 0 {
        if let Some(ref prompt) = app.active_prompt {
            let available_w = banner_area.width.saturating_sub(4) as usize;
            let first_line = prompt.lines().next().unwrap_or("").trim();
            let truncated = if first_line.chars().count() > available_w {
                let s: String = first_line
                    .chars()
                    .take(available_w.saturating_sub(1))
                    .collect();
                format!("{}…", s)
            } else {
                first_line.to_string()
            };
            let banner_line = Line::from(vec![
                Span::styled(
                    "▶ ",
                    Style::default()
                        .fg(TUI_STATE_BUSY)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    truncated,
                    Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                ),
            ]);
            frame.render_widget(Paragraph::new(banner_line), banner_area);
        }
    }

    // ── Chat ──
    // Store chat area bounds for mouse selection handlers.
    app.layout.chat_area_y = (chat_area.y, chat_area.y + chat_area.height);
    app.layout.chat_area_width = chat_area.width;

    {
        let cb = surfaces::content_zone_block();
        let chat_inner = cb.inner(chat_area);
        frame.render_widget(cb, chat_area);
        if is_welcome_only(app) {
            render_welcome(frame, app, chat_inner);
        } else {
            // Build pre-wrapped lines. wrap_content_lines guarantees every line
            // fits within inner_w columns, so we must NOT use Paragraph::wrap() —
            // that would apply a second, different wrapping pass whose line count
            // could diverge from wrapped_lines.len(), breaking scroll/selection math.
            let inner_w = chat_inner.width as usize;
            let lines = build_lines_w(app, inner_w);

            // Total height is exactly our wrapped line count — no ratatui recount.
            let total_height = app.wrapped_lines.len() as u32;
            let max_scroll = total_height.saturating_sub(chat_inner.height as u32);
            // Store for use in handle_key (Up/PageUp need the current max_scroll).
            app.layout.max_scroll = max_scroll;
            // If a resize just occurred, restore scroll to the saved ratio.
            if let Some(ratio) = app.pending_scroll_ratio.take() {
                app.scroll = ((ratio * max_scroll as f64).round() as u32).min(max_scroll);
            }
            let prev_max = app.chat_render_prev_max_scroll;
            app.chat_render_prev_max_scroll = max_scroll;
            if app.following && prev_max > 0 && max_scroll > prev_max.saturating_add(10) {
                app.suppress_next_chat_scroll_up = true;
            }
            // When not following and new content was added (max_scroll increased),
            // adjust scroll position so user stays at same relative position.
            if !app.following && prev_max > 0 && max_scroll > prev_max {
                let scroll_diff = max_scroll - prev_max;
                app.scroll = app.scroll.saturating_add(scroll_diff).min(max_scroll);
            }
            let scroll = if app.following {
                max_scroll
            } else {
                app.scroll.min(max_scroll)
            };
            // Keep app.scroll in [0, max_scroll] so that mouse-selection row
            // calculations (content_row = chat_row + app.scroll) and
            // apply_selection_highlight always get a valid offset.
            app.scroll = scroll;

            // Apply selection highlighting using the already-clamped scroll offset
            // so the highlight window is never out of sync with what's displayed.
            let lines = if app.selection.active {
                let visible_height = chat_inner.height as usize;
                apply_selection_highlight(&lines, &app.selection, scroll as usize, visible_height)
            } else {
                lines
            };

            // Render without Paragraph::wrap() — lines are already width-bounded.
            let para = Paragraph::new(lines);
            // ratatui's scroll() takes (u16, u16); clamp to u16::MAX before casting.
            frame.render_widget(
                para.scroll((scroll.min(u16::MAX as u32) as u16, 0)),
                chat_inner,
            );
        }
    }

    // ── Status rail (wide) or stacked progress / status / queue (narrow) ──
    if let Some(rail_area) = rail_area_opt {
        status_panel::render_status_rail(frame, app, rail_area);
    } else {
        // ── Task strip (above status; /tasks on|off|auto; hidden when /panel off uses this layout) ──
        if stacked_plan_h > 0 {
            let pb = surfaces::focus_lane_zone_block();
            let p_inner = pb.inner(plan_area);
            frame.render_widget(pb, plan_area);
            let plines = build_plan_todo_strip_lines(
                app,
                &plan_steps,
                p_inner.width,
                PLAN_MAX_VISIBLE_STEPS,
                true,
                app.plan_scroll,
            );
            frame.render_widget(Paragraph::new(plines), p_inner);
        }

        // ── Status strip ──
        if status_area.height > 0 {
            let sb = surfaces::status_zone_block();
            let s_inner = sb.inner(status_area);
            frame.render_widget(sb, status_area);
            let spinner = SPINNER[app.spinner_tick];
            let slines =
                status_strip_lines(&app.status_log, s_inner.width, spinner, TOOLS_CAP_STACKED);
            frame.render_widget(Paragraph::new(slines), s_inner);
        }

        // ── Queue strip ──
        // When the agent is thinking with queued items: show thinking step + queued list.
        // When idle with queued items: show queued list with count header.
        // When thinking alone: just the step text.
        if queue_area.height > 0 {
            let qb = surfaces::queue_zone_block();
            let q_inner = qb.inner(queue_area);

            let mut queue_lines: Vec<Line> = Vec::new();

            let has_thinking = app.current_step.is_some();
            let has_queue = !app.queued.is_empty();

            if has_thinking {
                queue_lines.push(Line::from(vec![
                    Span::styled(
                        format!("{TUI_GUTTER}▶ "),
                        Style::default()
                            .fg(TUI_SOFT_ACCENT)
                            .add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        truncate_chars(app.current_step.as_ref().unwrap(), 80),
                        Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                    ),
                ]));
            }

            if has_queue {
                if has_thinking {
                    // Agent is active — compact divider before queued items
                    queue_lines.push(Line::raw(""));
                    queue_lines.push(Line::from(Span::styled(
                        format!("{TUI_GUTTER_SUB}Queued — runs after current step"),
                        Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                    )));
                } else {
                    // Agent is idle — header with count
                    queue_lines.push(Line::from(Span::styled(
                        format!("{TUI_GUTTER_SUB}{} queued — sent next", app.queued.len()),
                        Style::default()
                            .fg(TUI_STATE_WARN)
                            .add_modifier(Modifier::DIM),
                    )));
                }

                // Each queued item on its own line — dynamic based on inner height (queue block top rule).
                let header_lines = if has_thinking { 2 } else { 1 };
                let max_items = (q_inner.height as usize).saturating_sub(header_lines);
                let effective_max = if max_items > 0 { max_items } else { 5 };

                for (idx, item) in app.queued.iter().enumerate().take(effective_max) {
                    let first_line = item.lines().next().unwrap_or(item.as_str());
                    let prefix = format!("{TUI_GUTTER_DEEP}{:>2}. ", idx + 1);
                    let prefix_len = prefix.chars().count();
                    let max_width = q_inner.width as usize;
                    let available = max_width.saturating_sub(prefix_len);
                    let truncated = if first_line.chars().count() > available {
                        format!(
                            "{}…",
                            first_line
                                .chars()
                                .take(available.saturating_sub(1))
                                .collect::<String>()
                        )
                    } else {
                        first_line.to_string()
                    };

                    queue_lines.push(Line::from(Span::styled(
                        format!("{}{}", prefix, truncated),
                        Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                    )));
                }

                let more_count = app.queued.len().saturating_sub(effective_max);
                if more_count > 0 {
                    queue_lines.push(Line::from(Span::styled(
                        format!("{TUI_GUTTER_SUB}… and {more_count} more"),
                        Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                    )));
                }
            }

            if queue_lines.is_empty() {
                queue_lines.push(Line::raw(""));
            }

            frame.render_widget(qb, queue_area);
            frame.render_widget(Paragraph::new(queue_lines), q_inner);
        }
    }

    // ── Input box (always rendered, even when permission popup is showing) ──
    let max_visible_content_rows = input_h.saturating_sub(2) as usize;
    let input_visible_w = (input_area.width as usize).saturating_sub(4).max(1);
    let is_multiline = app.text_input.text.contains('\n');

    let (input_para_lines, cursor_row, cursor_col) = if is_multiline {
        multiline_input_paragraph(&app.text_input, max_visible_content_rows, input_visible_w)
    } else {
        app.text_input.update_scroll(input_visible_w);
        let visible: String = app
            .text_input
            .text
            .chars()
            .skip(app.text_input.scroll)
            .take(input_visible_w)
            .collect();
        (
            vec![Line::raw(format!(" {}", visible))],
            0u16,
            (app.text_input.cursor - app.text_input.scroll) as u16,
        )
    };

    // Always clear the input area first — prevents any bleed-through from overlapping widgets.
    frame.render_widget(Clear, input_area);

    if app.busy || app.pending_perm.is_some() || app.enhancing {
        let spinner = SPINNER[app.spinner_tick];
        let chrome_note = Style::default().fg(TUI_ROW_DIM).add_modifier(Modifier::DIM);

        // Calculate elapsed time for display in all busy states
        let elapsed_s = app.turn_start.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        let elapsed_hint = if elapsed_s >= 1 {
            format!(" {elapsed_s}s")
        } else {
            String::new()
        };

        let title_line = if app.pending_perm.is_some() {
            Line::from(vec![
                Span::styled("▸ ", Style::default().fg(TUI_STATE_WARN)),
                Span::styled("Permission required", Style::default().fg(TUI_STATE_WARN)),
                Span::styled(format!("{TUI_SEP}use the dialog above"), chrome_note),
            ])
        } else if app.enhancing {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(TUI_SOFT_ACCENT),
                ),
                Span::styled("Enhancing prompt", Style::default().fg(TUI_SOFT_ACCENT)),
                Span::styled(format!("{TUI_SEP}please wait"), chrome_note),
            ])
        } else if !app.queued.is_empty() {
            let phase = match app.agent_run_state {
                AppRunState::RunningTools => format!("Running tools{elapsed_hint}"),
                AppRunState::Generating => format!("Generating{elapsed_hint}"),
                AppRunState::Idle => format!("Agent running{elapsed_hint}"),
            };
            Line::from(vec![
                Span::styled(format!("{} ", spinner), Style::default().fg(TUI_STATE_BUSY)),
                Span::styled(phase, Style::default().fg(TUI_BRAND_TEXT)),
                Span::styled(
                    format!("{TUI_SEP}Ctrl+Enter interrupt{TUI_SEP}Enter queues input"),
                    chrome_note,
                ),
            ])
        } else if app.text_input.is_empty() {
            let phase = match app.agent_run_state {
                AppRunState::RunningTools => format!("Running tools{elapsed_hint}"),
                AppRunState::Generating => format!("Generating{elapsed_hint}"),
                AppRunState::Idle => format!("Thinking{elapsed_hint}"),
            };
            Line::from(vec![
                Span::styled(format!("{} ", spinner), Style::default().fg(TUI_STATE_BUSY)),
                Span::styled(phase, Style::default().fg(TUI_BRAND_TEXT)),
                Span::styled(
                    format!("{TUI_SEP}type to queue a follow-up{TUI_SEP}Ctrl+Enter interrupt"),
                    chrome_note,
                ),
            ])
        } else {
            let phase = match app.agent_run_state {
                AppRunState::RunningTools => format!("Running tools{elapsed_hint}"),
                AppRunState::Generating => format!("Generating{elapsed_hint}"),
                AppRunState::Idle => format!("Thinking{elapsed_hint}"),
            };
            Line::from(vec![
                Span::styled(format!("{} ", spinner), Style::default().fg(TUI_STATE_BUSY)),
                Span::styled(phase, Style::default().fg(TUI_BRAND_TEXT)),
                Span::styled(
                    format!("{TUI_SEP}Enter queues draft{TUI_SEP}Ctrl+Enter interrupt"),
                    chrome_note,
                ),
            ])
        };
        let border_color = if app.enhancing {
            TUI_SOFT_ACCENT
        } else if app.pending_perm.is_some() {
            TUI_STATE_WARN
        } else {
            TUI_BORDER_UI
        };
        let block = Block::default()
            .style(surfaces::input_dock_fill_style(
                app.pending_perm.is_some(),
                false,
                app.enhancing,
            ))
            .title(title_line)
            .border_type(BorderType::Rounded)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        if app.pending_perm.is_none() {
            frame.set_cursor_position((
                input_area.x + 2 + cursor_col,
                input_area.y + 1 + cursor_row,
            ));
        }
    } else if app.rate_limit_resume_at.is_some() && !app.rate_limit_cancelled {
        // Show countdown to auto-resume after rate limit
        let resume_at = app.rate_limit_resume_at.unwrap();
        let remaining = resume_at.saturating_duration_since(std::time::Instant::now());
        let secs = remaining.as_secs();
        let countdown = if secs >= 3600 {
            format!(
                "{}h {:02}m {:02}s",
                secs / 3600,
                (secs % 3600) / 60,
                secs % 60
            )
        } else if secs >= 60 {
            format!("{}m {:02}s", secs / 60, secs % 60)
        } else {
            format!("{}s", secs)
        };
        let rl_dim = Style::default().fg(TUI_ROW_DIM).add_modifier(Modifier::DIM);
        let title_line = Line::from(vec![
            Span::styled("⏳ ", Style::default().fg(TUI_STATE_WARN)),
            Span::styled(
                format!("Rate limited{TUI_SEP}resume in {countdown}"),
                Style::default().fg(TUI_STATE_WARN),
            ),
            Span::styled(
                format!("{TUI_SEP}Esc cancel{TUI_SEP}/profile to switch"),
                rl_dim,
            ),
        ]);
        let block = Block::default()
            .style(surfaces::input_dock_fill_style(false, true, false))
            .title(title_line)
            .border_type(BorderType::Rounded)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TUI_BORDER_UI));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        frame.set_cursor_position((input_area.x + 2 + cursor_col, input_area.y + 1 + cursor_row));
    } else {
        let idle_title = Line::from(vec![
            Span::styled("Input", Style::default().fg(TUI_SOFT_ACCENT)),
            Span::styled(
                if is_multiline {
                    format!("{TUI_SEP}Shift+Enter newline{TUI_SEP}Enter send{TUI_SEP}Esc clear")
                } else {
                    format!("{TUI_SEP}Enter send{TUI_SEP}Shift+Enter newline{TUI_SEP}Esc clear")
                },
                Style::default().fg(TUI_ROW_DIM).add_modifier(Modifier::DIM),
            ),
        ]);
        let block = Block::default()
            .style(surfaces::input_dock_fill_style(false, false, false))
            .title(idle_title)
            .border_type(BorderType::Rounded)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TUI_BORDER_UI));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        frame.set_cursor_position((input_area.x + 2 + cursor_col, input_area.y + 1 + cursor_row));
    }

    // ── Hint line — hidden when terminal is very narrow ──
    if hint_area.height > 0 && area.width >= 40 {
        let hint_dim = Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM);
        // Mode indicator: show active overlay/picker name
        let mode_label = if !app.overlay_stack.is_empty() {
            app.overlay_stack.top().map(|o| o.title().to_string())
        } else if app.profile_overlay.is_some() {
            Some("Profile".into())
        } else if app.model_picker.is_some() {
            Some("Models".into())
        } else if app.session_picker.is_some() {
            Some("Sessions".into())
        } else if app.profile_picker.is_some() {
            Some("Profiles".into())
        } else {
            None
        };
        let mut hint_spans: Vec<Span<'static>> = Vec::new();
        if let Some(label) = mode_label {
            hint_spans.push(Span::styled(
                format!("{TUI_GUTTER}Mode:{TUI_SEP}{label}{TUI_SEP}"),
                Style::default()
                    .fg(TUI_SOFT_ACCENT)
                    .add_modifier(Modifier::DIM),
            ));
        }
        let hk = Style::default().fg(TUI_BRAND_TEXT);
        hint_spans.extend([
            Span::styled("Enter", hk),
            Span::styled(format!(" send{TUI_SEP}"), hint_dim),
            Span::styled("Shift+Enter", hk),
            Span::styled(format!(" newline{TUI_SEP}"), hint_dim),
            Span::styled("Ctrl+V", hk),
            Span::styled(format!(" paste{TUI_SEP}"), hint_dim),
            Span::styled("Esc", hk),
            Span::styled(format!(" clear{TUI_SEP}"), hint_dim),
            Span::styled("↑↓", hk),
            Span::styled(format!(" history{TUI_SEP}"), hint_dim),
            Span::styled("PgUp/Dn", hk),
            Span::styled(format!(" scroll{TUI_SEP}"), hint_dim),
            Span::styled("/help", hk),
            Span::styled(TUI_SEP, hint_dim),
            Span::styled("/settings", hk),
            Span::styled(format!(" prefs{TUI_SEP}"), hint_dim),
            Span::styled("Ctrl+/", hk),
            Span::styled(format!(" stop{TUI_SEP}"), hint_dim),
            Span::styled("Ctrl+C", hk),
            Span::styled(format!(" quit{TUI_SEP}"), hint_dim),
            Span::styled("Ctrl+L", hk),
            Span::styled(" redraw", hint_dim),
        ]);
        if app.layout.status_rail_active && app.layout.status_panel_max_scroll > 0 {
            hint_spans.push(Span::styled("Alt+PgUp/Dn", hk));
            hint_spans.push(Span::styled(format!(" status{TUI_SEP}"), hint_dim));
        }
        // Scroll position indicator when not following.
        if app.layout.max_scroll > 0 && !app.following {
            let pct = ((app.scroll as u64 * 100) / app.layout.max_scroll as u64).min(100) as u32;
            hint_spans.push(Span::styled(
                format!("{TUI_GUTTER}↑ {pct}%"),
                Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
            ));
        }
        let hintb = surfaces::hint_zone_block();
        let hint_inner = hintb.inner(hint_area);
        let hint_spans = fit_spans(hint_spans, hint_inner.width as usize);
        frame.render_widget(hintb, hint_area);
        let hint = Paragraph::new(Line::from(hint_spans));
        frame.render_widget(hint, hint_inner);
    }

    // ── "↓ new messages" scroll indicator ──
    render_scroll_indicator(frame, app, chat_area);

    // ── Overlay modals (all rendered above the input field, same structure) ──
    //
    // Rendering order matters: later draws on top. Only one modal is active at
    // a time (handle_key enforces this), but we still render in priority order:
    //   slash completions < session picker < permission < error
    //
    // Shared helpers used by every modal:
    //   popup_above_input(input_area, h, w) → Rect anchored just above input
    //   modal_block(title, border_color)    → styled Block
    //   modal_row_two_col(...)              → two-column selectable row

    // ── Slash command popup ──
    let rows = slash_completion_rows(&app.text_input.text);
    if !rows.is_empty() && app.pending_perm.is_none() && app.session_picker.is_none() {
        // Slash command completion popup.
        const VISIBLE: usize = SLASH_COMPLETION_VISIBLE;

        // Find the rendered-row index of the selected command.
        let selected_row_idx = app
            .selected_cmd
            .and_then(|sel| {
                rows.iter().position(
                    |r| matches!(r, CompletionRow::Cmd { flat_idx, .. } if *flat_idx == sel),
                )
            })
            .unwrap_or(0);

        // Scroll so the selected item sits at the bottom of the visible window —
        // same behaviour as the session / model pickers.
        let scroll_offset = selected_row_idx.saturating_sub(VISIBLE - 1);
        let end = (scroll_offset + VISIBLE).min(rows.len());
        let visible_slice = &rows[scroll_offset..end];

        let has_above = scroll_offset > 0;
        let has_below = rows.len() > scroll_offset + VISIBLE;
        let indicator = usize::from(has_above || has_below);
        let popup_h = (visible_slice.len() + 2 + indicator) as u16;

        // Use nearly the full terminal width; cap at 120 for ultra-wide displays.
        let popup_w = area.width.saturating_sub(4).min(120);
        // 2 chars for marker (▶ / space), 18 for command = 20 total left column.
        let cmd_col_w = 22usize;
        let popup_rect = popup_above_input(input_area, popup_h, popup_w);
        let desc_w = (popup_rect.width as usize).saturating_sub(cmd_col_w + 3);

        let mut content: Vec<Line<'static>> = visible_slice
            .iter()
            .map(|row| match row {
                CompletionRow::Header(section) => Line::from(Span::styled(
                    format!("{TUI_GUTTER}── {section} ──"),
                    Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                )),
                CompletionRow::Cmd {
                    flat_idx,
                    cmd,
                    desc,
                } => {
                    let selected = app.selected_cmd == Some(*flat_idx);
                    let marker = if selected { "▶" } else { " " };
                    let key_col = if *flat_idx < 9 {
                        format!("{} ", flat_idx + 1)
                    } else {
                        "  ".to_string()
                    };
                    let desc_str = if desc.len() > desc_w {
                        format!("{}…", &desc[..desc_w.saturating_sub(1)])
                    } else {
                        desc.to_string()
                    };
                    modal_row_two_col(
                        format!(
                            "{}{:<key_w$}{:<width$}",
                            marker,
                            key_col,
                            cmd,
                            key_w = 3,
                            width = cmd_col_w.saturating_sub(4)
                        ),
                        format!(" {}", desc_str),
                        TUI_SOFT_ACCENT,
                        TUI_ROW_DIM,
                        selected,
                    )
                }
            })
            .collect();

        // Scroll indicators — same style as session / model pickers.
        if has_above || has_below {
            let above_n = if has_above { scroll_offset } else { 0 };
            let below_n = if has_below {
                rows.len() - (scroll_offset + VISIBLE)
            } else {
                0
            };
            if let Some(line) = scroll_indicator_line(above_n, below_n) {
                content.push(line);
            }
        }

        let n_cmds = rows
            .iter()
            .filter(|r| matches!(r, CompletionRow::Cmd { .. }))
            .count();
        let title = format!(" / {n_cmds} commands ");
        let hint = " ↑↓ navigate · 1-9 jump to match · Tab / Enter select · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, TUI_SOFT_ACCENT)),
            popup_rect,
        );
    }

    // ── Session picker ───────────────────────────────────────────────────────
    if let Some(ref picker) = app.session_picker {
        const VISIBLE: usize = SESSION_PICKER_VISIBLE;
        let filtered: Vec<(usize, &clido_storage::SessionSummary)> =
            picker.picker.filtered_items().collect();
        let n_rows = filtered.len().min(VISIBLE) as u16;
        // border(2) + header(2) + blank(1) + filter(1) + rows = n_rows + 6
        let popup_h = (n_rows + 6).min(input_area.y.saturating_sub(2));
        let popup_h = popup_h.min(area.height.saturating_sub(4));
        let popup_h = (n_rows + 6).min(popup_h.max(7));
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);

        let inner_w = popup_rect.width.saturating_sub(4) as usize;
        // marker(2) + id(8) + gaps + turns(4) + cost(8) + when(26) + gap + name/preview (rest)
        const TURNS_W: usize = 4;
        const COST_W: usize = 8;
        const WHEN_W: usize = 26;
        let fixed = 2 + 8 + 2 + TURNS_W + 2 + COST_W + 2 + WHEN_W + 1;
        let name_w = inner_w.saturating_sub(fixed).max(8);

        let mut content: Vec<Line<'static>> = Vec::new();
        // Filter line
        if !picker.picker.filter.text.is_empty() {
            content.push(filter_indicator_line(&picker.picker.filter.text));
        }
        content.push(Line::from(vec![Span::styled(
            format!(
                "  {:<8}  {:>4}  {:<26}  {}",
                "id", "turns", "last edited (local · rel)", "name / preview"
            ),
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        )]));
        content.push(Line::from(vec![Span::styled(
            "  ────────  ────  ──────────────────────────  ────────────────────".to_string(),
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        )]));

        let end = (picker.picker.scroll_offset + VISIBLE).min(filtered.len());
        for (di, (_orig_idx, s)) in filtered[picker.picker.scroll_offset..end]
            .iter()
            .enumerate()
        {
            let cursor_selected = picker.picker.scroll_offset + di == picker.picker.selected;
            let is_multi_selected = picker.selected.contains(&s.session_id);
            let bg = if cursor_selected {
                TUI_SELECTION_BG
            } else {
                Color::Reset
            };
            let base_style = Style::default().bg(bg);
            let row_style = if cursor_selected {
                base_style.fg(TUI_TEXT).add_modifier(Modifier::BOLD)
            } else {
                base_style.fg(TUI_TEXT).add_modifier(Modifier::DIM)
            };
            let id_raw: String = s.session_id.chars().take(8).collect();
            let is_active = app.current_session_id.as_deref() == Some(s.session_id.as_str());
            let id_cell = if is_active {
                format!("{:<8}●", id_raw)
            } else {
                format!("{id_raw:<8}")
            };
            let wall = session_wall_clock(&s.last_edited);
            let rel = relative_time(&s.last_edited);
            let mut when_str = format!("{wall} · {rel}");
            if when_str.chars().count() > WHEN_W {
                when_str = wall.chars().take(WHEN_W).collect::<String>();
            }
            let name_part = match s.title.as_deref() {
                Some(t) if !t.is_empty() => {
                    format!("{t} — {}", s.preview)
                }
                _ => s.preview.clone(),
            };
            let name_trunc: String = name_part.chars().take(name_w).collect();
            let check = if is_multi_selected { "☑ " } else { "☐ " };
            let marker = if cursor_selected { "▶ " } else { "  " };
            // Only show turns, not cost - we can't reliably determine if this session
            // used subscription or on-demand pricing, and stored costs may be inaccurate
            content.push(Line::from(vec![Span::styled(
                format!(
                    "{}{}{id_cell}  {:>3}t  {:<w$}  {}",
                    marker,
                    check,
                    s.num_turns,
                    when_str,
                    name_trunc,
                    w = WHEN_W,
                ),
                row_style,
            )]));
        }

        // Add scroll indicators if there are more sessions above or below visible range.
        let above = picker.picker.scroll_offset;
        let below = filtered
            .len()
            .saturating_sub(picker.picker.scroll_offset + VISIBLE);
        if let Some(line) = scroll_indicator_line(above, below) {
            content.push(line);
        }

        let total = filtered.len();
        let selected_count = picker.selected.len();
        let picker_title = if selected_count > 0 {
            format!(" Sessions — {} total, {} selected ", total, selected_count)
        } else {
            format!(" Sessions — {} total ", total)
        };
        let hint = " ↑↓ navigate · Enter resume · Space toggle · Ctrl+D delete selected · c clear · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(
                &picker_title,
                hint,
                modal_border_default(),
            )),
            popup_rect,
        );
    }

    // ── Model picker popup ────────────────────────────────────────────────────
    if let Some(ref picker) = app.model_picker {
        const VISIBLE: usize = 14;
        let filtered = picker.filtered();
        let n_rows = filtered.len().clamp(1, VISIBLE) as u16;
        let popup_h = (n_rows + 5).min(area.height.saturating_sub(4)).max(6);
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);

        let mut content: Vec<Line<'static>> = vec![
            Line::from(vec![Span::styled(
                format!("  Filter: {}_", picker.filter),
                Style::default().fg(TUI_TEXT),
            )]),
            Line::from(vec![Span::styled(
                format!(
                    "    {:<32}  {:<12}  {:>8}  {:>8}  {:>6}  {}",
                    "model", "provider", "$/1M in", "$/1M out", "ctx", "role"
                ),
                Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
            )]),
            Line::raw(""),
        ];

        if filtered.is_empty() {
            let msg = if app.models_loading {
                "  ⟳ Fetching models from provider API…"
            } else {
                "  No models found. Check your API key and network connection."
            };
            content.push(Line::from(vec![Span::styled(
                msg.to_string(),
                Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
            )]));
        } else {
            // Count favorites and recent models for section headers.
            let n_fav = filtered.iter().filter(|m| m.is_favorite).count();
            let recent_set: std::collections::HashSet<&str> =
                app.model_prefs.recent.iter().map(|s| s.as_str()).collect();
            let n_recent = filtered
                .iter()
                .filter(|m| !m.is_favorite && recent_set.contains(m.id.as_str()))
                .count();
            let show_headers = picker.filter.is_empty() && (n_fav > 0 || n_recent > 0);

            let end = (picker.scroll_offset + VISIBLE).min(filtered.len());
            let mut global_idx = picker.scroll_offset;
            // Track which section header we've emitted.
            let mut header_shown_fav = false;
            let mut header_shown_recent = false;
            let mut header_shown_all = false;

            for (di, m) in filtered[picker.scroll_offset..end].iter().enumerate() {
                // Insert section headers when entering a new group.
                if show_headers && !header_shown_fav && m.is_favorite {
                    header_shown_fav = true;
                    content.push(Line::from(vec![Span::styled(
                        "  ★ Favorites".to_string(),
                        Style::default()
                            .fg(TUI_STATE_WARN)
                            .add_modifier(Modifier::BOLD),
                    )]));
                }
                if show_headers
                    && !header_shown_recent
                    && !m.is_favorite
                    && recent_set.contains(m.id.as_str())
                    && global_idx >= n_fav
                {
                    header_shown_recent = true;
                    content.push(Line::from(vec![Span::styled(
                        "  ⏱ Recent".to_string(),
                        Style::default()
                            .fg(TUI_STATE_INFO)
                            .add_modifier(Modifier::BOLD),
                    )]));
                }
                if show_headers
                    && !header_shown_all
                    && !m.is_favorite
                    && !recent_set.contains(m.id.as_str())
                    && global_idx >= n_fav + n_recent
                {
                    header_shown_all = true;
                    content.push(Line::from(vec![Span::styled(
                        "  All".to_string(),
                        Style::default().fg(TUI_MUTED).add_modifier(Modifier::BOLD),
                    )]));
                }

                let selected = picker.scroll_offset + di == picker.selected;
                let bg = if selected {
                    TUI_SELECTION_BG
                } else {
                    Color::Reset
                };
                let fg = if selected { TUI_TEXT } else { TUI_ROW_DIM };
                let fav = if m.is_favorite { "★ " } else { "  " };
                let ctx = m
                    .context_k
                    .map(|k| format!("{:>4}k", k))
                    .unwrap_or_else(|| "    ?".into());
                let role = m.role.as_deref().unwrap_or("-").trim();
                let id_display: String = m.id.chars().take(32).collect();
                let prov_display: String = m.provider.chars().take(12).collect();
                let formatted = format!(
                    "  {}{:<32}  {:<12}  {:>8.2}  {:>8.2}  {:>6}  {}",
                    fav, id_display, prov_display, m.input_mtok, m.output_mtok, ctx, role
                );
                content.push(Line::from(Span::styled(
                    formatted,
                    Style::default().fg(fg).bg(bg),
                )));
                global_idx += 1;
            }

            let above = picker.scroll_offset;
            let below = filtered
                .len()
                .saturating_sub(picker.scroll_offset + VISIBLE);
            if let Some(line) = scroll_indicator_line(above, below) {
                content.push(line);
            }
        }

        let total = filtered.len();
        let title = if app.models_loading && total == 0 {
            " Models — fetching… ".to_string()
        } else {
            format!(" Models — {} found ", total)
        };
        let hint = " ↑↓ navigate · Enter select · Ctrl+S save default · Ctrl+F favorite · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, TUI_MARK)),
            popup_rect,
        );
    }

    // ── Profile picker popup ──────────────────────────────────────────────────
    if let Some(ref picker) = app.profile_picker {
        const VISIBLE: usize = 12;
        let filtered: Vec<(usize, &(String, clido_core::ProfileEntry))> =
            picker.picker.filtered_items().collect();
        let n_rows = filtered.len().clamp(1, VISIBLE) as u16;
        let popup_h = (n_rows + 5).min(area.height.saturating_sub(4)).max(5);
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
        let inner_w = popup_rect.width.saturating_sub(4) as usize;

        let mut content: Vec<Line<'static>> = Vec::new();
        if !picker.picker.filter.text.is_empty() {
            content.push(filter_indicator_line(&picker.picker.filter.text));
        }
        content.push(Line::from(Span::styled(
            format!("  {:<20}  {}", "profile", "provider / model"),
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        )));
        content.push(Line::raw(""));

        if filtered.is_empty() {
            content.push(Line::from(Span::styled(
                if picker.picker.filter.text.is_empty() {
                    "  no profiles — press n to create one"
                } else {
                    "  no matches"
                },
                Style::default().fg(TUI_MUTED),
            )));
        }

        let end = (picker.picker.scroll_offset + VISIBLE).min(filtered.len());
        for (di, (_orig_idx, (name, entry))) in filtered[picker.picker.scroll_offset..end]
            .iter()
            .enumerate()
        {
            let selected = picker.picker.scroll_offset + di == picker.picker.selected;
            let is_active = name == &picker.active;
            let bg = if selected {
                TUI_SELECTION_BG
            } else {
                Color::Reset
            };
            let fg = if selected { TUI_TEXT } else { TUI_ROW_DIM };
            let marker = if selected { "▶" } else { " " };
            let active_mark = if is_active { "●" } else { " " };
            let model_display: String = format!("{} / {}", entry.provider, entry.model)
                .chars()
                .take(inner_w.saturating_sub(24))
                .collect();
            content.push(Line::from(Span::styled(
                format!("{} {} {:<20}  {}", marker, active_mark, name, model_display),
                Style::default().fg(fg).bg(bg),
            )));
        }

        let above = picker.picker.scroll_offset;
        let below = filtered
            .len()
            .saturating_sub(picker.picker.scroll_offset + VISIBLE);
        if let Some(line) = scroll_indicator_line(above, below) {
            content.push(line);
        }

        let title = format!(" Profiles — {} ", picker.active);
        let hint =
            " ↑↓ navigate · Enter switch · Ctrl+N new · Ctrl+E edit · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(
                &title,
                hint,
                modal_border_default(),
            )),
            popup_rect,
        );
    }

    // ── Profile overview/editor overlay ──────────────────────────────────────
    if let Some(ref st) = app.profile_overlay {
        render_profile_overlay(frame, area, input_area, st, app.models_loading);
    }

    // ── Permission popup ─────────────────────────────────────────────────────
    if let Some(perm) = &app.pending_perm {
        let inner_w = input_area.width.saturating_sub(4) as usize;

        // ── Feedback input mode ───────────────────────────────────────────
        if let Some(ref fb) = app.perm_feedback_input {
            let popup_rect = popup_above_input(input_area, 6, input_area.width);
            let preview = truncate_chars(&perm.preview, inner_w);
            let content = vec![
                Line::from(vec![Span::styled(
                    format!("  {}", preview),
                    Style::default().fg(TUI_MUTED),
                )]),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(" Feedback: ", Style::default().fg(TUI_STATE_WARN)),
                    Span::styled(fb.as_str(), Style::default().fg(TUI_TEXT)),
                    Span::styled("█", Style::default().fg(TUI_STATE_WARN)),
                ]),
                Line::raw(""),
                Line::from(vec![Span::styled(
                    "  Enter send · Esc back · paste supported",
                    Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                )]),
            ];
            frame.render_widget(Clear, popup_rect);
            frame.render_widget(
                Paragraph::new(content).block(modal_block(
                    " Explain why you are denying this ",
                    TUI_STATE_ERR,
                )),
                popup_rect,
            );
            return;
        }

        // ── Normal option mode ────────────────────────────────────────────
        // 1 preview + 1 blank + 5 options + 1 hint + 2 borders = 10
        let popup_rect = popup_above_input(input_area, 10, input_area.width);
        let preview = truncate_chars(&perm.preview, inner_w);

        const OPTIONS: &[(&str, &str)] = &[
            ("Allow once", "(1) this invocation only"),
            (
                "Allow for session",
                "(2) all calls to this tool until you quit",
            ),
            (
                "Allow all tools",
                "(3) skip all permission checks this session",
            ),
            ("Deny", "(4) block this call, agent continues"),
            (
                "Deny with feedback",
                "(5) block and explain why to the agent",
            ),
        ];

        let mut content = vec![
            Line::from(vec![Span::styled(
                format!("  {}", preview),
                Style::default().fg(TUI_MUTED),
            )]),
            Line::raw(""),
        ];
        for (i, (label, hint)) in OPTIONS.iter().enumerate() {
            let selected = i == app.perm_selected;
            if selected {
                content.push(Line::from(vec![
                    Span::styled(
                        " ▶ ",
                        Style::default()
                            .fg(TUI_STATE_WARN)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<28}", label),
                        Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", hint), Style::default().fg(TUI_MUTED)),
                ]));
            } else {
                content.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(format!("{:<28}", label), Style::default().fg(TUI_MUTED)),
                    Span::styled(
                        format!("  {}", hint),
                        Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                    ),
                ]));
            }
        }
        content.push(Line::from(vec![Span::styled(
            "  ↑↓ pick · 1–5 jump · Enter confirm · Esc deny",
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        )]));

        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block(
                &format!(" Allow {}? ", tool_display_name(&perm.tool_name)),
                TUI_STATE_WARN,
            )),
            popup_rect,
        );
    }

    // ── Overlay stack ────────────────────────────────────────────────────────
    for overlay in app.overlay_stack.iter_mut() {
        match overlay {
            OverlayKind::Error(e) => {
                let inner_w = input_area.width.saturating_sub(4) as usize;
                let wrapped = word_wrap(&e.message, inner_w);
                let recovery_lines = e
                    .recovery
                    .as_ref()
                    .map(|r| word_wrap(r, inner_w))
                    .unwrap_or_default();
                let extra = if recovery_lines.is_empty() {
                    0
                } else {
                    recovery_lines.len() + 1
                };
                let total_lines = wrapped.len() + extra + 2; // +2: blank + OK footer
                let popup_h = ((total_lines as u16) + 2).min(area.height.saturating_sub(4));
                let inner_h = popup_h.saturating_sub(2) as usize;
                e.max_scroll = total_lines.saturating_sub(inner_h);
                e.scroll_offset = e.scroll_offset.min(e.max_scroll);
                let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
                let mut content: Vec<Line<'static>> = wrapped
                    .into_iter()
                    .map(|l| {
                        Line::from(vec![Span::styled(
                            format!("  {}", l),
                            Style::default().fg(TUI_TEXT),
                        )])
                    })
                    .collect();
                if !recovery_lines.is_empty() {
                    content.push(Line::raw(""));
                    for l in recovery_lines {
                        content.push(Line::from(vec![Span::styled(
                            format!("  → {}", l),
                            Style::default().fg(TUI_STATE_INFO),
                        )]));
                    }
                }
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  [ OK ] · Enter · Esc · Space",
                    Style::default().fg(TUI_MARK).add_modifier(Modifier::BOLD),
                )]));
                let hint_title = if e.max_scroll > 0 {
                    format!(" {} — ↑↓ scroll · Enter/Esc close ", e.title)
                } else {
                    format!(" {} ", e.title)
                };
                frame.render_widget(Clear, popup_rect);
                frame.render_widget(
                    Paragraph::new(content)
                        .block(modal_block(&hint_title, TUI_STATE_ERR))
                        .scroll((e.scroll_offset as u16, 0)),
                    popup_rect,
                );
            }
            OverlayKind::ReadOnly(r) => {
                let mut content: Vec<Line<'static>> = Vec::new();
                if r.lines.is_empty() {
                    content.push(Line::from(vec![Span::styled(
                        "  (empty)".to_string(),
                        Style::default().fg(TUI_MUTED),
                    )]));
                } else {
                    for (heading, text) in &r.lines {
                        if heading.trim().is_empty() && text.trim().is_empty() {
                            content.push(Line::raw(""));
                        } else {
                            if !heading.is_empty() {
                                content.push(Line::from(vec![Span::styled(
                                    format!("  {}", heading),
                                    Style::default()
                                        .fg(TUI_SOFT_ACCENT)
                                        .add_modifier(Modifier::BOLD),
                                )]));
                            }
                            for line in text.lines() {
                                content.push(Line::from(vec![Span::styled(
                                    format!("    {}", line),
                                    Style::default().fg(TUI_ROW_DIM),
                                )]));
                            }
                            content.push(Line::raw(""));
                        }
                    }
                }
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  [ Close ] · Enter · Esc".to_string(),
                    Style::default().fg(TUI_MARK).add_modifier(Modifier::BOLD),
                )]));
                let popup_h = ((content.len() as u16) + 2).min(area.height.saturating_sub(4));
                let inner_h = popup_h.saturating_sub(2) as usize;
                // Update scroll metadata so handle_key can compute bounds correctly.
                r.visible_rows = inner_h;
                r.max_scroll = content.len().saturating_sub(inner_h);
                r.scroll_offset = r.scroll_offset.min(r.max_scroll);
                let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
                let scroll_offset = r.scroll_offset as u16;
                frame.render_widget(Clear, popup_rect);
                let hint_text = if r.max_scroll > 0 {
                    format!(" {} — ↑↓ scroll · Esc close ", r.title)
                } else {
                    format!(" {} — Esc close ", r.title)
                };
                frame.render_widget(
                    Paragraph::new(content)
                        .block(modal_block(&hint_text, modal_border_default()))
                        .wrap(Wrap { trim: false })
                        .scroll((scroll_offset, 0)),
                    popup_rect,
                );
            }
            OverlayKind::Choice(c) => {
                let mut content: Vec<Line<'static>> = Vec::new();
                if !c.message.is_empty() {
                    content.push(Line::from(vec![Span::styled(
                        format!("  {}", c.message),
                        Style::default().fg(TUI_TEXT),
                    )]));
                    content.push(Line::raw(""));
                }
                for (i, choice) in c.choices.iter().enumerate() {
                    let marker = if i == c.selected { "▸ " } else { "  " };
                    let style = if i == c.selected {
                        Style::default().fg(TUI_MARK).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(TUI_TEXT)
                    };
                    content.push(Line::from(vec![Span::styled(
                        format!("  {}{}", marker, choice.label),
                        style,
                    )]));
                }
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  ↑↓ navigate · Enter select · Esc cancel",
                    Style::default().fg(TUI_MUTED),
                )]));
                let popup_h = ((content.len() as u16) + 2).min(area.height.saturating_sub(4));
                let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
                frame.render_widget(Clear, popup_rect);
                frame.render_widget(
                    Paragraph::new(content).block(modal_block(
                        &format!(" {} ", c.title),
                        modal_border_default(),
                    )),
                    popup_rect,
                );
            }
        }
    }

    // ── Toast notifications (auto-dismiss) ──────────────────────────────────
    app.expire_toasts();
    if !app.toasts.is_empty() {
        let max_w = area.width.saturating_sub(4).min(50) as usize;
        let mut fallback_slot = 0u16;
        let toast_bg = TUI_TOAST_BG;
        for toast in app.toasts.iter().take(3) {
            let msg: String = toast.message.chars().take(max_w).collect();
            let msg_display_w: u16 = msg
                .chars()
                .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) as u16)
                .sum();
            let w = (msg_display_w + 2).min(area.width); // +2 for 1-char padding each side

            let toast_rect = if let Some((px, py)) = toast.position {
                // Position near cursor: try above the cursor, shift left if needed.
                let ty = if py >= area.y + 2 { py - 1 } else { py + 1 };
                let tx = if px + w <= area.x + area.width {
                    px
                } else {
                    (area.x + area.width).saturating_sub(w)
                };
                Rect {
                    x: tx,
                    y: ty.clamp(area.y, area.y + area.height - 1),
                    width: w,
                    height: 1,
                }
            } else {
                // Fallback: stack in top-right corner.
                let y = area.y + 1 + fallback_slot * 2;
                fallback_slot += 1;
                if y + 1 >= area.y + area.height {
                    break;
                }
                Rect {
                    x: (area.x + area.width).saturating_sub(w + 1),
                    y,
                    width: w,
                    height: 1,
                }
            };

            frame.render_widget(Clear, toast_rect);
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    format!(" {msg} "),
                    Style::default().fg(toast.style),
                )]))
                .style(Style::default().bg(toast_bg)),
                toast_rect,
            );
        }
    }
}

// ── Render helper sub-functions ────────────────────────────────────────────────

/// Render the header bar (brand, version, model, profile, session info).
/// Takes the pre-built line spans and renders them into `header_area`.
fn render_header_bar(
    frame: &mut Frame,
    hb: ratatui::widgets::Block<'static>,
    header_para: Paragraph<'static>,
    header_area: Rect,
) {
    let h_inner = hb.inner(header_area);
    frame.render_widget(hb, header_area);
    frame.render_widget(header_para, h_inner);
}

/// Render the scroll-down hint indicator when new messages are below the viewport.
fn render_scroll_indicator(frame: &mut Frame, app: &App, chat_area: Rect) {
    if !app.following && app.layout.max_scroll > app.scroll {
        let unread_hint = Span::styled(
            format!("{TUI_GUTTER}↓ new{TUI_SEP}PgDn"),
            Style::default().fg(TUI_MARK).add_modifier(Modifier::BOLD),
        );
        let hint_line = Line::from(vec![unread_hint]);
        let hint_para = Paragraph::new(hint_line);
        let hint_rect = Rect {
            x: chat_area.x + chat_area.width.saturating_sub(20),
            y: chat_area.y + chat_area.height.saturating_sub(1),
            width: 20,
            height: 1,
        };
        frame.render_widget(hint_para, hint_rect);
    }
}

/// Width-aware version; call this from render paths where chat_area.width is known.
/// Uses a per-width render cache keyed by message content hash to avoid re-rendering
/// unchanged messages on every tick.
/// Apply selection highlighting to lines within the visible area.
fn apply_selection_highlight(
    lines: &[Line<'static>],
    selection: &Selection,
    scroll_offset: usize,
    visible_height: usize,
) -> Vec<Line<'static>> {
    let mut result = lines.to_vec();

    let first_visible = scroll_offset;
    let last_visible = (scroll_offset + visible_height).min(lines.len());

    let (start_row, start_col, end_row, end_col) = selection.bounds();

    let sel_style = Style::default().bg(TUI_SELECTION_BG).fg(TUI_TEXT);

    for line_idx in first_visible..last_visible {
        if line_idx >= result.len() {
            break;
        }

        let in_selection = line_idx >= start_row && line_idx <= end_row;
        if !in_selection {
            continue;
        }

        let line_start_col = if line_idx == start_row { start_col } else { 0 };
        let line_end_col = if line_idx == end_row {
            end_col.saturating_add(1) // inclusive end, prevent overflow
        } else {
            usize::MAX
        };

        let line = &result[line_idx];
        let mut new_spans: Vec<Span<'static>> = Vec::new();
        let mut display_col = 0usize;

        for span in line.spans.iter() {
            let span_display_w: usize = span
                .content
                .chars()
                .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
                .sum();
            let span_end_col = display_col + span_display_w;

            if span_end_col <= line_start_col || display_col >= line_end_col {
                // Entirely outside selection.
                new_spans.push(span.clone());
            } else if display_col >= line_start_col && span_end_col <= line_end_col {
                // Entirely inside selection.
                let mut h = span.clone();
                h.style = span.style.patch(sel_style);
                new_spans.push(h);
            } else {
                // Partially overlaps — split the span at column boundaries.
                let mut before = String::new();
                let mut inside = String::new();
                let mut after = String::new();
                let mut col = display_col;
                for ch in span.content.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if col < line_start_col {
                        before.push(ch);
                    } else if col < line_end_col {
                        inside.push(ch);
                    } else {
                        after.push(ch);
                    }
                    col += cw;
                }
                if !before.is_empty() {
                    new_spans.push(Span::styled(before, span.style));
                }
                if !inside.is_empty() {
                    new_spans.push(Span::styled(inside, span.style.patch(sel_style)));
                }
                if !after.is_empty() {
                    new_spans.push(Span::styled(after, span.style));
                }
            }

            display_col = span_end_col;
        }

        result[line_idx] = Line::from(new_spans);
    }

    result
}

/// Pre-wrap styled lines so each `Line` fits within `width` columns.
/// Kept for potential future use; current render path uses `wrap_content_lines` instead.
#[allow(dead_code)]
///
/// After this, `Vec<Line>` index == visual row, which means mouse
/// coordinates (post-wrap) map 1:1 to vector indices. This is critical
/// for selection highlighting and text extraction.
///
/// Word-aware wrapping: never splits words, only breaks at whitespace.
/// Preserves indentation on continuation lines.
fn wrap_styled_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        // Fast path: measure total display width of this line.
        let total_w: usize = line
            .spans
            .iter()
            .map(|s| unicode_display_width(s.content.as_ref()))
            .sum();
        if total_w <= width {
            out.push(line);
            continue;
        }

        // Slow path: word-aware wrapping with indentation preservation
        // Extract indentation from leading whitespace-only spans
        let mut indent_spans: Vec<Span<'static>> = Vec::new();
        let mut content_spans: Vec<Span<'static>> = Vec::new();
        let mut found_non_whitespace = false;

        for span in &line.spans {
            if !found_non_whitespace && span.content.chars().all(|c| c.is_whitespace()) {
                indent_spans.push(span.clone());
            } else {
                found_non_whitespace = true;
                content_spans.push(span.clone());
            }
        }

        // Calculate indent width
        let indent_width: usize = indent_spans
            .iter()
            .map(|s| unicode_display_width(s.content.as_ref()))
            .sum();
        let avail_width = width.saturating_sub(indent_width);

        if avail_width < 10 {
            // Not enough space for word wrapping, fall back to character-based
            out.push(line);
            continue;
        }

        // Collect all content text
        let mut full_text = String::new();
        for span in &content_spans {
            full_text.push_str(span.content.as_ref());
        }

        // Word-wrap the content
        let words: Vec<&str> = full_text.split_whitespace().collect();
        if words.is_empty() {
            out.push(line);
            continue;
        }

        // Build style map: for each byte position in full_text, track the style
        let mut style_map: Vec<Style> = Vec::with_capacity(full_text.len());
        for span in &content_spans {
            let span_len = span.content.len();
            for _ in 0..span_len {
                style_map.push(span.style);
            }
        }

        let mut wrapped_lines: Vec<(String, Vec<Style>)> = Vec::new();
        let mut current_line = String::new();
        let mut current_styles: Vec<Style> = Vec::new();
        let mut current_width = 0usize;
        let mut is_first_line = true;

        for word in words {
            let word_width = unicode_display_width(word);
            let effective_width = if is_first_line { width } else { avail_width };

            // Check if word fits
            let needs_space = !current_line.is_empty();
            let space_width = if needs_space { 1 } else { 0 };

            if current_width + space_width + word_width <= effective_width {
                // Word fits - add it
                if needs_space {
                    current_line.push(' ');
                    current_styles.push(Style::default());
                    current_width += 1;
                }
                // Find the byte position of this word in full_text
                if let Some(word_pos) = full_text.find(word) {
                    for (i, c) in word.chars().enumerate() {
                        current_line.push(c);
                        let style_idx = word_pos + i;
                        if style_idx < style_map.len() {
                            current_styles.push(style_map[style_idx]);
                        } else {
                            current_styles.push(Style::default());
                        }
                    }
                } else {
                    // Fallback: add word without style
                    for c in word.chars() {
                        current_line.push(c);
                        current_styles.push(Style::default());
                    }
                }
                current_width += word_width;
            } else {
                // Word doesn't fit - flush current line
                if !current_line.is_empty() {
                    wrapped_lines.push((current_line, current_styles));
                    current_line = String::new();
                    current_styles = Vec::new();
                    is_first_line = false;
                }

                // Add word to new line
                if let Some(word_pos) = full_text.find(word) {
                    for (i, c) in word.chars().enumerate() {
                        current_line.push(c);
                        let style_idx = word_pos + i;
                        if style_idx < style_map.len() {
                            current_styles.push(style_map[style_idx]);
                        } else {
                            current_styles.push(Style::default());
                        }
                    }
                } else {
                    for c in word.chars() {
                        current_line.push(c);
                        current_styles.push(Style::default());
                    }
                }
                current_width = word_width;
            }
        }

        // Flush last line
        if !current_line.is_empty() {
            wrapped_lines.push((current_line, current_styles));
        }

        // Create Line objects from wrapped text with proper styles
        for (i, (wrapped_text, styles)) in wrapped_lines.iter().enumerate() {
            let mut line_spans: Vec<Span<'static>> = Vec::new();

            // Add indent spans for continuation lines (not first line)
            if i > 0 {
                line_spans.extend(indent_spans.clone());
            } else {
                // First line gets original indent spans
                line_spans.extend(indent_spans.clone());
            }

            // Build spans from text and styles
            if !wrapped_text.is_empty() {
                // Group consecutive characters with the same style
                let mut current_style = styles.first().copied().unwrap_or_default();
                let mut current_text = String::new();

                for (j, c) in wrapped_text.chars().enumerate() {
                    let style = styles.get(j).copied().unwrap_or_default();
                    if style == current_style {
                        current_text.push(c);
                    } else {
                        if !current_text.is_empty() {
                            line_spans.push(Span::styled(current_text, current_style));
                        }
                        current_text = String::from(c);
                        current_style = style;
                    }
                }
                if !current_text.is_empty() {
                    line_spans.push(Span::styled(current_text, current_style));
                }
            }

            out.push(Line::from(line_spans));
        }
    }
    out
}

/// Display width of a string (number of terminal columns).
#[allow(dead_code)]
fn unicode_display_width(s: &str) -> usize {
    // Use the same logic ratatui uses internally.
    s.chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// Take the longest prefix of `s` that fits in `max_cols` terminal columns.
/// Returns (prefix_str, display_width_of_prefix).
#[allow(dead_code)]
fn take_cols(s: &str, max_cols: usize) -> (&str, usize) {
    let mut col = 0usize;
    for (i, c) in s.char_indices() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if col + w > max_cols {
            return (&s[..i], col);
        }
        col += w;
    }
    (s, col)
}

pub(super) fn build_lines_w(app: &mut App, width: usize) -> Vec<Line<'static>> {
    // Fast path: return cached lines if messages and width haven't changed.
    // This avoids expensive markdown rendering on every frame.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let msg_count = app.messages.len();
    let last_msg_hash = app.messages.last().map(|m| {
        let mut hasher = DefaultHasher::new();
        std::mem::discriminant(m).hash(&mut hasher);
        // Hash the text content (works for all variants with String payload)
        match m {
            ChatLine::User(s) | ChatLine::Assistant(s) | ChatLine::Thinking(s)
            | ChatLine::Diff(s) | ChatLine::Info(s) | ChatLine::Section(s) => s.hash(&mut hasher),
            ChatLine::ToolCall { name, detail, done, is_error, .. } => {
                name.hash(&mut hasher);
                detail.hash(&mut hasher);
                done.hash(&mut hasher);
                is_error.hash(&mut hasher);
            }
            ChatLine::SlashCommand { cmd, text } => {
                cmd.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            ChatLine::WelcomeBrand | ChatLine::WelcomeSplash => {}
        }
        hasher.finish()
    });
    let cache_key = (msg_count, last_msg_hash, width);

    if app.last_chat_key.as_ref() == Some(&cache_key) {
        return app.last_lines.clone();
    }

    // Incremental rendering: only render NEW messages that weren't in the cache.
    // This avoids re-rendering all messages when only the last one changes.
    let new_msg_count = app.messages.len();
    let cached_msg_count = app.render_cache_msg_count;

    let (content_lines, _) = if new_msg_count > cached_msg_count && cached_msg_count > 0 {
        // Incremental: render only new messages and append to cached ones.
        let new_messages = &app.messages[cached_msg_count..];
        let new_content = render_chat_to_content_lines(new_messages, width, &app.model);

        // Adjust msg_idx in new content to reflect absolute positions
        let mut adjusted = Vec::with_capacity(new_content.len());
        for mut cl in new_content {
            cl.msg_idx += cached_msg_count;
            adjusted.push(cl);
        }

        // Append to existing content_lines
        app.content_lines.extend(adjusted);

        // Re-wrap from combined content lines
        let wrapped_lines = wrap_content_lines(&app.content_lines, width);
        app.wrapped_lines = wrapped_lines.clone();
        app.rendered_line_texts = wrapped_lines.iter().map(|wl| wl.plain_text()).collect();

        let lines: Vec<Line<'static>> = wrapped_lines
            .iter()
            .map(|wl| Line::from(wl.spans.clone()))
            .collect();

        app.last_chat_key = Some(cache_key);
        app.last_lines = lines.clone();
        app.render_cache_msg_count = new_msg_count;

        (lines, cache_key)
    } else {
        // Full render: no cache or width changed or messages were removed.
        let content_lines = render_chat_to_content_lines(&app.messages, width, &app.model);
        app.content_lines = content_lines.clone();

        let wrapped_lines = wrap_content_lines(&content_lines, width);
        app.wrapped_lines = wrapped_lines.clone();
        app.rendered_line_texts = wrapped_lines.iter().map(|wl| wl.plain_text()).collect();

        let lines: Vec<Line<'static>> = wrapped_lines
            .iter()
            .map(|wl| Line::from(wl.spans.clone()))
            .collect();

        app.last_chat_key = Some(cache_key);
        app.last_lines = lines.clone();
        app.render_cache_msg_count = new_msg_count;

        (lines, cache_key)
    };

    content_lines
}

/// Wrap content lines to fit screen width, tracking original positions.
fn wrap_content_lines(
    content_lines: &[crate::tui::state::ContentLine],
    width: usize,
) -> Vec<crate::tui::state::WrappedLine> {
    let mut wrapped_lines = Vec::new();

    for (content_idx, line) in content_lines.iter().enumerate() {
        let mut char_offset = 0usize;
        let mut chars_on_segment = 0usize;

        // Split the line into chunks that fit within width
        let mut current_col = 0usize;
        let mut current_spans: Vec<Span<'static>> = Vec::new();

        for span in &line.spans {
            let span_text = span.content.as_ref();
            let span_chars = span_text.chars().peekable();

            for ch in span_chars {
                let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

                if current_col + ch_width > width && current_col > 0 {
                    // Finish current wrapped line
                    wrapped_lines.push(crate::tui::state::WrappedLine::new(
                        current_spans.clone(),
                        line.source,
                        line.selectable,
                        line.msg_idx,
                        content_idx,
                        char_offset,
                    ));

                    // Advance char_offset by the number of chars emitted so far,
                    // then reset for the next segment.
                    char_offset += chars_on_segment;
                    chars_on_segment = 0;
                    current_spans = Vec::new();
                    current_col = 0;
                }

                // Add character to current span
                if let Some(last_span) = current_spans.last_mut() {
                    if last_span.style == span.style {
                        last_span.content = format!("{}{}", last_span.content, ch).into();
                    } else {
                        current_spans.push(Span::styled(ch.to_string(), span.style));
                    }
                } else {
                    current_spans.push(Span::styled(ch.to_string(), span.style));
                }

                chars_on_segment += 1;
                current_col += ch_width;
            }
        }

        // Always emit at least one WrappedLine per ContentLine.
        // Empty ContentLines (blank separators between messages) must produce a
        // blank row so that visual spacing and row-index arithmetic stay consistent.
        // Blank rows are never selectable — no content to copy.
        let is_blank = current_spans.is_empty();
        wrapped_lines.push(crate::tui::state::WrappedLine::new(
            current_spans,
            line.source,
            line.selectable && !is_blank,
            line.msg_idx,
            content_idx,
            char_offset,
        ));
    }

    wrapped_lines
}

/// Parse `@@ -old_start[,len] +new_start[,len] @@` → (old_start, new_start).
pub(super) fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let inner = line.strip_prefix("@@ ")?.split(" @@").next()?;
    let mut parts = inner.split_whitespace();
    let old_part = parts.next()?;
    let new_part = parts.next()?;
    let old_start: u32 = old_part
        .trim_start_matches('-')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new_start: u32 = new_part
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old_start, new_start))
}

/// Render markdown text into a series of tui `Line`s with appropriate styling.
///
/// Supports: headings, bold/italic/strikethrough, inline code, fenced code blocks,
/// ordered/unordered lists, blockquotes, tables (with box-drawing borders),
/// horizontal rules, task-list checkboxes, and hard/soft breaks.
pub(super) fn render_markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Tag};
    // Content width: match transcript gutter (2) + body inset (2) + 8 char safety margin
    // + 8 char for max list indentation (4 levels deep).
    let content_w = width
        .saturating_sub(super::TUI_GUTTER.len() * 2 + 8 + 8)
        .max(20);

    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(text, opts);

    let mut out: Vec<Line<'static>> = Vec::new();
    // Spans accumulating for the current output line.
    let mut cur_spans: Vec<Span<'static>> = Vec::new();

    // ── Inline style stack ────────────────────────────────────────────────
    // Each entry is the *combined* Style at that nesting depth.
    // On Start(Strong/Emphasis/…) we push a new style; on End we pop.
    // Text events use the top-of-stack style — no more empty-span tricks.
    let mut style_stack: Vec<Style> = vec![Style::default()];

    // ── Block state ───────────────────────────────────────────────────────
    let mut in_code_block = false;

    // ── List state ────────────────────────────────────────────────────────
    let mut list_depth: u32 = 0;

    // ── Blockquote depth ─────────────────────────────────────────────────
    let mut bq_depth: u32 = 0;

    // ── Table state ───────────────────────────────────────────────────────
    let mut in_table_head = false;
    let mut in_table_cell = false;
    let mut table_alignments: Vec<pulldown_cmark::Alignment> = Vec::new();
    let mut table_header: Option<Vec<String>> = None;
    let mut table_body: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();

    // flush current_spans as a new output Line (macro so it can access locals).
    macro_rules! flush {
        () => {
            if !cur_spans.is_empty() {
                out.push(Line::from(std::mem::take(&mut cur_spans)));
            }
        };
    }

    // Push a style derived from the current top-of-stack with an extra modifier.
    macro_rules! push_style {
        ($modifier:expr) => {{
            let s = style_stack
                .last()
                .copied()
                .unwrap_or_default()
                .add_modifier($modifier);
            style_stack.push(s);
        }};
    }

    for event in parser {
        match event {
            // ── Start tags ────────────────────────────────────────────────
            Event::Start(tag) => match tag {
                Tag::Strong => {
                    push_style!(Modifier::BOLD);
                }
                Tag::Emphasis => {
                    push_style!(Modifier::ITALIC);
                }
                Tag::Strikethrough => {
                    push_style!(Modifier::CROSSED_OUT);
                }
                Tag::Link(..) => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .fg(TUI_SOFT_ACCENT)
                        .add_modifier(Modifier::UNDERLINED);
                    style_stack.push(s);
                }
                Tag::Heading(level, ..) => {
                    flush!();
                    let prefix = match level {
                        HeadingLevel::H1 => "◉ ",
                        HeadingLevel::H2 => "◇ ",
                        HeadingLevel::H3 => "▸ ",
                        _ => "  ",
                    };
                    cur_spans.push(Span::styled(
                        prefix.to_string(),
                        Style::default()
                            .fg(TUI_SOFT_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ));
                    style_stack.push(
                        Style::default()
                            .fg(TUI_SOFT_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    flush!();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) if !l.is_empty() => l.to_string(),
                        _ => String::new(),
                    };
                    let fence_tail = |used: usize| {
                        let n = content_w.saturating_sub(used).clamp(8, 40);
                        "─".repeat(n)
                    };
                    if lang.is_empty() {
                        let tail = fence_tail(4);
                        out.push(Line::from(vec![Span::styled(
                            format!("  ┌{tail}"),
                            Style::default().fg(TUI_CODE_BORDER),
                        )]));
                    } else {
                        let used = lang.chars().count() + 5;
                        let tail = fence_tail(used);
                        out.push(Line::from(vec![Span::styled(
                            format!("  ┌ {lang} {tail}"),
                            Style::default().fg(TUI_CODE_LANG),
                        )]));
                    }
                    // Code content uses code block background
                    style_stack.push(Style::default().fg(TUI_CODE_FG).bg(TUI_CODE_BG));
                }
                Tag::List(_) => {
                    list_depth += 1;
                }
                Tag::Item => {
                    flush!();
                    let indent = "  ".repeat(list_depth.saturating_sub(1) as usize);
                    cur_spans.push(Span::styled(
                        format!("{}· ", indent),
                        Style::default()
                            .fg(TUI_SOFT_ACCENT)
                            .add_modifier(Modifier::DIM),
                    ));
                }
                Tag::Paragraph => {}
                Tag::BlockQuote => {
                    bq_depth += 1;
                }
                Tag::Table(aligns) => {
                    table_alignments = aligns;
                    table_header = None;
                    table_body.clear();
                    flush!();
                }
                Tag::TableHead => {
                    in_table_head = true;
                }
                Tag::TableRow => {
                    current_row.clear();
                }
                Tag::TableCell => {
                    in_table_cell = true;
                    current_cell.clear();
                }
                _ => {}
            },

            // ── End tags ──────────────────────────────────────────────────
            Event::End(tag) => match tag {
                Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link(..) => {
                    style_stack.pop();
                }
                Tag::Heading(..) => {
                    style_stack.pop();
                    flush!();
                    out.push(Line::raw(""));
                }
                Tag::CodeBlock(_) => {
                    in_code_block = false;
                    flush!();
                    style_stack.pop();
                    out.push(Line::from(vec![Span::styled(
                        format!("  └{}", "─".repeat(content_w.min(60))),
                        Style::default().fg(TUI_CODE_BORDER),
                    )]));
                    out.push(Line::raw(""));
                }
                Tag::List(_) => {
                    list_depth = list_depth.saturating_sub(1);
                    if list_depth == 0 {
                        out.push(Line::raw(""));
                    }
                }
                Tag::Item => {
                    flush!();
                }
                Tag::Paragraph => {
                    flush!();
                    out.push(Line::raw(""));
                }
                Tag::BlockQuote => {
                    flush!();
                    bq_depth = bq_depth.saturating_sub(1);
                    if bq_depth == 0 {
                        out.push(Line::raw(""));
                    }
                }
                Tag::TableCell => {
                    in_table_cell = false;
                    current_row.push(std::mem::take(&mut current_cell));
                }
                Tag::TableRow => {
                    if !in_table_head {
                        table_body.push(std::mem::take(&mut current_row));
                    }
                }
                Tag::TableHead => {
                    in_table_head = false;
                    table_header = Some(std::mem::take(&mut current_row));
                }
                Tag::Table(_) => {
                    render_table_to_lines(
                        table_header.take(),
                        std::mem::take(&mut table_body),
                        &table_alignments,
                        &mut out,
                    );
                }
                _ => {}
            },

            // ── Leaf events ───────────────────────────────────────────────
            Event::Text(t) => {
                if in_table_cell {
                    current_cell.push_str(&t);
                } else if in_code_block {
                    // Code text arrives as one blob; split on newlines.
                    for (i, line) in t.split('\n').enumerate() {
                        if i > 0 {
                            flush!();
                        }
                        if !line.is_empty() {
                            cur_spans.push(Span::styled(
                                format!("  {}", line),
                                style_stack.last().copied().unwrap_or_default(),
                            ));
                        }
                    }
                } else {
                    // Emit blockquote gutter at the start of each line.
                    if bq_depth > 0 && cur_spans.is_empty() {
                        cur_spans.push(Span::styled(
                            "│ ".repeat(bq_depth as usize),
                            Style::default().fg(TUI_DIVIDER).add_modifier(Modifier::DIM),
                        ));
                    }
                    let style = style_stack.last().copied().unwrap_or_default();
                    cur_spans.push(Span::styled(t.to_string(), style));
                }
            }
            Event::Code(t) => {
                // Inline code — padded cell, theme tokens (no inherited emphasis).
                if in_table_cell {
                    current_cell.push_str(&t);
                } else {
                    cur_spans.push(Span::styled(
                        format!(" {} ", t),
                        Style::default()
                            .fg(TUI_INLINE_CODE_FG)
                            .bg(TUI_INLINE_CODE_BG),
                    ));
                }
            }
            Event::SoftBreak => {
                if !in_table_cell && !in_code_block {
                    cur_spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if !in_table_cell {
                    flush!();
                }
            }
            Event::Rule => {
                flush!();
                let w = content_w.saturating_sub(4).clamp(16, 64);
                out.push(Line::from(vec![Span::styled(
                    format!("  {}", "─".repeat(w)),
                    Style::default().fg(TUI_DIVIDER).add_modifier(Modifier::DIM),
                )]));
                out.push(Line::raw(""));
            }
            Event::TaskListMarker(checked) => {
                let (marker, col) = if checked {
                    ("✓ ", TUI_STATE_OK)
                } else {
                    ("○ ", TUI_ROW_DIM)
                };
                cur_spans.push(Span::styled(
                    marker.to_string(),
                    Style::default().fg(col).add_modifier(Modifier::DIM),
                ));
            }
            Event::Html(_) | Event::FootnoteReference(_) => {}
        }
    }

    flush!();
    out
}

/// Render a collected markdown table into box-drawing `Line`s.
///
/// ```text
/// ┌──────────┬──────────┬──────────┐
/// │  Header1 │  Header2 │  Header3 │
/// ├──────────┼──────────┼──────────┤
/// │  Cell A  │  Cell B  │  Cell C  │
/// └──────────┴──────────┴──────────┘
/// ```
pub(super) fn render_table_to_lines(
    header: Option<Vec<String>>,
    rows: Vec<Vec<String>>,
    alignments: &[pulldown_cmark::Alignment],
    out: &mut Vec<Line<'static>>,
) {
    use pulldown_cmark::Alignment as Align;

    let ncols = alignments
        .len()
        .max(header.as_ref().map_or(0, |h| h.len()))
        .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));

    if ncols == 0 {
        return;
    }

    // Compute per-column content widths (padding added separately).
    let mut col_widths = vec![1usize; ncols];
    if let Some(ref h) = header {
        for (i, cell) in h.iter().enumerate().take(ncols) {
            col_widths[i] = col_widths[i].max(cell.len());
        }
    }
    for row in &rows {
        for (i, cell) in row.iter().enumerate().take(ncols) {
            col_widths[i] = col_widths[i].max(cell.len());
        }
    }

    let align_cell = |content: &str, width: usize, align: &Align| -> String {
        match align {
            Align::Right => format!("{:>width$}", content),
            Align::Center => {
                let pad = width.saturating_sub(content.len());
                let left = pad / 2;
                format!("{}{}{}", " ".repeat(left), content, " ".repeat(pad - left))
            }
            _ => format!("{:<width$}", content),
        }
    };

    let gray = Style::default().fg(TUI_MUTED);
    let hdr_style = Style::default()
        .fg(TUI_SOFT_ACCENT)
        .add_modifier(Modifier::BOLD);

    // ┌──┬──┬──┐
    let top: String = col_widths
        .iter()
        .map(|w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("┬");
    out.push(Line::from(vec![Span::styled(format!("┌{}┐", top), gray)]));

    // Header row (cyan bold)
    if let Some(ref h) = header {
        let mut spans = vec![Span::styled("│".to_string(), gray)];
        for (i, &w) in col_widths.iter().enumerate().take(ncols) {
            let content = h.get(i).map(|s| s.as_str()).unwrap_or("");
            let cell = align_cell(content, w, alignments.get(i).unwrap_or(&Align::None));
            spans.push(Span::styled(format!(" {} ", cell), hdr_style));
            spans.push(Span::styled("│".to_string(), gray));
        }
        out.push(Line::from(spans));

        // ├──┼──┼──┤
        let sep: String = col_widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┼");
        out.push(Line::from(vec![Span::styled(format!("├{}┤", sep), gray)]));
    }

    // Body rows
    for row in &rows {
        let mut spans = vec![Span::styled("│".to_string(), gray)];
        for (i, &w) in col_widths.iter().enumerate().take(ncols) {
            let content = row.get(i).map(|s| s.as_str()).unwrap_or("");
            let cell = align_cell(content, w, alignments.get(i).unwrap_or(&Align::None));
            spans.push(Span::raw(format!(" {} ", cell)));
            spans.push(Span::styled("│".to_string(), gray));
        }
        out.push(Line::from(spans));
    }

    // └──┴──┴──┘
    let bot: String = col_widths
        .iter()
        .map(|w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("┴");
    out.push(Line::from(vec![Span::styled(format!("└{}┘", bot), gray)]));
    out.push(Line::raw(""));
}

// ── Unified Content Line Renderer ─────────────────────────────────────────────

/// Render all chat messages to a unified list of ContentLines.
/// This is the single source of truth for display, scrolling, and selection.
pub(super) fn render_chat_to_content_lines(
    messages: &[ChatLine],
    width: usize,
    model: &str,
) -> Vec<ContentLine> {
    let mut out = Vec::new();

    for (msg_idx, msg) in messages.iter().enumerate() {
        match msg {
            ChatLine::User(text) => {
                // Header line
                out.push(ContentLine::new(
                    vec![Span::styled(
                        format!("{TUI_GUTTER}You"),
                        Style::default().fg(TUI_ACCENT).add_modifier(Modifier::BOLD),
                    )],
                    LineSource::User,
                    false, // Header not selectable
                    msg_idx,
                ));

                // Content lines
                for line in render_markdown(text, width) {
                    let spans = vec![Span::raw(TUI_GUTTER), Span::raw("  ")];
                    let mut content_line = ContentLine::new(
                        spans,
                        LineSource::User,
                        true, // Content selectable
                        msg_idx,
                    );
                    content_line.spans.extend(line.spans);
                    out.push(content_line);
                }

                // Blank line separator
                out.push(ContentLine::new(vec![], LineSource::User, false, msg_idx));
            }

            ChatLine::Assistant(text) => {
                let model_bit = if model.is_empty() {
                    String::new()
                } else {
                    format!("{TUI_SEP}{}", model)
                };

                // Header line
                out.push(ContentLine::new(
                    vec![
                        Span::styled(
                            format!("{TUI_GUTTER}{TUI_CHAT_AGENT_LABEL}"),
                            Style::default()
                                .fg(TUI_SOFT_ACCENT)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            model_bit,
                            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                        ),
                    ],
                    LineSource::Assistant,
                    false,
                    msg_idx,
                ));

                // Content lines
                for line in render_markdown(text, width) {
                    let spans = vec![Span::raw(TUI_GUTTER), Span::raw("  ")];
                    let mut content_line =
                        ContentLine::new(spans, LineSource::Assistant, true, msg_idx);
                    content_line.spans.extend(line.spans);
                    out.push(content_line);
                }

                // Blank line separator
                out.push(ContentLine::new(
                    vec![],
                    LineSource::Assistant,
                    false,
                    msg_idx,
                ));
            }

            ChatLine::Thinking(text) => {
                // Thinking is rendered inline without header
                for line in render_markdown(text, width) {
                    let spans = vec![
                        Span::styled(
                            TUI_GUTTER,
                            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
                        ),
                        Span::raw("  "),
                    ];
                    let mut content_line =
                        ContentLine::new(spans, LineSource::Thinking, true, msg_idx);
                    // Dim all spans
                    for span in line.spans {
                        content_line.spans.push(Span::styled(
                            span.content.to_string(),
                            span.style.add_modifier(Modifier::DIM),
                        ));
                    }
                    out.push(content_line);
                }
            }

            ChatLine::Info(text) => {
                if text.is_empty() {
                    out.push(ContentLine::new(vec![], LineSource::Info, false, msg_idx));
                } else {
                    // Info uses "› " prefix, so we need different width than regular chat.
                    let info_prefix_width = crate::tui::render::unicode_display_width(TUI_GUTTER) + 2;
                    let info_content_width = width.saturating_sub(info_prefix_width);
                    let muted_style = Style::default().fg(TUI_ROW_DIM);
                    let prefix = format!("{TUI_GUTTER}› ");
                    for line in render_markdown(text, info_content_width) {
                        let mut spans = vec![Span::styled(
                            prefix.clone(),
                            Style::default().fg(TUI_DIVIDER).add_modifier(Modifier::DIM),
                        )];
                        spans.extend(line.spans.into_iter().map(|span| {
                            Span::styled(span.content.to_string(), muted_style.patch(span.style))
                        }));
                        out.push(ContentLine::new(
                            spans,
                            LineSource::Info,
                            true,
                            msg_idx,
                        ));
                    }
                    // Blank separator
                    out.push(ContentLine::new(vec![], LineSource::Info, false, msg_idx));
                }
            }

            ChatLine::Section(text) => {
                out.push(ContentLine::new(
                    vec![Span::styled(
                        format!("{TUI_GUTTER}{}", text),
                        Style::default()
                            .fg(TUI_ACCENT)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                    )],
                    LineSource::Section,
                    false,
                    msg_idx,
                ));
                out.push(ContentLine::new(
                    vec![],
                    LineSource::Section,
                    false,
                    msg_idx,
                ));
            }

            ChatLine::ToolCall { name, detail, .. } => {
                // Tool call header
                out.push(ContentLine::new(
                    vec![
                        Span::styled(
                            format!("{TUI_GUTTER}▶ ",),
                            Style::default().fg(TUI_SOFT_ACCENT),
                        ),
                        Span::styled(
                            name.clone(),
                            Style::default()
                                .fg(TUI_SOFT_ACCENT)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ],
                    LineSource::ToolCall,
                    false,
                    msg_idx,
                ));

                // Tool detail
                for line in detail.lines() {
                    out.push(ContentLine::new(
                        vec![
                            Span::raw(TUI_GUTTER),
                            Span::raw("  "),
                            Span::styled(line.to_string(), Style::default().fg(TUI_MUTED)),
                        ],
                        LineSource::ToolCall,
                        true,
                        msg_idx,
                    ));
                }

                out.push(ContentLine::new(
                    vec![],
                    LineSource::ToolCall,
                    false,
                    msg_idx,
                ));
            }

            ChatLine::Diff(text) => {
                // Use the unified diff renderer (side-by-side on wide terminals ≥ 120 cols,
                // inline unified on narrower screens).
                let diff_lines = render_diff(text, width);
                for line in diff_lines {
                    let mut spans = Vec::with_capacity(1 + line.spans.len());
                    spans.push(Span::raw(TUI_GUTTER));
                    spans.push(Span::raw("  "));
                    spans.extend(line.spans);
                    out.push(ContentLine::new(
                        spans,
                        LineSource::Diff,
                        true,
                        msg_idx,
                    ));
                }
                out.push(ContentLine::new(vec![], LineSource::Diff, false, msg_idx));
            }

            ChatLine::SlashCommand { cmd, text } => {
                // Command line (highlighted)
                out.push(ContentLine::new(
                    vec![
                        Span::raw(TUI_GUTTER),
                        Span::styled(
                            cmd.clone(),
                            Style::default()
                                .fg(TUI_SOFT_ACCENT)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ],
                    LineSource::User,
                    false,
                    msg_idx,
                ));

                // Optional text argument
                if let Some(t) = text {
                    if !t.is_empty() {
                        for line in render_markdown(t, width) {
                            let spans = vec![Span::raw(TUI_GUTTER), Span::raw("  ")];
                            let mut content_line =
                                ContentLine::new(spans, LineSource::User, true, msg_idx);
                            content_line.spans.extend(line.spans);
                            out.push(content_line);
                        }
                    }
                }

                out.push(ContentLine::new(vec![], LineSource::User, false, msg_idx));
            }

            _ => {
                // Skip WelcomeBrand and WelcomeSplash
            }
        }
    }

    out
}
