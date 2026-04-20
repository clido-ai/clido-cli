//! StatusBar component — status log, spinner, queue, plan strip.
//! Phase 1: delegates to existing render logic.

use ratatui::Frame;
use crate::tui::app_state::App;
use super::{Component, DirtyFlag, EventResult, LayoutZones};

pub struct StatusBar {
    dirty: DirtyFlag,
    last_status_count: usize,
    last_spinner_tick: usize,
    last_queue_count: usize,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            dirty: DirtyFlag(true),
            last_status_count: 0,
            last_spinner_tick: 0,
            last_queue_count: 0,
        }
    }
}

impl Component for StatusBar {
    fn on_app_update(&mut self, app: &App) -> bool {
        let status_count = app.status_log.len();
        let spinner_tick = app.spinner_tick;
        let queue_count = app.queued.len();
        if status_count != self.last_status_count
            || spinner_tick != self.last_spinner_tick
            || queue_count != self.last_queue_count
        {
            self.last_status_count = status_count;
            self.last_spinner_tick = spinner_tick;
            self.last_queue_count = queue_count;
            self.dirty.set();
        }
        self.dirty.is_set()
    }

    fn handle_key(&mut self, _key: crossterm::event::KeyEvent, _app: &mut App) -> EventResult {
        EventResult::PassThrough
    }

    fn render(&self, _frame: &mut Frame, _zones: &LayoutZones, _app: &App) {
        // Phase 1: status rendering is still done by the main render function.
    }

    fn is_dirty(&self) -> bool { self.dirty.is_set() }
    fn mark_clean(&mut self) { self.dirty.take(); }
}
