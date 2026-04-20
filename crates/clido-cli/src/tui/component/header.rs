//! HeaderBar component — brand, model, profile, session info.
//! Phase 1: wraps existing header logic with component boundaries + dirty tracking.

use ratatui::Frame;

use crate::tui::app_state::App;

use super::{Component, DirtyFlag, EventResult, LayoutZones};

pub struct HeaderBar {
    dirty: DirtyFlag,
    last_provider: String,
    last_model: String,
    last_profile: String,
    last_session_id: Option<String>,
    last_title: Option<String>,
    last_cost: f64,
    last_busy: bool,
    last_per_turn: bool,
}

impl HeaderBar {
    pub fn new() -> Self {
        Self {
            dirty: DirtyFlag(true),
            last_provider: String::new(),
            last_model: String::new(),
            last_profile: String::new(),
            last_session_id: None,
            last_title: None,
            last_cost: 0.0,
            last_busy: false,
            last_per_turn: false,
        }
    }

    /// Whether the header-relevant state changed.
    fn header_dirty(app: &App, last: &Self) -> bool {
        app.provider != last.last_provider
            || app.model != last.last_model
            || app.current_profile != last.last_profile
            || app.current_session_id != last.last_session_id
            || app.session_title != last.last_title
            || (app.stats.session_total_cost_usd - last.last_cost).abs() > f64::EPSILON
            || app.busy != last.last_busy
            || app.per_turn_prev_model.is_some() != last.last_per_turn
    }
}

impl Component for HeaderBar {
    fn on_app_update(&mut self, app: &App) -> bool {
        if Self::header_dirty(app, self) {
            self.last_provider = app.provider.clone();
            self.last_model = app.model.clone();
            self.last_profile = app.current_profile.clone();
            self.last_session_id = app.current_session_id.clone();
            self.last_title = app.session_title.clone();
            self.last_cost = app.stats.session_total_cost_usd;
            self.last_busy = app.busy;
            self.last_per_turn = app.per_turn_prev_model.is_some();
            self.dirty.set();
        }
        self.dirty.is_set()
    }

    fn handle_key(&mut self, _key: crossterm::event::KeyEvent, _app: &mut App) -> EventResult {
        EventResult::PassThrough
    }

    fn render(&self, _frame: &mut Frame, _zones: &LayoutZones, _app: &App) {
        // Phase 1: header rendering is still done by the main render function.
        // Phase 2: extract header building into this component.
    }

    fn is_dirty(&self) -> bool { self.dirty.is_set() }
    fn mark_clean(&mut self) { self.dirty.take(); }
}
