//! AppShell — the root component that owns the component tree,
//! manages the layout pass, and orchestrates rendering + event dispatch.

use ratatui::{
    layout::{Constraint, Layout},
    Frame,
};

use crate::tui::app_state::App;

use super::{
    header::HeaderBar,
    chat::ChatArea,
    input::InputBar,
    status::StatusBar,
    Component, DirtyFlag, LayoutZones,
};

/// The root component — owns the entire UI tree.
pub struct AppShell {
    dirty: DirtyFlag,
    header: HeaderBar,
    chat: ChatArea,
    status: StatusBar,
    input: InputBar,
}

impl AppShell {
    pub fn new() -> Self {
        Self {
            dirty: DirtyFlag(true),
            header: HeaderBar::new(),
            chat: ChatArea::new(),
            status: StatusBar::new(),
            input: InputBar::new(),
        }
    }

    /// Compute layout zones from the terminal area.
    #[allow(dead_code)]
    pub fn compute_zones(&self, app: &App, area: ratatui::layout::Rect) -> LayoutZones {
        let h = area.height;
        if h == 0 { return LayoutZones::default(); }

        let line_count = app.text_input.text.matches('\n').count() + 1;
        let input_h = (line_count as u16).clamp(3, 8).min(h.saturating_sub(2));

        let status_h = if !app.status_log.is_empty() || !app.queued.is_empty() {
            3u16.min(h.saturating_sub(1 + input_h + 1))
        } else { 0 };

        let header_h = if app.current_session_id.is_some() || app.session_title.is_some() {
            2u16.min(h.saturating_sub(input_h + status_h + 1))
        } else { 1u16.min(h.saturating_sub(input_h + status_h)) };

        let chat_h = h.saturating_sub(header_h + input_h + status_h);

        let chunks = Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Length(chat_h),
            Constraint::Length(status_h),
            Constraint::Length(input_h),
        ]).split(area);

        LayoutZones {
            header: chunks[0],
            chat: chunks[1],
            status: chunks[2],
            input: chunks[3],
        }
    }

    /// Handle a key event through the component tree.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent, app: &mut App) -> bool {
        self.on_app_update(app);

        let mode = app.focus();
        match mode {
            crate::tui::state::FocusTarget::ChatInput => {
                return self.input.handle_key(key, app).is_consumed();
            }
            crate::tui::state::FocusTarget::PlanEditor
            | crate::tui::state::FocusTarget::WorkflowEditor => {
                return false;
            }
            _ => {}
        }

        if self.input.handle_key(key, app).is_consumed() { return true; }
        if self.chat.handle_key(key, app).is_consumed() { return true; }
        if self.status.handle_key(key, app).is_consumed() { return true; }
        if self.header.handle_key(key, app).is_consumed() { return true; }
        false
    }
}

impl Component for AppShell {
    fn on_app_update(&mut self, app: &App) -> bool {
        self.header.on_app_update(app);
        self.chat.on_app_update(app);
        self.status.on_app_update(app);
        self.input.on_app_update(app);
        self.dirty.is_set()
            || self.header.is_dirty()
            || self.chat.is_dirty()
            || self.status.is_dirty()
            || self.input.is_dirty()
    }

    fn render(&self, _frame: &mut Frame, _zones: &LayoutZones, _app: &App) {
        // Phase 1: rendering done by main render function.
    }

    fn is_dirty(&self) -> bool {
        self.dirty.is_set()
            || self.header.is_dirty()
            || self.chat.is_dirty()
            || self.status.is_dirty()
            || self.input.is_dirty()
    }

    fn mark_clean(&mut self) {
        self.dirty.take();
        self.header.mark_clean();
        self.chat.mark_clean();
        self.status.mark_clean();
        self.input.mark_clean();
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn create_shell() -> AppShell { AppShell::new() }

pub fn sync_shell(shell: &mut AppShell, app: &App) -> bool {
    shell.on_app_update(app)
}

pub fn handle_key_shell(shell: &mut AppShell, key: crossterm::event::KeyEvent, app: &mut App) -> bool {
    shell.handle_key(key, app)
}

#[allow(dead_code)]
pub fn compute_zones(shell: &AppShell, app: &App, area: ratatui::layout::Rect) -> LayoutZones {
    shell.compute_zones(app, area)
}

pub fn clean_shell(shell: &mut AppShell) { shell.mark_clean(); }
