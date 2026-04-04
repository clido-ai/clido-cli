mod diff;
mod plan;
mod profile;
mod welcome;
mod widgets;

pub(super) use plan::*;
pub(super) use profile::*;
pub(super) use welcome::*;
pub(super) use widgets::*;

use std::hash::{Hash, Hasher};

use pulldown_cmark::Parser;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::overlay::OverlayKind;

use super::*;

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

    // ── Header spans (built before layout so we can measure and pick height) ──
    let version = env!("CARGO_PKG_VERSION");
    let dim = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);

    // Line 1: brand · version  |  provider/model  |  profile  |  session title
    let mut hline1: Vec<Span<'static>> = vec![
        Span::styled(
            "cli",
            Style::default()
                .fg(Color::Rgb(210, 220, 240))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            ";",
            Style::default()
                .fg(TUI_SOFT_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "do",
            Style::default()
                .fg(Color::Rgb(210, 220, 240))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    hline1.push(Span::raw(" "));
    hline1.push(Span::styled(format!("v{}", version), dim));

    if !app.provider.is_empty() || !app.model.is_empty() {
        let model_str = if app.per_turn_prev_model.is_some() {
            format!("{}  {}⁺", app.provider, app.model)
        } else {
            format!("{}  {}", app.provider, app.model)
        };
        hline1.push(Span::styled("  │ ", Style::default().fg(Color::Rgb(60, 60, 75))));
        hline1.push(Span::styled(model_str, dim));
    }
    hline1.push(Span::styled("  │ ", Style::default().fg(Color::Rgb(60, 60, 75))));
    hline1.push(Span::styled(
        app.current_profile.clone(),
        Style::default()
            .fg(TUI_SOFT_ACCENT)
            .add_modifier(Modifier::BOLD),
    ));
    // Session info compact: id + title in one span
    if let Some(ref session_id) = app.current_session_id {
        let short_id = session_id[..session_id.len().min(8)].to_string();
        hline1.push(Span::styled(
            format!("  #{}", short_id),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
    }
    if let Some(ref title) = app.session_title {
        let title_str = title.clone();
        hline1.push(Span::styled(
            format!("  — {}", title_str),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
    }
    if app.reviewer_configured {
        let reviewer_on = app.reviewer_enabled.load(Ordering::Relaxed);
        let (dot, color) = if reviewer_on {
            ("●", Color::Green)
        } else {
            ("○", Color::DarkGray)
        };
        hline1.push(Span::styled("  │ ", Style::default().fg(Color::Rgb(60, 60, 75))));
        hline1.push(Span::styled(format!("r {}", dot), Style::default().fg(color).add_modifier(Modifier::DIM)));
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
            format!("  {}", short)
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
            format!("  {:.0}% window", pct)
        } else {
            String::new()
        };

        let is_subscription = clido_providers::is_subscription_provider(&app.provider);

        if is_subscription {
            // Subscription providers: show tokens + turns, no dollar cost
            let turn_str = if app.stats.session_turn_count > 0 {
                format!("  {} turns", app.stats.session_turn_count)
            } else {
                String::new()
            };
            hline2.push(Span::styled(
                format!("   session: {}{}{}", tok_str, turn_str, ctx_str),
                dim,
            ));
        } else {
            // On-demand providers: show cost in USD
            let cost_str = if let Some(budget) = app.max_budget_usd {
                format!("${:.4} / ${:.2}", app.stats.session_total_cost_usd, budget)
            } else {
                format!("${:.4}", app.stats.session_total_cost_usd)
            };
            hline2.push(Span::styled(
                format!("   session: {}  {}{}", cost_str, tok_str, ctx_str),
                dim,
            ));
        }
    } else if let Some(budget) = app.max_budget_usd {
        if !clido_providers::is_subscription_provider(&app.provider) {
            hline2.push(Span::styled(format!("   budget: ${:.2}", budget), dim));
        }
    }

    // Decide header height: 1 line if everything fits side-by-side, else 2.
    // When the terminal is very narrow, use a single minimal header.
    let w = area.width as usize;
    let is_narrow = area.width < 60;
    let line1_w: usize = hline1.iter().map(|s| s.content.chars().count()).sum();
    let line2_w: usize = hline2.iter().map(|s| s.content.chars().count()).sum();
    let header_h: u16 = if is_narrow || line1_w + line2_w <= w {
        1
    } else {
        2
    };

    // Layout: header | chat | status (2) | queue (1) | hint (1) | input (dynamic)
    // Input grows with content: 1 line of text = 3 rows (2 borders + 1), capped at 12.
    // When very narrow (< 40), collapse optional rows to avoid layout panics.
    let input_line_count = app.text_input.text.matches('\n').count() + 1;
    let input_h = (input_line_count as u16 + 2).clamp(3, 7); // 1-5 text lines + 2 borders, min 3
    let (hint_h, status_h) = if area.width < 40 { (0, 0) } else { (1, 2) };
    // Queue area: height = header + items, but reserve enough chat space (min 10 lines).
    // Dynamic — fills available vertical space on tall terminals.
    let min_chat_h = 10u16;
    let reserved_h = header_h + status_h + hint_h + input_h + 2; // chat borders
    let available_for_queue = area.height.saturating_sub(min_chat_h + reserved_h);
    let queue_h = if app.current_step.is_some() && !app.queued.is_empty() {
        let total = 1 + 1 + app.queued.len();
        total.min(available_for_queue.max(3) as usize) as u16
    } else if app.current_step.is_some() {
        1 // Agent thinking - show step only
    } else if !app.queued.is_empty() {
        let total = 1 + app.queued.len() + 1;
        total.min(available_for_queue.max(3) as usize) as u16 // header + items
    } else {
        0 // Nothing to show
    };
    let [header_area, chat_area, status_area, queue_area, hint_area, input_area] =
        Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Min(0),
            Constraint::Length(status_h),
            Constraint::Length(queue_h),
            Constraint::Length(hint_h),
            Constraint::Length(input_h),
        ])
        .areas(area);

    // ── Header render ──
    let header_para = if is_narrow {
        // Minimal single-line header: just the model name.
        Paragraph::new(Line::from(vec![Span::styled(
            truncate_chars(&app.model, w),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]))
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
    frame.render_widget(header_para, header_area);

    // ── Chat ──
    // Store chat area bounds for mouse selection handlers.
    app.layout.chat_area_y = (chat_area.y, chat_area.y + chat_area.height);
    app.layout.chat_area_width = chat_area.width;

    if is_welcome_only(app) {
        render_welcome(frame, app, chat_area);
    } else {
        // Use ratatui's own line_count() so the scroll calculation matches actual rendering.
        let lines = build_lines_w(app, chat_area.width as usize);
        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        let total_height = para.line_count(chat_area.width) as u32;
        let max_scroll = total_height.saturating_sub(chat_area.height as u32);
        // Store for use in handle_key (Up/PageUp need the current max_scroll).
        app.layout.max_scroll = max_scroll;
        // If a resize just occurred, restore scroll to the saved ratio.
        if let Some(ratio) = app.pending_scroll_ratio.take() {
            app.scroll = ((ratio * max_scroll as f64).round() as u32).min(max_scroll);
        }
        let scroll = if app.following {
            max_scroll
        } else {
            app.scroll.min(max_scroll)
        };
        // ratatui's scroll() takes (u16, u16); clamp to u16::MAX before casting.
        frame.render_widget(
            para.scroll((scroll.min(u16::MAX as u32) as u16, 0)),
            chat_area,
        );
    }

    // ── Status strip ──
    {
        let status_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        let spinner = SPINNER[app.spinner_tick];
        let mut slines: Vec<Line<'static>> = Vec::new();
        for entry in &app.status_log {
            if entry.done {
                let ms = entry.elapsed_ms.unwrap_or(0);
                let t = if ms < 1000 {
                    format!("{}ms", ms)
                } else {
                    format!("{:.1}s", ms as f64 / 1000.0)
                };
                let (icon, style) = if entry.is_error {
                    ("✗", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                } else {
                    ("✓", status_style)
                };
                slines.push(Line::from(vec![
                    Span::styled(format!(" {} ", icon), style),
                    Span::styled(tool_display_name(&entry.name).to_string(), style),
                    Span::styled(format!(" {}", entry.detail), status_style),
                    Span::styled(format!("  {}", t), status_style),
                ]));
            } else {
                let elapsed = entry.start.elapsed();
                let secs = elapsed.as_secs_f64();
                let t = if secs < 1.0 {
                    format!("{}ms", elapsed.as_millis())
                } else {
                    format!("{:.1}s", secs)
                };
                // Running state: single consistent amber color for all tools
                let running_style = Style::default()
                    .fg(Color::Rgb(200, 170, 80))
                    .add_modifier(Modifier::DIM);
                slines.push(Line::from(vec![
                    Span::styled(format!(" {} ", spinner), running_style),
                    Span::styled(tool_display_name(&entry.name).to_string(), running_style),
                    Span::styled(format!(" {}", entry.detail), status_style),
                    Span::styled(format!("  {}", t), status_style),
                ]));
            }
        }
        while slines.len() < 2 {
            slines.push(Line::raw(""));
        }
        frame.render_widget(Paragraph::new(slines), status_area);
    }

    // ── Queue strip ──
    // When the agent is thinking with queued items: show thinking step + queued list.
    // When idle with queued items: show queued list with count header.
    // When thinking alone: just the step text.
    {
        let mut queue_lines: Vec<Line> = Vec::new();

        let has_thinking = app.current_step.is_some();
        let has_queue = !app.queued.is_empty();

        if has_thinking {
            queue_lines.push(Line::from(vec![
                Span::styled(
                    "  ▶ ",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    truncate_chars(app.current_step.as_ref().unwrap(), 80),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ]));
        }

        if has_queue {
            if has_thinking {
                // Agent is active — compact divider before queued items
                queue_lines.push(Line::raw(""));
                queue_lines.push(Line::from(Span::styled(
                    "    queued:",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )));
            } else {
                // Agent is idle — header with count
                queue_lines.push(Line::from(Span::styled(
                    format!("    {} queued", app.queued.len()),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::DIM),
                )));
            }

            // Each queued item on its own line — dynamic based on queue_area height
            let header_lines = if has_thinking { 2 } else { 1 };
            let max_items = queue_area.height.saturating_sub(header_lines) as usize;
            let effective_max = if max_items > 0 { max_items } else { 5 };

            for (idx, item) in app.queued.iter().enumerate().take(effective_max) {
                let first_line = item.lines().next().unwrap_or(item.as_str());
                // Prefix is "      N. " — 6 spaces + number + ". "
                let prefix = format!("      {}. ", idx + 1);
                let prefix_len = prefix.chars().count();
                let max_width = queue_area.width as usize;
                let available = max_width.saturating_sub(prefix_len);
                let truncated = if first_line.chars().count() > available {
                    format!(
                        "{}…",
                        first_line.chars().take(available.saturating_sub(1)).collect::<String>()
                    )
                } else {
                    first_line.to_string()
                };

                queue_lines.push(Line::from(Span::styled(
                    format!("{}{}", prefix, truncated),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )));
            }

            let more_count = app.queued.len().saturating_sub(effective_max);
            if more_count > 0 {
                queue_lines.push(Line::from(Span::styled(
                    format!("    ... and {} more", more_count),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )));
            }
        }

        if queue_lines.is_empty() {
            queue_lines.push(Line::raw(""));
        }

        frame.render_widget(Paragraph::new(queue_lines), queue_area);
    }

    // ── Input box (always rendered, even when permission popup is showing) ──
    // Compute cursor position.  For multiline input, derive (row, col) from the
    // char offset; for single-line input use horizontal scroll as before.
    let input_visible_w = (input_area.width as usize).saturating_sub(4).max(1);
    let byte_at_cursor = char_byte_pos(&app.text_input.text, app.text_input.cursor);
    let before_cursor = &app.text_input.text[..byte_at_cursor];
    let is_multiline = app.text_input.text.contains('\n');
    let (cursor_row, cursor_col): (u16, u16) = if is_multiline {
        let row = before_cursor.matches('\n').count() as u16;
        let col = before_cursor
            .rfind('\n')
            .map(|p| app.text_input.text[p + 1..byte_at_cursor].chars().count())
            .unwrap_or_else(|| before_cursor.chars().count()) as u16;
        (row, col.min(input_visible_w as u16))
    } else {
        // Single-line: maintain horizontal scroll window.
        if app.text_input.cursor < app.text_input.scroll {
            app.text_input.scroll = app.text_input.cursor;
        } else if app.text_input.cursor >= app.text_input.scroll + input_visible_w {
            app.text_input.scroll = app.text_input.cursor - input_visible_w + 1;
        }
        (0, (app.text_input.cursor - app.text_input.scroll) as u16)
    };

    // Build the paragraph text.  Multiline: one ratatui Line per input line.
    // Single-line: horizontally-scrolled window as before.
    let max_visible_content_rows = input_h.saturating_sub(2) as usize; // minus top+bottom border
    let input_para_lines: Vec<Line<'static>> = if is_multiline {
        let all_lines: Vec<&str> = app.text_input.text.split('\n').collect();
        // Vertical scroll: keep the cursor line visible.
        let v_scroll = if cursor_row as usize >= max_visible_content_rows {
            cursor_row as usize - max_visible_content_rows + 1
        } else {
            0
        };
        all_lines
            .iter()
            .skip(v_scroll)
            .take(max_visible_content_rows)
            .map(|l| Line::raw(format!(" {}", l)))
            .collect()
    } else {
        let visible: String = app
            .text_input
            .text
            .chars()
            .skip(app.text_input.scroll)
            .take(input_visible_w)
            .collect();
        vec![Line::raw(format!(" {}", visible))]
    };

    // Always clear the input area first — prevents any bleed-through from overlapping widgets.
    frame.render_widget(Clear, input_area);

    if app.busy || app.pending_perm.is_some() || app.enhancing {
        let spinner = SPINNER[app.spinner_tick];
        let title_line = if app.pending_perm.is_some() {
            Line::from(vec![
                Span::styled("⏸", Style::default().fg(Color::LightMagenta)),
                Span::styled(
                    " waiting for permission… ",
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        } else if app.enhancing {
            Line::from(vec![
                Span::styled(format!("{} ", spinner), Style::default().fg(Color::Cyan)),
                Span::styled("✦ enhancing prompt…", Style::default().fg(Color::Cyan)),
            ])
        } else if !app.queued.is_empty() {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    "queued — Ctrl+Enter to interrupt".to_string(),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        } else if app.text_input.text.is_empty() {
            let elapsed_s = app.turn_start.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            let elapsed_hint = if elapsed_s >= 1 {
                format!(" {}s", elapsed_s)
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    format!(
                        "thinking…{}  (type a follow-up to queue, Ctrl+Enter to interrupt)",
                        elapsed_hint
                    ),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    "thinking…  Enter=queue  Ctrl+Enter=interrupt".to_string(),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        };
        let border_color = if app.enhancing {
            Color::Cyan
        } else {
            Color::LightMagenta
        };
        let block = Block::default()
            .title(title_line)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        if app.pending_perm.is_none() {
            frame.set_cursor_position((
                input_area.x + 2 + cursor_col,
                input_area.y + 1 + cursor_row.min(max_visible_content_rows as u16 - 1),
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
        let title_line = Line::from(vec![
            Span::styled("⏳ ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("Auto-resume in {countdown}  (Esc=cancel  /profile=switch provider)"),
                Style::default().fg(Color::Yellow),
            ),
        ]);
        let block = Block::default()
            .title(title_line)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        frame.set_cursor_position((
            input_area.x + 2 + cursor_col,
            input_area.y + 1 + cursor_row.min(max_visible_content_rows as u16 - 1),
        ));
    } else {
        let idle_title = Line::from(vec![Span::styled(
            if is_multiline {
                " Shift+Enter=newline  (Enter=send  Ctrl+Enter=interrupt  /help=commands) "
            } else {
                " Ask anything  (Enter=send  Shift+Enter=newline  /help=commands) "
            },
            Style::default().fg(TUI_SOFT_ACCENT),
        )]);
        let block = Block::default()
            .title(idle_title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TUI_SOFT_ACCENT));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        frame.set_cursor_position((
            input_area.x + 2 + cursor_col,
            input_area.y + 1 + cursor_row.min(max_visible_content_rows as u16 - 1),
        ));
    }

    // ── Hint line — hidden when terminal is very narrow ──
    if area.width >= 40 {
        let hint_dim = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
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
                format!("  [{}]  ", label),
                Style::default()
                    .fg(TUI_SOFT_ACCENT)
                    .add_modifier(Modifier::DIM),
            ));
        }
        hint_spans.extend([
            Span::styled("  Enter", Style::default().fg(Color::DarkGray)),
            Span::styled(" send  ", hint_dim),
            Span::styled("Shift+Enter", Style::default().fg(Color::DarkGray)),
            Span::styled(" newline  ", hint_dim),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" clear  ", hint_dim),
            Span::styled("↑↓", Style::default().fg(Color::DarkGray)),
            Span::styled(" history  ", hint_dim),
            Span::styled("PgUp/PgDn", Style::default().fg(Color::DarkGray)),
            Span::styled(" scroll  ", hint_dim),
            Span::styled("/settings", Style::default().fg(Color::DarkGray)),
            Span::styled(" settings  ", hint_dim),
            Span::styled("/help", Style::default().fg(Color::DarkGray)),
            Span::styled(" commands  ", hint_dim),
            Span::styled("Ctrl+/", Style::default().fg(Color::DarkGray)),
            Span::styled(" stop agent  ", hint_dim),
            Span::styled("Ctrl+C", Style::default().fg(Color::DarkGray)),
            Span::styled(" quit  ", hint_dim),
            Span::styled("Ctrl+L", Style::default().fg(Color::DarkGray)),
            Span::styled(" refresh  ", hint_dim),
        ]);
        // Scroll position indicator when not following.
        if app.layout.max_scroll > 0 && !app.following {
            let pct = (app.scroll * 100 / app.layout.max_scroll).min(100);
            hint_spans.push(Span::styled(
                format!("  ↑ {}%", pct),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
        }
        let hint_spans = fit_spans(hint_spans, hint_area.width as usize);
        let hint = Paragraph::new(Line::from(hint_spans));
        frame.render_widget(hint, hint_area);
    }

    // ── "↓ new messages" scroll indicator ──
    if !app.following && app.layout.max_scroll > app.scroll {
        let unread_hint = Span::styled(
            "  ↓ new messages  PgDn ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
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
        const VISIBLE: usize = 12;

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
        let cmd_col_w = 20usize;
        let popup_rect = popup_above_input(input_area, popup_h, popup_w);
        let desc_w = (popup_rect.width as usize).saturating_sub(cmd_col_w + 3);

        let mut content: Vec<Line<'static>> = visible_slice
            .iter()
            .map(|row| match row {
                CompletionRow::Header(section) => Line::from(Span::styled(
                    format!("  ── {} ", section),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )),
                CompletionRow::Cmd {
                    flat_idx,
                    cmd,
                    desc,
                } => {
                    let selected = app.selected_cmd == Some(*flat_idx);
                    let marker = if selected { "▶" } else { " " };
                    let desc_str = if desc.len() > desc_w {
                        format!("{}…", &desc[..desc_w.saturating_sub(1)])
                    } else {
                        desc.to_string()
                    };
                    modal_row_two_col(
                        format!("{} {:<width$}", marker, cmd, width = cmd_col_w - 2),
                        format!(" {}", desc_str),
                        Color::Cyan,
                        Color::DarkGray,
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
        let title = format!(" {} commands ", n_cmds);
        let hint = " ↑↓ navigate · Tab/Enter select · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, TUI_SOFT_ACCENT)),
            popup_rect,
        );
    }

    // ── Session picker ───────────────────────────────────────────────────────
    if let Some(ref picker) = app.session_picker {
        const VISIBLE: usize = 12;
        let filtered: Vec<(usize, &clido_storage::SessionSummary)> =
            picker.picker.filtered_items().collect();
        let n_rows = filtered.len().min(VISIBLE) as u16;
        // border(2) + header(1) + blank(1) + filter(1) + rows = n_rows + 5
        let popup_h = (n_rows + 5).min(input_area.y.saturating_sub(2));
        let popup_h = popup_h.min(area.height.saturating_sub(4));
        let popup_h = (n_rows + 5).min(popup_h.max(6));
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);

        let inner_w = popup_rect.width.saturating_sub(4) as usize;
        // fixed cols: marker(2) id(8) sep(2) msg(3) sep(2) cost(6) sep(2) date(11) sep(2) = 38
        let preview_w = inner_w.saturating_sub(38).max(8);

        let mut content: Vec<Line<'static>> = Vec::new();
        // Filter line
        if !picker.picker.filter.text.is_empty() {
            content.push(filter_indicator_line(&picker.picker.filter.text));
        }
        content.push(Line::from(vec![Span::styled(
            format!(
                "  {:<8}  {:<5}  {:<6}  {:<11}  {}",
                "id", "turns", "cost", "when", "preview"
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));
        content.push(Line::from(vec![Span::styled(
            "  ────────  ─────  ──────  ───────────  ────────────────────".to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));

        let end = (picker.picker.scroll_offset + VISIBLE).min(filtered.len());
        for (di, (_orig_idx, s)) in filtered[picker.picker.scroll_offset..end]
            .iter()
            .enumerate()
        {
            let selected = picker.picker.scroll_offset + di == picker.picker.selected;
            let bg = if selected {
                TUI_SELECTION_BG
            } else {
                Color::Reset
            };
            let fg = if selected { Color::White } else { Color::Gray };
            let id_short = &s.session_id[..s.session_id.len().min(8)];
            let date_str = relative_time(&s.start_time);
            let preview_str: String = s.preview.chars().take(preview_w).collect();
            let marker = if selected { "▶ " } else { "  " };
            content.push(Line::from(vec![Span::styled(
                format!(
                    "{}{:<8}  {:>5}  ${:<5.2}  {:<11}  {}",
                    marker, id_short, s.num_turns, s.total_cost_usd, date_str, preview_str
                ),
                Style::default().fg(fg).bg(bg),
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
        let picker_title = format!(" Sessions — {} total ", total);
        let hint = " ↑↓ navigate · Enter resume · d delete · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&picker_title, hint, Color::Cyan)),
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
                Style::default().fg(Color::White),
            )]),
            Line::from(vec![Span::styled(
                format!(
                    "    {:<32}  {:<12}  {:>8}  {:>8}  {:>6}  {}",
                    "model", "provider", "$/1M in", "$/1M out", "ctx", "role"
                ),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
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
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
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
                            .fg(Color::Yellow)
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
                            .fg(Color::Cyan)
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
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    )]));
                }

                let selected = picker.scroll_offset + di == picker.selected;
                let bg = if selected {
                    TUI_SELECTION_BG
                } else {
                    Color::Reset
                };
                let fg = if selected { Color::White } else { Color::Gray };
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
        let hint = " ↑↓ navigate · Enter select · Ctrl+S save default · f fav · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, Color::Magenta)),
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
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
        content.push(Line::raw(""));

        if filtered.is_empty() {
            content.push(Line::from(Span::styled(
                if picker.picker.filter.text.is_empty() {
                    "  no profiles — press n to create one"
                } else {
                    "  no matches"
                },
                Style::default().fg(Color::DarkGray),
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
            let fg = if selected { Color::White } else { Color::Gray };
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
        let hint = " ↑↓ navigate · Enter switch · n new · e edit · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, Color::Cyan)),
            popup_rect,
        );
    }

    // ── Profile overview/editor overlay ──────────────────────────────────────
    if let Some(ref st) = app.profile_overlay {
        render_profile_overlay(frame, area, input_area, st);
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
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(" Feedback: ", Style::default().fg(Color::Yellow)),
                    Span::styled(fb.as_str(), Style::default().fg(Color::White)),
                    Span::styled("█", Style::default().fg(Color::Yellow)),
                ]),
                Line::raw(""),
                Line::from(vec![Span::styled(
                    "  Enter to send feedback   Esc to go back",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )]),
            ];
            frame.render_widget(Clear, popup_rect);
            frame.render_widget(
                Paragraph::new(content).block(modal_block(
                    " Explain why you are denying this ",
                    Color::Red,
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
                Style::default().fg(Color::DarkGray),
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
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<28}", label),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", hint), Style::default().fg(Color::DarkGray)),
                ]));
            } else {
                content.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        format!("{:<28}", label),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("  {}", hint),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    ),
                ]));
            }
        }
        content.push(Line::from(vec![Span::styled(
            "  ↑↓/1-5 select   Enter confirm   Esc deny",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));

        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block(
                &format!(" Allow {}? ", tool_display_name(&perm.tool_name)),
                Color::Yellow,
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
                            Style::default().fg(Color::White),
                        )])
                    })
                    .collect();
                if !recovery_lines.is_empty() {
                    content.push(Line::raw(""));
                    for l in recovery_lines {
                        content.push(Line::from(vec![Span::styled(
                            format!("  → {}", l),
                            Style::default().fg(Color::Cyan),
                        )]));
                    }
                }
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  [ OK ]  (Enter / Esc / Space)",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]));
                let hint_title = if e.max_scroll > 0 {
                    format!(" {} — ↑↓ scroll · Enter/Esc close ", e.title)
                } else {
                    format!(" {} ", e.title)
                };
                frame.render_widget(Clear, popup_rect);
                frame.render_widget(
                    Paragraph::new(content)
                        .block(modal_block(&hint_title, Color::Red))
                        .scroll((e.scroll_offset as u16, 0)),
                    popup_rect,
                );
            }
            OverlayKind::ReadOnly(r) => {
                let mut content: Vec<Line<'static>> = Vec::new();
                if r.lines.is_empty() {
                    content.push(Line::from(vec![Span::styled(
                        "  (empty)".to_string(),
                        Style::default().fg(Color::DarkGray),
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
                                        .fg(Color::Cyan)
                                        .add_modifier(Modifier::BOLD),
                                )]));
                            }
                            for line in text.lines() {
                                content.push(Line::from(vec![Span::styled(
                                    format!("    {}", line),
                                    Style::default().fg(Color::Gray),
                                )]));
                            }
                            content.push(Line::raw(""));
                        }
                    }
                }
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  [ Close ]  (Enter / Esc)".to_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
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
                        .block(modal_block(&hint_text, Color::Cyan))
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
                        Style::default().fg(Color::White),
                    )]));
                    content.push(Line::raw(""));
                }
                for (i, choice) in c.choices.iter().enumerate() {
                    let marker = if i == c.selected { "▸ " } else { "  " };
                    let style = if i == c.selected {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    content.push(Line::from(vec![Span::styled(
                        format!("  {}{}", marker, choice.label),
                        style,
                    )]));
                }
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  ↑↓ Navigate  Enter Select  Esc Cancel",
                    Style::default().fg(Color::DarkGray),
                )]));
                let popup_h = ((content.len() as u16) + 2).min(area.height.saturating_sub(4));
                let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
                frame.render_widget(Clear, popup_rect);
                frame.render_widget(
                    Paragraph::new(content)
                        .block(modal_block(&format!(" {} ", c.title), Color::Yellow)),
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
        let toast_bg = Color::Indexed(238); // medium-dark gray
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

    let sel_style = Style::default()
        .bg(Color::Indexed(24)) // deep blue background
        .fg(Color::White);

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
            end_col + 1 // inclusive end
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
///
/// After this, `Vec<Line>` index == visual row, which means mouse
/// coordinates (post-wrap) map 1:1 to vector indices. This is critical
/// for selection highlighting and text extraction.
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
        // Slow path: split spans across multiple output lines.
        let mut cur_spans: Vec<Span<'static>> = Vec::new();
        let mut col = 0usize;
        for span in line.spans {
            let style = span.style;
            let mut remaining: &str = span.content.as_ref();
            while !remaining.is_empty() {
                let avail = width.saturating_sub(col);
                if avail == 0 {
                    out.push(Line::from(std::mem::take(&mut cur_spans)));
                    col = 0;
                    continue;
                }
                // Take as many chars as fit within `avail` columns.
                let (chunk, chunk_w) = take_cols(remaining, avail);
                if chunk.is_empty() {
                    // Single character wider than available space — force wrap.
                    out.push(Line::from(std::mem::take(&mut cur_spans)));
                    col = 0;
                    continue;
                }
                cur_spans.push(Span::styled(chunk.to_string(), style));
                col += chunk_w;
                remaining = &remaining[chunk.len()..];
                if col >= width && !remaining.is_empty() {
                    out.push(Line::from(std::mem::take(&mut cur_spans)));
                    col = 0;
                }
            }
        }
        if !cur_spans.is_empty() {
            out.push(Line::from(cur_spans));
        }
    }
    out
}

