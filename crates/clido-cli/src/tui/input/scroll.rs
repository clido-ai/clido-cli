use super::super::*;

// ── Scroll helpers ────────────────────────────────────────────────────────────

pub fn scroll_up(app: &mut App, lines: u32) {
    if app.suppress_next_chat_scroll_up {
        app.suppress_next_chat_scroll_up = false;
        return;
    }
    // app.scroll is always kept in [0, max_scroll] by the render loop, so
    // plain saturating_sub is correct in all cases (including following mode).
    app.scroll = app.scroll.saturating_sub(lines);
    app.following = false;
}

pub fn scroll_down(app: &mut App, lines: u32) {
    app.suppress_next_chat_scroll_up = false;
    // app.scroll is always kept in [0, max_scroll] by the render loop, so
    // a plain saturating_add and comparison is correct in all cases.
    let new_scroll = app.scroll.saturating_add(lines);
    if new_scroll >= app.layout.max_scroll {
        app.scroll = app.layout.max_scroll;
        app.following = true;
    } else {
        app.scroll = new_scroll;
        app.following = false;
    }
}

// ── Plan panel scroll helpers ────────────────────────────────────────────────

/// Scroll up in the plan/task panel.
pub fn plan_scroll_up(app: &mut App) {
    app.plan_scroll = app.plan_scroll.saturating_sub(1);
}

/// Scroll down in the plan/task panel.
pub fn plan_scroll_down(app: &mut App) {
    let gather_plan_panel_steps = || crate::tui::render::plan::gather_plan_panel_steps(app);
    let total = gather_plan_panel_steps().len();
    let visible_cap = 12usize;
    let max_scroll = if total > visible_cap {
        (total - visible_cap) as u16
    } else {
        0
    };
    if app.plan_scroll < max_scroll {
        app.plan_scroll += 1;
    }
}
