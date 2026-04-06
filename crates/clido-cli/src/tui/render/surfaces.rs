//! Zoned layout surfaces for the main chat TUI.
//!
//! ## Design
//!
//! The UI is drawn in **fixed horizontal bands** (see `render::render`). Each band gets a distinct
//! background so users can parse **where they are** without reading copy: chrome vs transcript vs
//! activity vs input. Surfaces use **truecolor** RGB values from the parent `tui` module (same
//! family as the rest of the theme). Horizontal bands intentionally avoid **border widgets** that consume rows:
//! single-line headers and thin queue areas must keep their full layout height.
//!
//! Modal popups and the welcome card use `TUI_TOAST_BG` in `widgets` and `welcome` — elevated above
//! `TUI_SURFACE_CONTENT`, distinct from the input dock (`TUI_SURFACE_ACTION`).
//!
//! ## Dock states
//!
//! [`input_dock_fill_style`] encodes **semantic** backgrounds: default action dock, permission
//! wait, rate-limit hold, and prompt enhancement — each visible at a glance next to border color.

use ratatui::{layout::Rect, style::Style, widgets::Block, Frame};

use crate::tui::{
    TUI_SURFACE_ACTION, TUI_SURFACE_APP, TUI_SURFACE_CHROME, TUI_SURFACE_CONTENT,
    TUI_SURFACE_ENHANCE_BG, TUI_SURFACE_FOCUS, TUI_SURFACE_HINT, TUI_SURFACE_HOLD_BG,
    TUI_SURFACE_STATUS, TUI_SURFACE_WARN_BG,
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

/// Dock fill behind the rounded input `Block`.
///
/// Precedence: **permission** > **rate limit** > **enhancing** > default action dock.
pub(crate) fn input_dock_fill_style(
    pending_perm: bool,
    rate_limited: bool,
    enhancing: bool,
) -> Style {
    let bg = if pending_perm {
        TUI_SURFACE_WARN_BG
    } else if rate_limited {
        TUI_SURFACE_HOLD_BG
    } else if enhancing {
        TUI_SURFACE_ENHANCE_BG
    } else {
        TUI_SURFACE_ACTION
    };
    Style::default().bg(bg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{
        TUI_SURFACE_ACTION, TUI_SURFACE_ENHANCE_BG, TUI_SURFACE_HOLD_BG, TUI_SURFACE_WARN_BG,
    };

    #[test]
    fn dock_fill_permission_wins_over_rate_limit_and_enhancing() {
        let s = input_dock_fill_style(true, true, true);
        assert_eq!(s.bg.expect("bg"), TUI_SURFACE_WARN_BG);
    }

    #[test]
    fn dock_fill_rate_limit_second_priority() {
        let s = input_dock_fill_style(false, true, true);
        assert_eq!(s.bg.expect("bg"), TUI_SURFACE_HOLD_BG);
    }

    #[test]
    fn dock_fill_enhancing_when_not_perm_or_rl() {
        let s = input_dock_fill_style(false, false, true);
        assert_eq!(s.bg.expect("bg"), TUI_SURFACE_ENHANCE_BG);
    }

    #[test]
    fn dock_fill_default_action() {
        let s = input_dock_fill_style(false, false, false);
        assert_eq!(s.bg.expect("bg"), TUI_SURFACE_ACTION);
    }
}
