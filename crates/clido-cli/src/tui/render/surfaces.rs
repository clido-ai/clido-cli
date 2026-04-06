//! Full-frame and zoned background fills so header, transcript, activity strips, and input read as
//! separate layers (not a flat single-color screen).

use ratatui::{layout::Rect, style::Style, widgets::Block, Frame};

use crate::tui::{
    TUI_SURFACE_ACTION, TUI_SURFACE_APP, TUI_SURFACE_CHROME, TUI_SURFACE_CONTENT,
    TUI_SURFACE_FOCUS, TUI_SURFACE_HINT, TUI_SURFACE_HOLD_BG, TUI_SURFACE_STATUS,
    TUI_SURFACE_WARN_BG,
};

/// L0 — paint behind the entire TUI so every region is visibly “on” a surface.
pub(crate) fn paint_app_canvas(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(TUI_SURFACE_APP)),
        area,
    );
}

/// L1 — header chrome (background only — borders would steal rows on 1-line headers).
pub(crate) fn header_zone_block() -> Block<'static> {
    Block::default().style(Style::default().bg(TUI_SURFACE_CHROME))
}

/// L2 — main chat / welcome well.
pub(crate) fn content_zone_block() -> Block<'static> {
    Block::default().style(Style::default().bg(TUI_SURFACE_CONTENT))
}

/// Focus lane — progress / plan / harness (lifted vs transcript and status band).
pub(crate) fn focus_lane_zone_block() -> Block<'static> {
    Block::default().style(Style::default().bg(TUI_SURFACE_FOCUS))
}

/// Tool status strip.
pub(crate) fn status_zone_block() -> Block<'static> {
    Block::default().style(Style::default().bg(TUI_SURFACE_STATUS))
}

/// Queue list (same plane as status — one instrumentation band).
pub(crate) fn queue_zone_block() -> Block<'static> {
    Block::default().style(Style::default().bg(TUI_SURFACE_STATUS))
}

/// Global shortcut hint row (slightly different from queue for a subtle “footer” read).
pub(crate) fn hint_zone_block() -> Block<'static> {
    Block::default().style(Style::default().bg(TUI_SURFACE_HINT))
}

/// Dock fill behind the rounded input `Block` (permission / rate-limit / normal).
pub(crate) fn input_dock_fill_style(pending_perm: bool, rate_limited: bool) -> Style {
    let bg = if pending_perm {
        TUI_SURFACE_WARN_BG
    } else if rate_limited {
        TUI_SURFACE_HOLD_BG
    } else {
        TUI_SURFACE_ACTION
    };
    Style::default().bg(bg)
}