/// Display width of a string (number of terminal columns).
fn unicode_display_width(s: &str) -> usize {
    // Use the same logic ratatui uses internally.
    s.chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// Take the longest prefix of `s` that fits in `max_cols` terminal columns.
/// Returns (prefix_str, display_width_of_prefix).
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
    // Compute a cheap hash of the current messages state.
    // Key: (content_hash, width) where content_hash covers message count + last message text.
    let msg_count = app.messages.len();
    let content_hash = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        msg_count.hash(&mut h);
        // Include last message content so new streaming tokens invalidate the cache.
        if let Some(last) = app.messages.last() {
            std::mem::discriminant(last).hash(&mut h);
            match last {
                ChatLine::User(t) | ChatLine::Assistant(t) | ChatLine::Thinking(t) => {
                    t.hash(&mut h);
                }
                _ => {}
            }
        }
        h.finish()
    };
    let cache_key = (content_hash, width);

    // Evict stale entries when the message list changes (shrinks after /compact,
    // or grows with new messages) to prevent unbounded cache growth.
    if msg_count != app.render_cache_msg_count {
        app.render_cache.clear();
    }
    app.render_cache_msg_count = msg_count;

    let result = if let Some(cached) = app.render_cache.get(&cache_key) {
        cached.clone()
    } else {
        // Hard cap: evict all if too many entries (safety net against memory leaks).
        if app.render_cache.len() >= 512 {
            app.render_cache.clear();
        }

        let built = build_lines_w_uncached(app, width);
        // Pre-wrap so each Line fits within `width` columns.
        // After this, Vec<Line> index == visual row, which makes mouse
        // selection coordinates map 1:1 to vector indices.
        let wrapped = wrap_styled_lines(built, width);
        app.render_cache.insert(cache_key, wrapped.clone());
        wrapped
    };

    // Keep a plain-text snapshot of rendered lines so `get_selected_text()`
    // can resolve selection coordinates (which live in rendered-line space).
    app.rendered_line_texts = result
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect();

    // Apply selection highlighting to visible lines
    if app.selection.active {
        let visible_height = (app.layout.chat_area_y.1 - app.layout.chat_area_y.0) as usize;
        apply_selection_highlight(&result, &app.selection, app.scroll as usize, visible_height)
    } else {
        result
    }
}

