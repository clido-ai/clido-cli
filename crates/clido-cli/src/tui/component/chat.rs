//! ChatArea component — messages, scroll, welcome screen.
//! Phase 1: delegates to existing render logic.

use super::{Component, DirtyFlag, EventResult, LayoutZones};
use crate::tui::app_state::App;
use ratatui::Frame;

pub struct ChatArea {
    dirty: DirtyFlag,
    last_msg_count: usize,
    last_scroll: u32,
}

impl ChatArea {
    pub fn new() -> Self {
        Self {
            dirty: DirtyFlag(true),
            last_msg_count: 0,
            last_scroll: 0,
        }
    }
}

impl Component for ChatArea {
    fn on_app_update(&mut self, app: &App) -> bool {
        if app.messages.len() != self.last_msg_count || app.scroll != self.last_scroll {
            self.last_msg_count = app.messages.len();
            self.last_scroll = app.scroll;
            self.dirty.set();
        }
        self.dirty.is_set()
    }

    fn handle_key(&mut self, _key: crossterm::event::KeyEvent, _app: &mut App) -> EventResult {
        EventResult::PassThrough
    }

    fn render(&self, _frame: &mut Frame, _zones: &LayoutZones, _app: &App) {
        // Phase 1: chat rendering is still done by the main render function.
    }

    fn is_dirty(&self) -> bool {
        self.dirty.is_set()
    }
    fn mark_clean(&mut self) {
        self.dirty.take();
    }
}
