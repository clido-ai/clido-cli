//! TUI rendering functions for the setup wizard.

use std::env;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use clido_providers::registry::PROVIDER_REGISTRY;

use super::types::{ModelOption, SetupState, SetupStep};

use super::{anonymize_key, FAST_PROVIDER_OPTIONS, PROFILE_NAME_PREFIX, SETUP_INPUT_ACCENT};

use crate::tui::TUI_TEXT;

// ── TUI rendering ─────────────────────────────────────────────────────────────

pub(super) fn draw_setup(f: &mut Frame, s: &SetupState) {
    let area = f.area();
    let [hdr, body, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let step_label = match s.step {
        SetupStep::ProfileName => "new profile — enter name",
        SetupStep::Provider => "main agent — choose provider",
        SetupStep::Credential => {
            if s.is_local() {
                "main agent — set base URL"
            } else {
                "main agent — enter API key"
            }
        }
        SetupStep::FetchingModels => "main agent — fetching models…",
        SetupStep::Model => "main agent — choose model",
        SetupStep::FastProviderIntro => "fast provider — optional",
        SetupStep::FastProvider => "fast provider — choose provider",
        SetupStep::FastCredential => "fast provider — enter API key",
        SetupStep::FetchingFastModels => "fast provider — fetching models…",
        SetupStep::FastModel => "fast provider — choose model",
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "clido",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  v{}  │  setup — ", env!("CARGO_PKG_VERSION")),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(step_label, Style::default().fg(TUI_TEXT)),
        ])),
        hdr,
    );

    match s.step {
        SetupStep::ProfileName => draw_profile_name(f, body, s),
        SetupStep::Provider => draw_provider(f, body, s),
        SetupStep::Credential => draw_credential(f, body, s),
        SetupStep::FetchingModels => draw_fetching(f, body),
        SetupStep::Model => draw_model(f, body, s),
        SetupStep::FastProviderIntro => draw_fast_intro(f, body, s),
        SetupStep::FastProvider => draw_subagent_provider(f, body, s),
        SetupStep::FastCredential => draw_subagent_credential(f, body, s),
        SetupStep::FetchingFastModels => draw_fetching(f, body),
        SetupStep::FastModel => draw_fast_model(f, body, s),
    }

    // Hint / error line
    if let Some(err) = &s.error {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  ✗  ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(err.clone(), Style::default().fg(Color::Red)),
            ])),
            hint_area,
        );
    } else {
        let hint = match s.step {
            SetupStep::ProfileName => "  ←→ move   Home/End   Enter confirm   Esc cancel wizard   Ctrl+C cancel",
            SetupStep::Provider => {
                if s.started_with_profile_name {
                    "  ↑↓ navigate   Enter select   Esc edit name   Ctrl+C cancel"
                } else {
                    "  ↑↓ navigate   Enter select   Esc cancel wizard   Ctrl+C cancel"
                }
            }
            SetupStep::Credential => {
                if s.credential_pick_active && !s.saved_keys_for_current_provider().is_empty() {
                    "  ↑↓ saved key   Enter use   n type new   Esc back   Ctrl+C cancel"
                } else {
                    "  Enter confirm   ←→ edit   Esc back   Ctrl+C cancel"
                }
            }
            SetupStep::FetchingModels | SetupStep::FetchingFastModels => "",
            SetupStep::Model if s.model_list_mode() => {
                "  ↑↓ navigate   Enter select   type to search   Backspace erase   Esc back   Ctrl+C cancel"
            }
            SetupStep::FastProviderIntro => {
                "  ↑↓ navigate   Enter select   Esc back   Ctrl+C cancel"
            }
            SetupStep::FastProvider => {
                "  ↑↓ navigate   Enter select   Esc skip   Ctrl+C cancel"
            }
            SetupStep::FastCredential => {
                "  Enter confirm   ←→ edit   Esc skip   Ctrl+C cancel"
            }
            SetupStep::FastModel if s.fast_model_list_mode() => {
                "  ↑↓ navigate   Enter select   type to search   Backspace erase   Esc back   Ctrl+C cancel"
            }
            _ => "  Enter confirm   Backspace edit   Esc back   Ctrl+C cancel",
        };
        f.render_widget(
            Paragraph::new(hint).style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
            hint_area,
        );
    }
}

