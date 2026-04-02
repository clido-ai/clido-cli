use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::*;

use super::widgets::{popup_above_input, scroll_indicator_line};

// ── Profile overlay renderer ──────────────────────────────────────────────────

pub(crate) fn render_profile_overlay(
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

pub(crate) fn render_profile_overview(
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
                4 => ProfileEditField::FastProvider,
                5 => ProfileEditField::FastApiKey,
                6 => ProfileEditField::FastModel,
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
        let display_value = if *key == "api_key" || *key == "fast_api_key" {
            if is_editing {
                let len = st.input.len();
                if len == 0 {
                    String::new()
                } else {
                    format!("{} ({} chars)", "•".repeat(len.min(30)), len)
                }
            } else if *key == "api_key" {
                st.masked_api_key()
            } else {
                st.masked_fast_api_key()
            }
        } else if is_editing {
            st.input.clone()
        } else {
            let raw = match field_cursor {
                0 => st.provider.clone(),
                1 => st.masked_api_key(),
                2 => st.model.clone(),
                3 => st.base_url.clone(),
                4 => st.fast_provider.clone(),
                5 => st.masked_fast_api_key(),
                6 => st.fast_model.clone(),
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

pub(crate) fn render_profile_create(
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

pub(crate) fn render_profile_provider_picker(
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

pub(crate) fn render_profile_model_picker(
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
