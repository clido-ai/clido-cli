use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::tui::*;

// ── Welcome panel ─────────────────────────────────────────────────────────────

pub(crate) fn is_welcome_only(app: &App) -> bool {
    app.messages.len() == 1 && matches!(app.messages[0], ChatLine::WelcomeSplash)
}

/// Centered welcome panel rendered when no conversation has started yet.
pub(crate) fn render_welcome(frame: &mut Frame, app: &App, area: Rect) {
    let muted = Style::default().fg(Color::Rgb(110, 125, 150));
    let soft = Style::default().fg(Color::Rgb(185, 195, 215));
    let accent = Style::default()
        .fg(TUI_SOFT_ACCENT)
        .add_modifier(Modifier::BOLD);
    let dim_green = Style::default().fg(Color::Rgb(100, 180, 120));
    let dim_yellow = Style::default().fg(Color::Rgb(200, 180, 90));

    // Shorten workdir to ~/...
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = app.workspace_root.display().to_string();
    let workdir = if !home.is_empty() && raw.starts_with(&home) {
        format!("~{}", &raw[home.len()..])
    } else {
        raw
    };

    // Key status
    let key_status = if app.api_key.is_empty() {
        Span::styled("key ✗", Style::default().fg(Color::Red))
    } else {
        Span::styled("key ✓", dim_green)
    };

    // Budget display
    let budget_span = if clido_providers::is_subscription_provider(&app.provider) {
        Span::styled("subscription", dim_green)
    } else {
        match clido_core::load_config(&app.workspace_root) {
            Ok(cfg) => {
                if let Some(b) = cfg.agent.max_budget_usd {
                    Span::styled(format!("budget ${:.2}", b), dim_yellow)
                } else {
                    Span::styled("budget unlimited", muted)
                }
            }
            Err(_) => Span::styled("budget unlimited", muted),
        }
    };

    // Prompt enhancement mode
    let prompt_span = match app.prompt_mode {
        PromptMode::Auto => Span::styled("prompt ✦ auto", dim_green),
        PromptMode::Off => Span::styled("prompt off", muted),
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
        ]),
        Line::from(vec![
            Span::styled("    provider ".to_string(), muted),
            Span::styled(app.provider.clone(), soft),
            Span::styled("  ·  ".to_string(), muted),
            Span::styled(app.model.clone(), soft),
        ]),
        Line::from(vec![
            Span::styled("    ".to_string(), muted),
            key_status,
            Span::styled("  ·  ".to_string(), muted),
            budget_span,
            Span::styled("  ·  ".to_string(), muted),
            prompt_span,
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "    /help   /model   /settings   /config".to_string(),
            accent,
        )),
        Line::from(Span::styled(
            "    ↑↓=history  PgUp/Dn=scroll  Ctrl+/=stop".to_string(),
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