fn draw_profile_name(f: &mut Frame, area: Rect, s: &SetupState) {
    let block = Block::default()
        .title(" Profile Name ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [_pad, input_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
    let prefix_w = PROFILE_NAME_PREFIX.chars().count() as u16;
    let cc = s.text_input.cursor.min(s.text_input.text.chars().count()) as u16;
    let cursor_col = input_area.x + prefix_w + cc;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(PROFILE_NAME_PREFIX, Style::default().fg(Color::DarkGray)),
            Span::styled(
                s.text_input.text.clone(),
                Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
            ),
        ])),
        input_area,
    );
    f.set_cursor_position((cursor_col, input_area.y + 1));
}

fn draw_provider(f: &mut Frame, area: Rect, s: &SetupState) {
    let mut lines = vec![Line::raw("")];
    for (i, entry) in s.provider_picker.items().iter().enumerate() {
        let def = &PROVIDER_REGISTRY[entry.0];
        let selected = i == s.provider_picker.selected;
        lines.push(if selected {
            let mut spans = vec![
                Span::styled(
                    " ▶ ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<16}", def.name),
                    Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", def.description),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if s.current_credential.is_some() {
                spans.push(Span::styled(
                    "  (current)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ));
            }
            Line::from(spans)
        } else {
            Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("{:<16}", def.name),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  {}", def.description),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        });
    }
    lines.push(Line::raw(""));
    let block = Block::default()
        .title(" Provider ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_credential(f: &mut Frame, area: Rect, s: &SetupState) {
    let pname = PROVIDER_REGISTRY[s.provider].name;
    let offers = s.saved_keys_for_current_provider();

    if s.is_local() {
        let [info_area, input_area, _] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .areas(area);
        f.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::from(vec![
                    Span::raw("  Provider: "),
                    Span::styled(
                        pname.to_string(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ]),
            info_area,
        );
        draw_text_input(
            f,
            input_area,
            " Base URL ",
            &s.text_input.text,
            s.text_input.cursor,
            false,
        );
        return;
    }

    if s.credential_pick_active && !offers.is_empty() {
        let [info_area, pick_area, _] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Min(0),
        ])
        .areas(area);
        f.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::from(vec![
                    Span::raw("  Provider: "),
                    Span::styled(
                        pname.to_string(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ]),
            info_area,
        );
        let mut lines: Vec<Line> = vec![
            Line::raw(""),
            Line::from(vec![
                Span::styled("  Reuse a saved key", Style::default().fg(Color::DarkGray)),
                Span::styled("  (↑↓  Enter  n=new)", Style::default().fg(Color::DarkGray)),
            ]),
        ];
        for (i, o) in offers.iter().enumerate() {
            let mark = if i == s.credential_pick_index {
                Span::styled(
                    " ▶ ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("   ")
            };
            let preview = anonymize_key(&o.api_key);
            let fg = if i == s.credential_pick_index {
                Color::White
            } else {
                Color::DarkGray
            };
            lines.push(Line::from(vec![
                mark,
                Span::styled(
                    format!("{}  (profile \"{}\")", preview, o.source_profile),
                    Style::default().fg(fg),
                ),
            ]));
        }
        lines.push(Line::raw(""));
        let block = Block::default()
            .title(" Saved keys ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(SETUP_INPUT_ACCENT));
        f.render_widget(Paragraph::new(lines).block(block), pick_area);
        return;
    }

    let [info_area, input_area, _] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![
                Span::raw("  Provider: "),
                Span::styled(
                    pname.to_string(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ]),
        info_area,
    );

    let title = format!(" {} ", s.key_env());
    let masked: String = s.text_input.text.chars().map(|_| '•').collect();
    let display = if s.text_input.text.is_empty() {
        let placeholder = if let Some(ref k) = s.current_credential {
            let masked_key: String = k
                .chars()
                .enumerate()
                .map(|(i, c)| if i < 4 || i + 4 >= k.len() { c } else { '•' })
                .collect();
            format!(" Enter to keep: {}", masked_key)
        } else {
            " paste key here".to_string()
        };
        Line::from(vec![Span::styled(
            placeholder,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )])
    } else {
        Line::from(vec![Span::styled(
            format!(" {}", masked),
            Style::default().fg(TUI_TEXT),
        )])
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(SETUP_INPUT_ACCENT));
    f.render_widget(Paragraph::new(display).block(block), input_area);
    if !s.text_input.text.is_empty() {
        let cc = s.text_input.cursor.min(s.text_input.text.chars().count()) as u16;
        f.set_cursor_position((input_area.x + 2 + cc, input_area.y + 1));
    }
}

fn draw_fetching(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![Span::styled(
                "  ⏳  Fetching models from API…",
                Style::default().fg(Color::DarkGray),
            )]),
        ]),
        area,
    );
}

fn draw_model(f: &mut Frame, area: Rect, s: &SetupState) {
    let pname = PROVIDER_REGISTRY[s.provider].name;

    if s.model_list_mode() {
        // Layout: provider info | search box | scrollable model list
        let [info_area, search_area, list_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .areas(area);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  Provider: "),
                Span::styled(
                    pname,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            info_area,
        );

        // Search box
        let filter_text = &s.model_picker.filter.text;
        let search_block = Block::default()
            .title(" Search ")
            .borders(Borders::ALL)
            .border_style(if filter_text.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Yellow)
            });
        let search_content = if filter_text.is_empty() {
            Line::from(vec![Span::styled(
                " type to filter…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )])
        } else {
            Line::from(format!(" {}", filter_text))
        };
        f.render_widget(
            Paragraph::new(search_content).block(search_block),
            search_area,
        );
        // Show cursor in search box
        let cursor_pos = s
            .model_picker
            .filter
            .cursor
            .min(filter_text.chars().count());
        f.set_cursor_position((search_area.x + 2 + cursor_pos as u16, search_area.y + 1));

        // Filtered model list via picker state
        let visible_rows = list_area.height.saturating_sub(2) as usize;
        let total = s.model_picker.filtered_count();
        let scroll = s.model_picker.scroll_offset.min(total.saturating_sub(1));
        let end = (scroll + visible_rows).min(total);
        let filtered_indices = s.model_picker.filtered_indices();

        let mut lines = vec![Line::raw("")];
        for (i, &orig_idx) in filtered_indices.iter().enumerate().take(end).skip(scroll) {
            let item = &s.model_picker.items()[orig_idx];
            let selected = i == s.model_picker.selected;
            match item {
                ModelOption::Custom => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(
                                " ▶ ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                "Custom\u{2026}",
                                Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled("Custom\u{2026}", Style::default().fg(Color::DarkGray)),
                        ])
                    });
                }
                ModelOption::Entry(entry) if entry.available => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(
                                " ▶ ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                entry.id.clone(),
                                Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(entry.id.clone(), Style::default().fg(Color::DarkGray)),
                        ])
                    });
                }
                ModelOption::Entry(entry) => {
                    // Unavailable model — greyed out with a "no endpoints" marker
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(" ▶ ", Style::default().fg(Color::DarkGray)),
                            Span::styled(entry.id.clone(), Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                "  no endpoints",
                                Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(
                                format!("{}  no endpoints", entry.id),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ])
                    });
                }
            }
        }
        lines.push(Line::raw(""));

        // Count only real model entries (not Custom sentinel)
        let filtered_model_count = s
            .model_picker
            .filtered_items()
            .filter(|(_, o)| matches!(o, ModelOption::Entry(_)))
            .count();
        let avail_count = s
            .model_picker
            .filtered_items()
            .filter_map(|(_, o)| match o {
                ModelOption::Entry(m) => Some(m),
                _ => None,
            })
            .filter(|m| m.available)
            .count();
        let title = if !s.model_picker.filter.text.is_empty() {
            format!(
                " Model  ({} available / {} matched / {} total) ",
                avail_count,
                filtered_model_count,
                s.fetched_models.len()
            )
        } else {
            let total_avail = s.fetched_models.iter().filter(|e| e.available).count();
            format!(
                " Model  ({} available of {}) ",
                total_avail,
                s.fetched_models.len()
            )
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Paragraph::new(lines).block(block), list_area);
    } else {
        // Text input: fetch failed or user chose Custom…
        let [info_area, list_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  Provider: "),
                Span::styled(
                    pname,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            info_area,
        );
        let title = if s.fetched_models.is_empty() {
            " Model ID (couldn't fetch — type manually) "
        } else {
            " Model ID "
        };
        draw_text_input(
            f,
            list_area,
            title,
            &s.text_input.text,
            s.text_input.cursor,
            false,
        );
    }
}