pub(super) fn build_lines_w_uncached(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for msg in &app.messages {
        match msg {
            ChatLine::User(text) => {
                // User messages: bold label, clean content — like a command in a REPL.
                out.push(Line::from(vec![Span::styled(
                    "you",
                    Style::default()
                        .fg(TUI_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )]));
                out.extend(render_markdown(text, width));
                out.push(Line::raw(""));
            }
            ChatLine::Assistant(text) => {
                // Assistant: show brand + model name as dim signature.
                let header = if app.model.is_empty() {
                    "clido".to_string()
                } else {
                    format!("clido · {}", app.model)
                };
                out.push(Line::from(Span::styled(
                    header,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )));
                out.extend(render_markdown(text, width));
                out.push(Line::raw(""));
            }
            ChatLine::Thinking(text) => {
                // Thinking lines: dim, left-indented with a subtle `…` indicator.
                for part in text.lines() {
                    out.push(Line::from(vec![
                        Span::styled(
                            " ··· ",
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::DIM),
                        ),
                        Span::styled(
                            part.to_string(),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::DIM),
                        ),
                    ]));
                }
            }
            ChatLine::ToolCall {
                name,
                detail,
                done,
                is_error,
                ..
            } => {
                // Tool calls as "execution events":
                //   · read     crates/cli/src/main.rs
                //   ✓ write    crates/cli/src/main.rs  (+12 −3)
                //   ✗ bash     make test
                let color = tool_color(name, *done, *is_error);
                let icon = if *is_error {
                    "✗"
                } else if *done {
                    "✓"
                } else {
                    "·"
                };
                let display_name = tool_display_name(name).to_string();
                let dim = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM);
                let style = Style::default().fg(color);

                if detail.is_empty() {
                    out.push(Line::from(vec![
                        Span::styled(format!(" {} ", icon), style),
                        Span::styled(display_name, style),
                    ]));
                } else {
                    out.push(Line::from(vec![
                        Span::styled(format!(" {} ", icon), style),
                        Span::styled(display_name, style),
                        Span::styled(format!("  {}", detail.clone()), dim),
                    ]));
                }
            }
            ChatLine::Diff(text) => {
                out.extend(diff::render_diff(text, width));
            }
            ChatLine::Info(text) => {
                out.push(Line::from(vec![Span::styled(
                    if text.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", text)
                    },
                    Style::default().fg(Color::DarkGray),
                )]));
            }
            ChatLine::Section(text) => {
                // Section separator: thin line + label
                out.push(Line::raw(""));
                out.push(Line::from(vec![
                    Span::styled("─ ", Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)),
                    Span::styled(
                        text.clone(),
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                out.push(Line::raw(""));
            }
            ChatLine::WelcomeBrand | ChatLine::WelcomeSplash => {
                // Skipped — no longer displayed
            }
        }
    }
    out
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
    // Available content width: subtract 4 chars for left margin / padding.
    let content_w = width.saturating_sub(4).max(20);

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
                        HeadingLevel::H1 => "█ ",
                        HeadingLevel::H2 => "▌ ",
                        HeadingLevel::H3 => "▸ ",
                        _ => "  ",
                    };
                    cur_spans.push(Span::styled(
                        prefix.to_string(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ));
                    style_stack.push(
                        Style::default()
                            .fg(Color::Cyan)
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
                    if lang.is_empty() {
                        out.push(Line::from(vec![Span::styled(
                            "  ┌── code ──────────────────────────",
                            Style::default().fg(TUI_CODE_BORDER),
                        )]));
                    } else {
                        out.push(Line::from(vec![Span::styled(
                            format!("  ┌─ {} ", lang),
                            Style::default().fg(TUI_CODE_LANG),
                        )]));
                    }
                    // Code content uses code block background
                    style_stack.push(
                        Style::default()
                            .fg(Color::Rgb(200, 210, 220))
                            .bg(TUI_CODE_BG),
                    );
                }
                Tag::List(_) => {
                    list_depth += 1;
                }
                Tag::Item => {
                    flush!();
                    let indent = "  ".repeat(list_depth.saturating_sub(1) as usize);
                    cur_spans.push(Span::styled(
                        format!("{}• ", indent),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
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
                            "▌ ".repeat(bq_depth as usize),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    let style = style_stack.last().copied().unwrap_or_default();
                    cur_spans.push(Span::styled(t.to_string(), style));
                }
            }
            Event::Code(t) => {
                // Inline code — always use yellow dim style, never inherit parent style.
                if in_table_cell {
                    current_cell.push_str(&t);
                } else {
                    cur_spans.push(Span::styled(
                        t.to_string(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::DIM),
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
                out.push(Line::from(vec![Span::styled(
                    "─".repeat(content_w.min(72)),
                    Style::default().fg(Color::DarkGray),
                )]));
                out.push(Line::raw(""));
            }
            Event::TaskListMarker(checked) => {
                let marker = if checked { "☑ " } else { "☐ " };
                cur_spans.push(Span::styled(
                    marker.to_string(),
                    Style::default().fg(Color::Cyan),
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

    let gray = Style::default().fg(Color::DarkGray);
    let hdr_style = Style::default()
        .fg(Color::Cyan)
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
