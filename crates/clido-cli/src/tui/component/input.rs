//! InputBar component — text input, busy state.
//! Phase 1: delegates to existing input handling.

use super::{Component, DirtyFlag, EventResult, LayoutZones};
use crate::tui::app_state::App;
use ratatui::Frame;

pub struct InputBar {
    dirty: DirtyFlag,
    last_text: String,
    last_cursor: usize,
    last_busy: bool,
    last_enhancing: bool,
    last_pending_perm: bool,
}

impl InputBar {
    pub fn new() -> Self {
        Self {
            dirty: DirtyFlag(true),
            last_text: String::new(),
            last_cursor: 0,
            last_busy: false,
            last_enhancing: false,
            last_pending_perm: false,
        }
    }
}

impl Component for InputBar {
    fn on_app_update(&mut self, app: &App) -> bool {
        let text = app.text_input.text.clone();
        let cursor = app.text_input.cursor;
        let busy = app.busy;
        let enhancing = app.enhancing;
        let pending_perm = app.pending_perm.is_some();
        if text != self.last_text
            || cursor != self.last_cursor
            || busy != self.last_busy
            || enhancing != self.last_enhancing
            || pending_perm != self.last_pending_perm
        {
            self.last_text = text;
            self.last_cursor = cursor;
            self.last_busy = busy;
            self.last_enhancing = enhancing;
            self.last_pending_perm = pending_perm;
            self.dirty.set();
        }
        self.dirty.is_set()
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent, app: &mut App) -> EventResult {
        crate::tui::input::handle_key(app, key);
        EventResult::Consumed
    }

    fn render(&self, _frame: &mut Frame, _zones: &LayoutZones, _app: &App) {
        // Phase 1: input rendering is still done by the main render function.
    }

    fn is_dirty(&self) -> bool {
        self.dirty.is_set()
    }
    fn mark_clean(&mut self) {
        self.dirty.take();
    }
}