fn draw_text_input(
    f: &mut Frame,
    area: Rect,
    title: &str,
    value: &str,
    cursor: usize,
    _masked: bool,
) {
    let block = Block::default()
        .title(title.to_string())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(SETUP_INPUT_ACCENT));
    let display = if value.is_empty() {
        Line::from(vec![Span::styled(
            " ",
            Style::default().fg(Color::DarkGray),
        )])
    } else {
        Line::from(format!(" {}", value))
    };
    f.render_widget(Paragraph::new(display).block(block), area);
    let cc = cursor.min(value.chars().count()) as u16;
    f.set_cursor_position((area.x + 2 + cc, area.y + 1));
}

fn draw_fast_intro(f: &mut Frame, area: Rect, s: &SetupState) {
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![Span::styled(
            "  A fast provider routes utility tasks (titles, summaries) to a cheaper model.",
            Style::default().fg(Color::Gray),
        )]),
        Line::from(vec![Span::styled(
            "  This reduces request usage on your main provider. Routing is automatic.",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]),
        Line::raw(""),
    ];

    for (i, (name, desc)) in FAST_PROVIDER_OPTIONS.iter().enumerate() {
        let selected = i == s.fast_intro_cursor;
        lines.push(if selected {
            Line::from(vec![
                Span::styled(
                    " ▶ ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<24}", name),
                    Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", desc), Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("{:<24}", name),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  {}", desc),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        });
    }
    lines.push(Line::raw(""));

    let block = Block::default()
        .title(" Fast Provider  (optional) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_subagent_provider(f: &mut Frame, area: Rect, s: &SetupState) {
    let [title_area, body_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    "Fast provider",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "  (cheaper model for utility tasks)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ]),
        ]),
        title_area,
    );

    let picker = &s.fast_provider_picker;
    let mut lines = vec![Line::raw("")];
    for (i, entry) in picker.items().iter().enumerate() {
        let def = &PROVIDER_REGISTRY[entry.0];
        let selected = i == picker.selected;
        lines.push(if selected {
            Line::from(vec![
                Span::styled(
                    " ▶ ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<16}", def.name),
                    Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", def.description),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else {
            Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("{:<16}", def.name),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  {}", def.description),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        });
    }
    lines.push(Line::raw(""));
    let block = Block::default()
        .title(" Fast Provider — Provider ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), body_area);
}

fn draw_subagent_credential(f: &mut Frame, area: Rect, s: &SetupState) {
    let [info_area, input_area, _] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    let prov_idx = s.fast_provider_idx;
    let pname = PROVIDER_REGISTRY[prov_idx].name;
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![
                Span::raw("  Fast provider — Provider: "),
                Span::styled(
                    pname.to_string(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ]),
        info_area,
    );

    let key_env = PROVIDER_REGISTRY[prov_idx].api_key_env;
    let title = if key_env.is_empty() {
        " Base URL ".to_string()
    } else {
        format!(" {} ", key_env)
    };
    let masked: String = s.text_input.text.chars().map(|_| '•').collect();
    let display = if s.text_input.text.is_empty() {
        // Show saved credential partially masked if available
        if let Some(ref saved) = s.current_fast_credential {
            let visible_len = saved.len().min(8);
            let hidden_len = saved.len().saturating_sub(visible_len);
            let visible = &saved[..visible_len];
            let masked = "•".repeat(hidden_len.min(4));
            Line::from(vec![Span::styled(
                format!(" {}{}", visible, masked),
                Style::default().fg(TUI_TEXT),
            )])
        } else {
            Line::from(vec![Span::styled(
                " paste key here",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )])
        }
    } else {
        Line::from(vec![Span::styled(
            format!(" {}", masked),
            Style::default().fg(TUI_TEXT),
        )])
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(SETUP_INPUT_ACCENT));
    f.render_widget(Paragraph::new(display).block(block), input_area);
    if !s.text_input.text.is_empty() {
        let cc = s.text_input.cursor.min(s.text_input.text.chars().count()) as u16;
        f.set_cursor_position((input_area.x + 2 + cc, input_area.y + 1));
    }
}

fn draw_fast_model(f: &mut Frame, area: Rect, s: &SetupState) {
    let prov_idx = s.fast_provider_idx;
    let pname = PROVIDER_REGISTRY[prov_idx].name;

    if s.fast_model_list_mode() {
        let [info_area, search_area, list_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .areas(area);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  Fast provider — Provider: "),
                Span::styled(
                    pname,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            info_area,
        );

        let picker = &s.fast_model_picker;
        let filter_text = &picker.filter.text;
        let search_block = Block::default()
            .title(" Search ")
            .borders(Borders::ALL)
            .border_style(if filter_text.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Yellow)
            });
        let search_content = if filter_text.is_empty() {
            Line::from(vec![Span::styled(
                " type to filter…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )])
        } else {
            Line::from(format!(" {}", filter_text))
        };
        f.render_widget(
            Paragraph::new(search_content).block(search_block),
            search_area,
        );
        let cursor_pos = picker.filter.cursor.min(filter_text.chars().count());
        f.set_cursor_position((search_area.x + 2 + cursor_pos as u16, search_area.y + 1));

        let visible_rows = list_area.height.saturating_sub(2) as usize;
        let total = picker.filtered_count();
        let scroll = picker.scroll_offset.min(total.saturating_sub(1));
        let end = (scroll + visible_rows).min(total);
        let filtered_indices = picker.filtered_indices();

        let mut lines = vec![Line::raw("")];
        for (i, &orig_idx) in filtered_indices.iter().enumerate().take(end).skip(scroll) {
            let item = &picker.items()[orig_idx];
            let selected = i == picker.selected;
            match item {
                ModelOption::Custom => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(
                                " ▶ ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                "Custom\u{2026}",
                                Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled("Custom\u{2026}", Style::default().fg(Color::DarkGray)),
                        ])
                    });
                }
                ModelOption::Entry(e) if e.available => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(
                                " ▶ ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                e.id.clone(),
                                Style::default().fg(TUI_TEXT).add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(e.id.clone(), Style::default().fg(Color::DarkGray)),
                        ])
                    });
                }
                ModelOption::Entry(e) => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(" ▶ ", Style::default().fg(Color::DarkGray)),
                            Span::styled(e.id.clone(), Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                "  no endpoints",
                                Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(
                                format!("{}  no endpoints", e.id),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ])
                    });
                }
            }
        }
        lines.push(Line::raw(""));
        let block = Block::default()
            .title(" Fast Provider — Model ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Paragraph::new(lines).block(block), list_area);
    } else {
        let [info_area, list_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  Fast provider — Provider: "),
                Span::styled(
                    pname,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            info_area,
        );
        draw_text_input(
            f,
            list_area,
            " Model ID ",
            &s.text_input.text,
            s.text_input.cursor,
            false,
        );
    }
}
