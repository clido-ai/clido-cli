use std::hash::{Hash, Hasher};

use pulldown_cmark::Parser;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use clido_planner::{Complexity, Plan, TaskStatus};

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

    // Line 1: brand · version · provider/model · profile · reviewer
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
        Span::styled(
            format!("  v{}  ", version),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            if app.per_turn_prev_model.is_some() {
                format!("{}  {}⁺", app.provider, app.model)
            } else {
                format!("{}  {}", app.provider, app.model)
            },
            dim,
        ),
        Span::styled(format!("  [{}]", app.current_profile), dim),
    ];
    if let Some(ref title) = app.session_title {
        hline1.push(Span::styled(
            format!("  — {}", title),
            Style::default()
                .fg(Color::Rgb(180, 200, 230))
                .add_modifier(Modifier::ITALIC),
        ));
    }
    if app.reviewer_configured {
        let (dot, color) = if app.reviewer_enabled.load(Ordering::Relaxed) {
            ("●", Color::Green)
        } else {
            ("○", Color::DarkGray)
        };
        hline1.push(Span::styled(
            format!("  reviewer {}", dot),
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
            format!("  {}", short)
        },
        dim,
    )];
    if app.stats.session_total_cost_usd > 0.0 {
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

        hline2.push(Span::styled(
            format!(
                "   session: ${:.4}  {}{}",
                app.stats.session_total_cost_usd, tok_str, ctx_str
            ),
            dim,
        ));
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
    let input_h = (input_line_count as u16 + 2).min(12);
    let (hint_h, status_h) = if area.width < 40 { (0, 0) } else { (1, 2) };
    let [header_area, chat_area, status_area, queue_area, hint_area, input_area] =
        Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Min(0),
            Constraint::Length(status_h),
            Constraint::Length(1),
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
    if is_welcome_only(app) {
        render_welcome(frame, app, chat_area);
    } else {
        // Use ratatui's own line_count() so the scroll calculation matches actual rendering.
        let lines = build_lines_w(app, chat_area.width as usize);
        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        let total_height = para.line_count(chat_area.width) as u32;
        let max_scroll = total_height.saturating_sub(chat_area.height as u32);
        // Store for use in handle_key (Up/PageUp need the current max_scroll).
        app.max_scroll = max_scroll;
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
            let (icon, style, elapsed_str) = if entry.done {
                let ms = entry.elapsed_ms.unwrap_or(0);
                let t = format!(" {}ms", ms);
                if entry.is_error {
                    (
                        "✗",
                        Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                        t,
                    )
                } else {
                    ("✓", status_style, t)
                }
            } else {
                let elapsed = entry.start.elapsed();
                let secs = elapsed.as_secs_f64();
                let t = if secs < 1.0 {
                    format!(" {:.0}ms", elapsed.as_millis())
                } else {
                    format!(" {:.1}s", secs)
                };
                let running_color = tool_color(&entry.name, false, false);
                (
                    spinner,
                    Style::default()
                        .fg(running_color)
                        .add_modifier(Modifier::DIM),
                    t,
                )
            };
            slines.push(Line::from(vec![
                Span::styled(format!("  {} ", icon), style),
                Span::styled(tool_display_name(&entry.name).to_string(), style),
                Span::styled(format!("  {}", entry.detail), status_style),
                Span::styled(elapsed_str, status_style),
            ]));
        }
        while slines.len() < 2 {
            slines.push(Line::raw(""));
        }
        frame.render_widget(Paragraph::new(slines), status_area);
    }

    // ── Queue strip ──
    {
        let queue_line = if !app.queued.is_empty() {
            let n = app.queued.len();
            let first = app.queued.front().unwrap();
            let preview = if first.chars().count() > 50 {
                format!("{}…", first.chars().take(50).collect::<String>())
            } else {
                first.clone()
            };
            let label = if n == 1 {
                "  ↻ 1 queued  ".to_string()
            } else {
                format!("  ↻ {} queued  ", n)
            };
            Line::from(vec![
                Span::styled(
                    label,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    format!("\"{}\"", preview),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else if let Some(ref step) = app.current_step {
            Line::from(vec![
                Span::styled(
                    "  ▶ ",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    truncate_chars(step, 80),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::raw("")
        };
        frame.render_widget(Paragraph::new(queue_line), queue_area);
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

    if app.busy || app.pending_perm.is_some() {
        let spinner = SPINNER[app.spinner_tick];
        let title_line = if app.pending_perm.is_some() {
            Line::from(vec![
                Span::styled("⏸", Style::default().fg(Color::LightMagenta)),
                Span::styled(
                    " waiting for permission… ",
                    Style::default().fg(Color::LightMagenta),
                ),
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
        let block = Block::default()
            .title(title_line)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightMagenta));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        if app.pending_perm.is_none() {
            frame.set_cursor_position((
                input_area.x + 2 + cursor_col,
                input_area.y + 1 + cursor_row.min(max_visible_content_rows as u16 - 1),
            ));
        }
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
        } else if app.role_picker.is_some() {
            Some("Roles".into())
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
            Span::styled("Shift+select", Style::default().fg(Color::DarkGray)),
            Span::styled(" copy text  ", hint_dim),
        ]);
        // Scroll position indicator when not following.
        if app.max_scroll > 0 && !app.following {
            let pct = (app.scroll * 100 / app.max_scroll).min(100);
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
    if !app.following && app.max_scroll > app.scroll {
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
                    "  {:<2} {:<32}  {:<12}  {:>8}  {:>8}  {:>6}  {}",
                    "  ", "model", "provider", "$/1M in", "$/1M out", "ctx k", "alias"
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
                let fav = if m.is_favorite { "★" } else { "  " };
                let ctx = m
                    .context_k
                    .map(|k| format!("{:>4}k", k))
                    .unwrap_or_else(|| "    ?".into());
                let role = m.role.as_deref().unwrap_or("");
                let id_display: String = m.id.chars().take(32).collect();
                let prov_display: String = m.provider.chars().take(12).collect();
                content.push(Line::from(vec![Span::styled(
                    format!(
                        "  {} {:<32}  {:<12}  {:>8.2}  {:>8.2}  {}  {}",
                        fav, id_display, prov_display, m.input_mtok, m.output_mtok, ctx, role
                    ),
                    Style::default().fg(fg).bg(bg),
                )]));
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

    // ── Role picker popup ─────────────────────────────────────────────────────
    if let Some(ref picker) = app.role_picker {
        const VISIBLE: usize = 10;
        let filtered: Vec<(usize, &(String, String))> = picker.picker.filtered_items().collect();
        let n_rows = filtered.len().min(VISIBLE) as u16;
        let popup_h = (n_rows + 5).min(area.height.saturating_sub(4)).max(5);
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
        let inner_w = popup_rect.width.saturating_sub(4) as usize;

        let mut content: Vec<Line<'static>> = Vec::new();
        if !picker.picker.filter.text.is_empty() {
            content.push(filter_indicator_line(&picker.picker.filter.text));
        }
        content.push(Line::from(Span::styled(
            format!("  {:<16}  {}", "role", "model"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
        content.push(Line::raw(""));

        let end = (picker.picker.scroll_offset + VISIBLE).min(filtered.len());
        for (di, (_orig_idx, (role, model))) in filtered[picker.picker.scroll_offset..end]
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
            let marker = if selected { "▶" } else { " " };
            let model_display: String = model.chars().take(inner_w.saturating_sub(20)).collect();
            content.push(Line::from(Span::styled(
                format!("{} {:<16}  {}", marker, role, model_display),
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

        let title = format!(" Roles — {} ", filtered.len());
        let hint = " ↑↓ navigate · Enter switch model · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, Color::Yellow)),
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
                let popup_h =
                    ((total_lines as u16) + 2).min(area.height.saturating_sub(4));
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

    // ── Toast notifications (top-right, auto-dismiss) ────────────────────────
    app.expire_toasts();
    if !app.toasts.is_empty() {
        let max_w = area.width.saturating_sub(4).min(50) as usize;
        for (i, toast) in app.toasts.iter().enumerate().take(3) {
            let msg: String = toast.message.chars().take(max_w).collect();
            let w = (msg.len() as u16 + 4).min(area.width);
            let y = area.y + 1 + (i as u16) * 2;
            if y + 1 >= area.height {
                break;
            }
            let toast_rect = Rect {
                x: area.width.saturating_sub(w + 1),
                y,
                width: w,
                height: 1,
            };
            frame.render_widget(Clear, toast_rect);
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    format!(" {} ", msg),
                    Style::default().fg(toast.style).bg(Color::DarkGray),
                )])),
                toast_rect,
            );
        }
    }
}

pub(super) fn render_plan_editor(frame: &mut Frame, app: &App, area: Rect) {
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
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let notes_style = if form.focused_field == TaskEditField::Notes {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let comp_style = if form.focused_field == TaskEditField::Complexity {
            Style::default()
                .fg(Color::White)
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

pub(super) fn render_plan_text_editor(frame: &mut Frame, app: &App, area: Rect) {
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

// ── Welcome panel ─────────────────────────────────────────────────────────────

pub(super) fn is_welcome_only(app: &App) -> bool {
    app.messages.len() == 1 && matches!(app.messages[0], ChatLine::WelcomeSplash)
}

/// Centered welcome panel rendered when no conversation has started yet.
pub(super) fn render_welcome(frame: &mut Frame, app: &App, area: Rect) {
    let muted = Style::default().fg(Color::Rgb(110, 125, 150));
    let soft = Style::default().fg(Color::Rgb(185, 195, 215));
    let accent = Style::default()
        .fg(TUI_SOFT_ACCENT)
        .add_modifier(Modifier::BOLD);

    // Shorten workdir to ~/...
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = app.workspace_root.display().to_string();
    let workdir = if !home.is_empty() && raw.starts_with(&home) {
        format!("~{}", &raw[home.len()..])
    } else {
        raw
    };

    let content: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(Span::styled(format!("    {}", workdir), muted)),
        Line::raw(""),
        Line::from(vec![
            Span::styled("    profile  ".to_string(), muted),
            Span::styled(
                app.current_profile.clone(),
                soft.add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ".to_string(), muted),
            Span::styled(app.model.clone(), soft),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "    /help   /model   /role   /workdir".to_string(),
            accent,
        )),
        Line::from(Span::styled(
            "    Enter=send  ·  Ctrl+/=stop  ·  Ctrl+Enter=interrupt+send".to_string(),
            muted,
        )),
        Line::raw(""),
    ];

    let border_color = Color::Rgb(55, 70, 95);
    let panel_w = 64u16.min(area.width.saturating_sub(4));
    let panel_h = (content.len() as u16 + 2).min(area.height.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(panel_w) / 2;
    let y = area.y + area.height.saturating_sub(panel_h) / 2;
    let panel_area = Rect::new(x, y, panel_w, panel_h);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "cli".to_string(),
                Style::default()
                    .fg(Color::Rgb(210, 220, 240))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                ";".to_string(),
                Style::default()
                    .fg(TUI_SOFT_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "do".to_string(),
                Style::default()
                    .fg(Color::Rgb(210, 220, 240))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .title_alignment(Alignment::Left);

    let inner = block.inner(panel_area);
    frame.render_widget(block, panel_area);
    frame.render_widget(Paragraph::new(content), inner);
}

// ── Profile overlay renderer ──────────────────────────────────────────────────

pub(super) fn render_profile_overlay(
    frame: &mut Frame,
    area: Rect,
    input_area: Rect,
    st: &ProfileOverlayState,
) {
    let popup_h = area.height.saturating_sub(6).max(12);
    let popup_w = area.width.saturating_sub(8).min(80);
    let popup_rect = popup_above_input(input_area, popup_h, popup_w);
    frame.render_widget(Clear, popup_rect);

    let inner = Rect {
        x: popup_rect.x + 1,
        y: popup_rect.y + 1,
        width: popup_rect.width.saturating_sub(2),
        height: popup_rect.height.saturating_sub(2),
    };
    let [content_area, hint_area] =
        ratatui::layout::Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    match &st.mode {
        ProfileOverlayMode::Overview | ProfileOverlayMode::EditField(_) => {
            render_profile_overview(frame, popup_rect, content_area, hint_area, st)
        }
        ProfileOverlayMode::Creating { step } => {
            render_profile_create(frame, popup_rect, content_area, hint_area, st, step)
        }
        ProfileOverlayMode::PickingProvider { .. } => {
            render_profile_provider_picker(frame, popup_rect, content_area, hint_area, st)
        }
        ProfileOverlayMode::PickingModel { .. } => {
            render_profile_model_picker(frame, popup_rect, content_area, hint_area, st)
        }
    }
}

pub(super) fn render_profile_overview(
    frame: &mut Frame,
    popup_rect: Rect,
    content_area: Rect,
    hint_area: Rect,
    st: &ProfileOverlayState,
) {
    let title = if st.is_new {
        " New Profile ".to_string()
    } else {
        format!(" Profile: {} ", st.name)
    };
    frame.render_widget(
        Block::default()
            .title(title.as_str())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        popup_rect,
    );

    let editing = matches!(&st.mode, ProfileOverlayMode::EditField(_));
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::raw(""));

    // cursor_idx tracks which editable field we're on as we iterate PROFILE_FIELDS.
    // Section headers don't consume a cursor index.
    let mut cursor_idx: usize = 0;
    // line_count tracks rendered lines so we can place the text cursor correctly.
    let mut line_count: u16 = 1; // starts at 1 for the leading blank
    let mut editing_line_y: u16 = 0; // Y position of the value row for the active edit field

    for (key, label) in PROFILE_FIELDS.iter() {
        if *key == "__section__" {
            // Non-editable section divider
            lines.push(Line::from(Span::styled(
                format!("  {}", label),
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::DIM | Modifier::BOLD),
            )));
            lines.push(Line::raw(""));
            line_count += 2;
            continue;
        }

        let field_cursor = cursor_idx;
        cursor_idx += 1;

        let selected = st.cursor == field_cursor && !editing;
        let is_editing = matches!(&st.mode, ProfileOverlayMode::EditField(f) if {
            let expected = match field_cursor {
                0 => ProfileEditField::Provider,
                1 => ProfileEditField::ApiKey,
                2 => ProfileEditField::Model,
                3 => ProfileEditField::BaseUrl,
                4 => ProfileEditField::WorkerProvider,
                5 => ProfileEditField::WorkerModel,
                6 => ProfileEditField::ReviewerProvider,
                7 => ProfileEditField::ReviewerModel,
                _ => ProfileEditField::None,
            };
            *f == expected
        });

        // Label row
        lines.push(Line::from(Span::styled(
            format!("  {}", label),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
        line_count += 1;

        // Value row
        let display_value = if *key == "api_key" {
            if is_editing {
                let len = st.input.len();
                if len == 0 {
                    String::new()
                } else {
                    format!("{} ({} chars)", "•".repeat(len.min(30)), len)
                }
            } else {
                st.masked_api_key()
            }
        } else if is_editing {
            st.input.clone()
        } else {
            let raw = match field_cursor {
                0 => st.provider.clone(),
                1 => st.masked_api_key(),
                2 => st.model.clone(),
                3 => st.base_url.clone(),
                4 => st.worker_provider.clone(),
                5 => st.worker_model.clone(),
                6 => st.reviewer_provider.clone(),
                7 => st.reviewer_model.clone(),
                _ => String::new(),
            };
            if raw.is_empty() {
                "—".to_string()
            } else {
                raw
            }
        };

        let cursor_span = if selected {
            Span::styled(
                " ▶ ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        };

        let value_style = if is_editing {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };

        let mut spans = vec![cursor_span, Span::styled(display_value, value_style)];

        if is_editing {
            spans.push(Span::styled("▌", Style::default().fg(Color::Yellow)));
            spans.push(Span::styled(
                "  Esc=cancel  Enter=save",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            editing_line_y = content_area.y + line_count;
        } else if selected {
            spans.push(Span::styled(
                "  (Enter to edit)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
        }

        lines.push(Line::from(spans));
        lines.push(Line::raw(""));
        line_count += 2;
    }

    // Status message
    if let Some(ref msg) = st.status {
        lines.push(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Green),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );

    // Hint footer
    let hint = if editing {
        "Type to edit  ·  Enter=save  ·  Esc=cancel"
    } else {
        "↑↓ navigate  ·  Enter=edit field  ·  Ctrl+S=save all  ·  Esc=close"
    };
    frame.render_widget(
        Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        hint_area,
    );

    // Place terminal cursor on the value row of the field being edited.
    if matches!(&st.mode, ProfileOverlayMode::EditField(_)) && editing_line_y > 0 {
        let cursor_x = content_area.x + 3 + st.input_cursor as u16; // 3 = " ▶ " prefix width
        if editing_line_y < content_area.y + content_area.height {
            frame.set_cursor_position((cursor_x, editing_line_y));
        }
    }
}

pub(super) fn render_profile_create(
    frame: &mut Frame,
    popup_rect: Rect,
    content_area: Rect,
    hint_area: Rect,
    st: &ProfileOverlayState,
    step: &ProfileCreateStep,
) {
    match step {
        ProfileCreateStep::Provider => {
            render_profile_provider_picker(frame, popup_rect, content_area, hint_area, st);
            return;
        }
        ProfileCreateStep::Model => {
            render_profile_model_picker(frame, popup_rect, content_area, hint_area, st);
            return;
        }
        _ => {}
    }

    frame.render_widget(
        Block::default()
            .title(" New Profile ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        popup_rect,
    );

    let (step_num, step_total, step_label, current_value, placeholder) = match step {
        ProfileCreateStep::Name => (
            1,
            4,
            "Profile name",
            &st.name,
            "optional — Enter to auto-generate from provider",
        ),
        ProfileCreateStep::Provider => (2, 4, "Provider", &st.provider, "select a provider"),
        ProfileCreateStep::ApiKey => (3, 4, "API key", &st.api_key, "paste your key here"),
        ProfileCreateStep::Model => (
            4,
            4,
            "Default model",
            &st.model,
            "e.g. claude-opus-4-5, gpt-4o",
        ),
    };
    let _ = current_value;

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("  Step {step_num} of {step_total} — {step_label}"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )));
    lines.push(Line::raw(""));

    let display_input = if matches!(step, ProfileCreateStep::ApiKey) && !st.input.is_empty() {
        // Show masked dots while typing, with a length indicator
        let len = st.input.len();
        format!("{} ({} chars)", "•".repeat(len.min(30)), len)
    } else {
        st.input.clone()
    };

    let value_display = if display_input.is_empty() {
        Span::styled(
            format!("   {placeholder}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )
    } else {
        Span::styled(
            format!("   {display_input}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    };

    lines.push(Line::from(vec![
        value_display,
        Span::styled("▌", Style::default().fg(Color::Yellow)),
    ]));
    lines.push(Line::raw(""));

    // Summary of already-entered fields
    if step_num > 1 {
        lines.push(Line::from(Span::styled(
            "  Already entered:",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
        if !st.name.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    name       {}", st.name),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if !st.provider.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    provider   {}", st.provider),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    if let Some(ref msg) = st.status {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(if msg.starts_with("  ✓") {
                Color::Green
            } else {
                Color::Red
            }),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );

    let hint = if matches!(step, ProfileCreateStep::ApiKey) {
        "Type API key  ·  Enter=next  ·  Esc=cancel"
    } else {
        "Type value  ·  Enter=next  ·  Esc=cancel"
    };
    frame.render_widget(
        Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        hint_area,
    );

    // Cursor inside the input line (line index 3 = blank + hint + blank + input)
    let cursor_y = content_area.y + 4;
    let shown_len = if matches!(step, ProfileCreateStep::ApiKey) {
        let len = st.input.len();
        // "•" repeated + " (N chars)"
        len.min(30) + format!(" ({} chars)", len).len()
    } else {
        st.input_cursor
    };
    let cursor_x = content_area.x + 3 + shown_len as u16;
    if cursor_y < content_area.y + content_area.height {
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

pub(super) fn render_profile_provider_picker(
    frame: &mut Frame,
    popup_rect: Rect,
    content_area: Rect,
    hint_area: Rect,
    st: &ProfileOverlayState,
) {
    frame.render_widget(
        Block::default()
            .title(" Select Provider ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        popup_rect,
    );

    // 2 lines for filter + blank, 1 line for scroll indicator
    let visible: usize = (content_area.height as usize).saturating_sub(3).max(3);
    let picker = &st.provider_picker;
    let indices = picker.filtered();

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![Span::styled(
            format!("  Filter: {}_", picker.filter),
            Style::default().fg(Color::White),
        )]),
        Line::raw(""),
    ];

    let end = (picker.scroll_offset + visible).min(indices.len());
    for (di, &idx) in indices[picker.scroll_offset..end].iter().enumerate() {
        let abs_pos = picker.scroll_offset + di;
        let selected = abs_pos == picker.selected;
        let (id, name, needs_key) = KNOWN_PROVIDERS[idx];
        let bg = if selected {
            TUI_SELECTION_BG
        } else {
            Color::Reset
        };
        let fg = if selected { Color::White } else { Color::Gray };
        let key_hint = if !needs_key { "  (no key needed)" } else { "" };
        lines.push(Line::from(vec![Span::styled(
            format!("  {:<12}  {}{}", id, name, key_hint),
            Style::default().fg(fg).bg(bg),
        )]));
    }

    let above = picker.scroll_offset;
    let below = indices.len().saturating_sub(picker.scroll_offset + visible);
    if let Some(line) = scroll_indicator_line(above, below) {
        lines.push(line);
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );
    frame.render_widget(
        Paragraph::new("↑↓=navigate  Enter=select  type to filter  Esc=cancel").style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        hint_area,
    );
}

pub(super) fn render_profile_model_picker(
    frame: &mut Frame,
    popup_rect: Rect,
    content_area: Rect,
    hint_area: Rect,
    st: &ProfileOverlayState,
) {
    frame.render_widget(
        Block::default()
            .title(" Select Model ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
        popup_rect,
    );

    let Some(ref picker) = st.profile_model_picker else {
        return;
    };
    // 3 lines for filter + header + blank, 1 for scroll indicator
    let visible: usize = (content_area.height as usize).saturating_sub(4).max(3);
    let filtered = picker.filtered();

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            format!("  Filter: {}_", picker.filter),
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            format!(
                "  {:<32}  {:<12}  {:>8}  {:>8}  {:>6}",
                "model", "provider", "$/1M in", "$/1M out", "ctx k"
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )),
        Line::raw(""),
    ];

    let end = (picker.scroll_offset + visible).min(filtered.len());
    for (di, m) in filtered[picker.scroll_offset..end].iter().enumerate() {
        let selected = picker.scroll_offset + di == picker.selected;
        let bg = if selected {
            TUI_SELECTION_BG
        } else {
            Color::Reset
        };
        let fg = if selected { Color::White } else { Color::Gray };
        let ctx = m
            .context_k
            .map(|k| format!("{:>4}k", k))
            .unwrap_or_else(|| "    ?".into());
        let id_display: String = m.id.chars().take(32).collect();
        let prov_display: String = m.provider.chars().take(12).collect();
        lines.push(Line::from(Span::styled(
            format!(
                "  {:<32}  {:<12}  {:>8.2}  {:>8.2}  {}",
                id_display, prov_display, m.input_mtok, m.output_mtok, ctx
            ),
            Style::default().fg(fg).bg(bg),
        )));
    }

    let above = picker.scroll_offset;
    let below = filtered
        .len()
        .saturating_sub(picker.scroll_offset + visible);
    if let Some(line) = scroll_indicator_line(above, below) {
        lines.push(line);
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );
    frame.render_widget(
        Paragraph::new("↑↓=navigate  Enter=select  type to filter  Esc=cancel").style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        hint_area,
    );
}

// ── Modal component helpers ───────────────────────────────────────────────────

/// Rect anchored just above the input field (grows upward).
pub(super) fn popup_above_input(input_area: Rect, h: u16, w: u16) -> Rect {
    let w = w.min(input_area.width);
    let x = input_area.x + (input_area.width.saturating_sub(w)) / 2;
    let y = input_area.y.saturating_sub(h);
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Format a timestamp (RFC-3339 or similar) as a relative duration from now.
pub(super) fn relative_time(ts: &str) -> String {
    let parsed = chrono::DateTime::parse_from_rfc3339(ts).or_else(|_| {
        chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.f")
            .map(|dt: chrono::NaiveDateTime| dt.and_utc().fixed_offset())
    });
    let dt = match parsed {
        Ok(dt) => dt,
        Err(_) => {
            // Fallback: show truncated original
            return if ts.len() >= 16 {
                format!("{} {}", &ts[5..10], &ts[11..16])
            } else {
                ts.to_string()
            };
        }
    };
    let now = chrono::Utc::now();
    let delta = now.signed_duration_since(dt);
    let secs = delta.num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;
    if secs < 60 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{}m ago", mins)
    } else if hours < 24 {
        format!("{}h ago", hours)
    } else if days < 7 {
        format!("{}d ago", days)
    } else if days < 30 {
        format!("{}w ago", days / 7)
    } else if days < 365 {
        format!("{}mo ago", days / 30)
    } else {
        format!("{}y ago", days / 365)
    }
}

/// Styled popup block — same structure for every modal.
pub(super) fn modal_block(title: &str, border_color: Color) -> Block<'static> {
    Block::default()
        .title(title.to_string())
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

pub(super) fn modal_block_with_hint(
    title: &str,
    hint: &str,
    border_color: Color,
) -> Block<'static> {
    Block::default()
        .title(title.to_string())
        .title_alignment(Alignment::Left)
        .title_bottom(Line::from(Span::styled(
            hint.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

/// Build a filter indicator line (🔍 + yellow text) for picker popups.
pub(super) fn filter_indicator_line(filter_text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  🔍 ", Style::default().fg(Color::DarkGray)),
        Span::styled(filter_text.to_string(), Style::default().fg(Color::Yellow)),
    ])
}

/// Build a scroll indicator line showing how many items are above/below the visible window.
pub(super) fn scroll_indicator_line(above: usize, below: usize) -> Option<Line<'static>> {
    if above == 0 && below == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if above > 0 {
        parts.push(format!("↑↑ {} more", above));
    }
    if below > 0 {
        parts.push(format!("↓↓ {} more", below));
    }
    Some(Line::from(Span::styled(
        format!("  {}", parts.join("  ")),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )))
}

/// Two-column row (e.g. for slash completions): cmd | description, with highlight on selection.
pub(super) fn modal_row_two_col(
    left: String,
    right: String,
    left_color: Color,
    right_color: Color,
    selected: bool,
) -> Line<'static> {
    let bg = if selected {
        TUI_SELECTION_BG
    } else {
        Color::Reset
    };
    Line::from(vec![
        Span::styled(
            left,
            Style::default()
                .fg(left_color)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(right, Style::default().fg(right_color).bg(bg)),
    ])
}

/// Build a deterministic plan snapshot from assistant text.
/// This is the canonical path used for both saving and display.
pub(super) fn build_plan_from_assistant_text(text: &str) -> Option<Plan> {
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

pub(super) fn build_plan_from_tasks(tasks: &[String]) -> Option<Plan> {
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
pub(super) fn strip_plan_line_prefix(line: &str) -> String {
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
pub(super) fn parse_plan_from_text(text: &str) -> Vec<String> {
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
pub(super) fn extract_current_step_full(text: &str) -> Option<(usize, String)> {
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

pub(super) fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

/// Word-wrap `text` to lines of at most `width` characters.
pub(super) fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut cur = String::new();
        for word in paragraph.split_whitespace() {
            if cur.is_empty() {
                cur = word.to_string();
            } else if cur.len() + 1 + word.len() <= width {
                cur.push(' ');
                cur.push_str(word);
            } else {
                lines.push(cur);
                cur = word.to_string();
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

/// Drop spans from the right until the total char width fits within `max_width`.
/// Prevents mid-span clipping in single-line bars.
pub(super) fn fit_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    let mut used = 0usize;
    let mut out = Vec::new();
    for span in spans {
        let w = span.content.chars().count();
        if used + w > max_width {
            break;
        }
        used += w;
        out.push(span);
    }
    out
}

/// Return the semantic color for a tool call based on its type and state.
pub(super) fn tool_color(name: &str, done: bool, is_error: bool) -> Color {
    if is_error {
        return Color::Red;
    }
    if done {
        return Color::DarkGray;
    }
    match name {
        "Read" | "Glob" | "Grep" => TUI_SOFT_ACCENT,
        "Write" | "Edit" => Color::Green,
        "Bash" => Color::Yellow,
        "SemanticSearch" => Color::Cyan,
        "WebFetch" | "WebSearch" => Color::Magenta,
        "SpawnWorker" | "SpawnReviewer" => Color::LightCyan,
        _ => Color::White,
    }
}

/// Maps internal tool names to human-readable display labels.
pub(super) fn tool_display_name(name: &str) -> &str {
    match name {
        "SemanticSearch" => "Search",
        "SpawnWorker" => "Worker",
        "SpawnReviewer" => "Reviewer",
        "TodoWrite" => "Todo",
        "WebFetch" => "Fetch",
        "WebSearch" => "Web",
        other => other,
    }
}

/// Width-aware version; call this from render paths where chat_area.width is known.
/// Uses a per-width render cache keyed by message content hash to avoid re-rendering
/// unchanged messages on every tick.
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

    if let Some(cached) = app.render_cache.get(&cache_key) {
        return cached.clone();
    }

    let result = build_lines_w_uncached(app, width);
    app.render_cache.insert(cache_key, result.clone());
    result
}

pub(super) fn build_lines_w_uncached(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for msg in &app.messages {
        match msg {
            ChatLine::User(text) => {
                out.push(Line::from(vec![Span::styled(
                    "you",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )]));
                out.extend(render_markdown(text, width));
                out.push(Line::raw(""));
            }
            ChatLine::Assistant(text) => {
                let label = if app.model.is_empty() {
                    "clido".to_string()
                } else {
                    app.model.clone()
                };
                out.push(Line::from(vec![Span::styled(
                    label,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )]));
                out.extend(render_markdown(text, width));
                out.push(Line::raw(""));
            }
            ChatLine::Thinking(text) => {
                for part in text.lines() {
                    out.push(Line::from(vec![
                        Span::raw("    "),
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
                let color = tool_color(name, *done, *is_error);
                let style = Style::default().fg(color);
                let icon = if *is_error {
                    "✗"
                } else if *done {
                    "✓"
                } else {
                    "↻"
                };
                let display_name = tool_display_name(name);
                let dim = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM);
                if detail.is_empty() {
                    out.push(Line::from(vec![Span::styled(
                        format!("  {} {}", icon, display_name),
                        style,
                    )]));
                } else {
                    out.push(Line::from(vec![
                        Span::styled(format!("  {} {}", icon, display_name), style),
                        Span::styled(format!("  {}", detail.clone()), dim),
                    ]));
                }
            }
            ChatLine::Diff(text) => {
                let mut old_lineno: u32 = 0;
                let mut new_lineno: u32 = 0;
                for line in text.lines() {
                    if line.starts_with("@@") {
                        if let Some((o, n)) = parse_hunk_header(line) {
                            old_lineno = o;
                            new_lineno = n;
                        }
                        out.push(Line::from(vec![Span::styled(
                            format!("  {}", line),
                            Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                        )]));
                    } else if line.starts_with("---") || line.starts_with("+++") {
                        out.push(Line::from(vec![Span::styled(
                            format!("  {}", line),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::DIM),
                        )]));
                    } else if line.starts_with('+') {
                        let lineno = new_lineno;
                        new_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ]));
                    } else if line.starts_with('-') {
                        let lineno = old_lineno;
                        old_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                            ),
                        ]));
                    } else {
                        // context line — belongs to both
                        let lineno = new_lineno;
                        old_lineno += 1;
                        new_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ]));
                    }
                }
                out.push(Line::raw(""));
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
                out.push(Line::from(vec![Span::styled(
                    format!("  {}", text),
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::BOLD),
                )]));
            }
            ChatLine::WelcomeBrand => {
                out.push(Line::from(vec![
                    Span::styled("  ── ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        "cli",
                        Style::default()
                            .fg(Color::Gray)
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
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " ─────────────────────",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
            ChatLine::WelcomeSplash => {
                // Shown only when scrolling back past the start of a resumed conversation.
                out.push(Line::from(vec![
                    Span::styled("  ── ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        "cli",
                        Style::default()
                            .fg(Color::Gray)
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
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " ─────────────────────",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
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

    for event in parser {
        match event {
            // ── Start tags ────────────────────────────────────────────────
            Event::Start(tag) => match tag {
                Tag::Strong => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::BOLD);
                    style_stack.push(s);
                }
                Tag::Emphasis => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::ITALIC);
                    style_stack.push(s);
                }
                Tag::Strikethrough => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::CROSSED_OUT);
                    style_stack.push(s);
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
                    let label = if lang.is_empty() {
                        "code".to_string()
                    } else {
                        lang
                    };
                    out.push(Line::from(vec![Span::styled(
                        format!("┌─ {} ", label),
                        Style::default().fg(Color::DarkGray),
                    )]));
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
                    out.push(Line::from(vec![Span::styled(
                        format!("└{}", "─".repeat(content_w.min(60))),
                        Style::default().fg(Color::DarkGray),
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
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::DIM),
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── word_wrap ────────────────────────────────────────────────────────────

    #[test]
    fn word_wrap_short_line_fits() {
        let lines = word_wrap("hello world", 20);
        assert_eq!(lines, vec!["hello world"]);
    }

    #[test]
    fn word_wrap_breaks_at_width() {
        let lines = word_wrap("hello world foo bar", 11);
        assert_eq!(lines, vec!["hello world", "foo bar"]);
    }

    #[test]
    fn word_wrap_single_long_word() {
        let lines = word_wrap("superlongword", 5);
        // A word longer than width is placed on its own line
        assert_eq!(lines, vec!["superlongword"]);
    }

    #[test]
    fn word_wrap_preserves_paragraph_breaks() {
        let lines = word_wrap("first paragraph\nsecond paragraph", 50);
        assert_eq!(lines, vec!["first paragraph", "second paragraph"]);
    }

    #[test]
    fn word_wrap_empty_input() {
        let lines = word_wrap("", 20);
        assert!(lines.is_empty());
    }

    #[test]
    fn word_wrap_multiple_words_wrap_correctly() {
        let lines = word_wrap("a b c d e f", 5);
        // "a b c" fits in 5, "d e f" fits in 5
        assert_eq!(lines, vec!["a b c", "d e f"]);
    }

    // ── truncate_chars ──────────────────────────────────────────────────────

    #[test]
    fn truncate_chars_short_string_unchanged() {
        assert_eq!(truncate_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_chars_exact_length_unchanged() {
        assert_eq!(truncate_chars("hello", 5), "hello");
    }

    #[test]
    fn truncate_chars_long_string_truncated() {
        let result = truncate_chars("hello world", 8);
        assert_eq!(result, "hello w…");
        assert!(result.chars().count() <= 8);
    }

    #[test]
    fn truncate_chars_respects_unicode_boundaries() {
        // "你好世界" is 4 chars; truncate to 3 should give "你好…"
        let result = truncate_chars("你好世界", 3);
        assert_eq!(result, "你好…");
    }

    #[test]
    fn truncate_chars_max_1_gives_ellipsis() {
        let result = truncate_chars("hello", 1);
        assert_eq!(result, "…");
    }

    // ── tool_color ──────────────────────────────────────────────────────────

    #[test]
    fn tool_color_error_is_red() {
        assert_eq!(tool_color("Read", false, true), Color::Red);
        assert_eq!(tool_color("Bash", false, true), Color::Red);
    }

    #[test]
    fn tool_color_done_is_dark_gray() {
        assert_eq!(tool_color("Read", true, false), Color::DarkGray);
        assert_eq!(tool_color("Write", true, false), Color::DarkGray);
    }

    #[test]
    fn tool_color_active_returns_semantic_colors() {
        assert_eq!(tool_color("Read", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Glob", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Grep", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Write", false, false), Color::Green);
        assert_eq!(tool_color("Edit", false, false), Color::Green);
        assert_eq!(tool_color("Bash", false, false), Color::Yellow);
        assert_eq!(tool_color("SemanticSearch", false, false), Color::Cyan);
        assert_eq!(tool_color("WebFetch", false, false), Color::Magenta);
        assert_eq!(tool_color("WebSearch", false, false), Color::Magenta);
        assert_eq!(tool_color("SpawnWorker", false, false), Color::LightCyan);
        assert_eq!(tool_color("SpawnReviewer", false, false), Color::LightCyan);
    }

    #[test]
    fn tool_color_unknown_tool_is_white() {
        assert_eq!(tool_color("UnknownTool", false, false), Color::White);
    }

    #[test]
    fn tool_color_consistent_across_calls() {
        let c1 = tool_color("Bash", false, false);
        let c2 = tool_color("Bash", false, false);
        assert_eq!(c1, c2);
    }

    // ── tool_display_name ───────────────────────────────────────────────────

    #[test]
    fn tool_display_name_maps_known_tools() {
        assert_eq!(tool_display_name("SemanticSearch"), "Search");
        assert_eq!(tool_display_name("SpawnWorker"), "Worker");
        assert_eq!(tool_display_name("SpawnReviewer"), "Reviewer");
        assert_eq!(tool_display_name("TodoWrite"), "Todo");
        assert_eq!(tool_display_name("WebFetch"), "Fetch");
        assert_eq!(tool_display_name("WebSearch"), "Web");
    }

    #[test]
    fn tool_display_name_passes_through_unknown() {
        assert_eq!(tool_display_name("Read"), "Read");
        assert_eq!(tool_display_name("Bash"), "Bash");
        assert_eq!(tool_display_name("CustomTool"), "CustomTool");
    }

    // ── fit_spans ───────────────────────────────────────────────────────────

    #[test]
    fn fit_spans_all_fit() {
        let spans = vec![Span::raw("hello"), Span::raw(" "), Span::raw("world")];
        let result = fit_spans(spans, 20);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn fit_spans_drops_overflow() {
        let spans = vec![Span::raw("hello"), Span::raw(" "), Span::raw("world")];
        let result = fit_spans(spans, 6);
        // "hello" (5) + " " (1) = 6, "world" would overflow
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn fit_spans_empty_input() {
        let result = fit_spans(vec![], 10);
        assert!(result.is_empty());
    }

    // ── relative_time ───────────────────────────────────────────────────────

    #[test]
    fn relative_time_just_now() {
        let now = chrono::Utc::now().to_rfc3339();
        assert_eq!(relative_time(&now), "just now");
    }

    #[test]
    fn relative_time_minutes_ago() {
        let five_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let result = relative_time(&five_min_ago);
        assert!(result.contains("m ago"), "expected minutes, got: {result}");
    }

    #[test]
    fn relative_time_hours_ago() {
        let three_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
        let result = relative_time(&three_hours_ago);
        assert!(result.contains("h ago"), "expected hours, got: {result}");
    }

    #[test]
    fn relative_time_days_ago() {
        let two_days_ago = (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339();
        let result = relative_time(&two_days_ago);
        assert!(result.contains("d ago"), "expected days, got: {result}");
    }

    #[test]
    fn relative_time_future_timestamp_shows_just_now() {
        let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        assert_eq!(relative_time(&future), "just now");
    }

    #[test]
    fn relative_time_invalid_string_fallback() {
        assert_eq!(relative_time("not a timestamp"), "not a timestamp");
    }

    #[test]
    fn relative_time_truncated_fallback_for_long_invalid() {
        let result = relative_time("2024-01-15T10:30:00 not-rfc3339");
        // Should fall through to truncation fallback (len >= 16)
        assert!(!result.is_empty());
    }
}
