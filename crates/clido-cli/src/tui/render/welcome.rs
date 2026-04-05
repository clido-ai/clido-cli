use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
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
    let muted = Style::default().fg(TUI_MUTED);
    let soft = Style::default().fg(TUI_BRAND_TEXT);
    let accent = Style::default()
        .fg(TUI_SOFT_ACCENT)
        .add_modifier(Modifier::BOLD);
    let dim_green = Style::default().fg(TUI_STATE_OK);
    let dim_yellow = Style::default().fg(TUI_STATE_WARN);

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
        Span::styled("key ✗", Style::default().fg(TUI_STATE_ERR))
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

    // Fast/utility provider status
    let fast_span = if app.utility_model != app.model {
        Span::styled(format!("fast {}", app.utility_model), dim_green)
    } else {
        Span::styled("fast ·", muted)
    };

    const L: &str = "    ";
    const LW: usize = 10;
    let lbl = |s: &str| format!("{:<w$}", s, w = LW);

    let content: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(format!("{L}{}", lbl("workdir")), muted),
            Span::styled(workdir, soft),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled(format!("{L}{}", lbl("profile")), muted),
            Span::styled(
                app.current_profile.clone(),
                soft.add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{L}{}", lbl("provider")), muted),
            Span::styled(app.provider.clone(), soft),
            Span::styled("  ·  ".to_string(), muted),
            Span::styled(app.model.clone(), soft),
        ]),
        Line::from(vec![
            Span::styled(format!("{L}{}", lbl("status")), muted),
            key_status,
            Span::styled("  ·  ".to_string(), muted),
            budget_span,
            Span::styled("  ·  ".to_string(), muted),
            fast_span,
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            format!("{L}/help · /model · /settings · /config"),
            accent,
        )),
        Line::from(Span::styled(
            format!("{L}Ctrl+M models · Ctrl+P profiles · Ctrl+K keys · Ctrl+V paste"),
            muted,
        )),
        Line::from(Span::styled(
            format!("{L}↑↓ history · PgUp/Dn scroll · Shift+Enter newline · Ctrl+/ stop"),
            muted,
        )),
        Line::raw(""),
    ];

    let border_color = TUI_BORDER_UI;
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
                    .fg(TUI_BRAND_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                ";".to_string(),
                Style::default().fg(TUI_MARK).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "do".to_string(),
                Style::default()
                    .fg(TUI_BRAND_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .title_alignment(Alignment::Left);

    let inner = block.inner(panel_area);
    frame.render_widget(block, panel_area);
    frame.render_widget(Paragraph::new(content), inner);
}
