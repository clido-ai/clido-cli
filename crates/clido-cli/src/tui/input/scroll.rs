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
