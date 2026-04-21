//! AppShell — the root component that owns the component tree,
//! manages the layout pass, and orchestrates rendering + event dispatch.

use ratatui::Frame;

use crate::tui::app_state::App;

use super::{
    chat::ChatArea, header::HeaderBar, input::InputBar, status::StatusBar, Component, DirtyFlag,
    LayoutZones,
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

    /// Handle a key event through the component tree.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent, app: &mut App) -> bool {
        self.on_app_update(app);
        match app.focus() {
            crate::tui::state::FocusTarget::ChatInput => {
                return self.input.handle_key(key, app).is_consumed();
            }
            crate::tui::state::FocusTarget::PlanEditor
            | crate::tui::state::FocusTarget::WorkflowEditor => {
                return false;
            }
            _ => {}
        }
        if self.input.handle_key(key, app).is_consumed() {
            return true;
        }
        if self.chat.handle_key(key, app).is_consumed() {
            return true;
        }
        if self.status.handle_key(key, app).is_consumed() {
            return true;
        }
        if self.header.handle_key(key, app).is_consumed() {
            return true;
        }
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
        // Phase 1: full rendering delegated to existing render() function.
        // Phase 2: each component renders its zone independently.
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

// ── Public API for the event loop ─────────────────────────────────────────────

pub fn create_shell() -> AppShell {
    AppShell::new()
}
pub fn sync_shell(shell: &mut AppShell, app: &App) -> bool {
    shell.on_app_update(app)
}
pub fn handle_key_shell(
    shell: &mut AppShell,
    key: crossterm::event::KeyEvent,
    app: &mut App,
) -> bool {
    shell.handle_key(key, app)
}
pub fn clean_shell(shell: &mut AppShell) {
    shell.mark_clean();
}
