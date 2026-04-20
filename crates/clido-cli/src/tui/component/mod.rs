//! Ratatui component layer — provides component boundaries, dirty tracking,
//! and event propagation. Components initially delegate to existing render
//! functions; logic migrates incrementally.

mod header;
mod chat;
mod input;
mod status;
mod shell;

#[allow(unused_imports)]
pub use header::*;
#[allow(unused_imports)]
pub use chat::*;
#[allow(unused_imports)]
pub use input::*;
#[allow(unused_imports)]
pub use status::*;
#[allow(unused_imports)]
pub use shell::*;

use ratatui::layout::Rect;

use crate::tui::app_state::App;

// ── Event Result ─────────────────────────────────────────────────────────────

/// Result of a key event handled by a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventResult {
    /// Event was handled — do not propagate further.
    Consumed,
    /// Event was not handled — propagate to parent/next component.
    PassThrough,
}

impl EventResult {
    #[inline]
    pub fn is_consumed(self) -> bool {
        matches!(self, Self::Consumed)
    }
}

// ── Dirty Flag ───────────────────────────────────────────────────────────────

/// A simple dirty flag for selective re-rendering.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirtyFlag(bool);

impl DirtyFlag {
    #[inline]
    pub fn set(&mut self) { self.0 = true; }
    #[inline]
    pub fn take(&mut self) -> bool {
        std::mem::replace(&mut self.0, false)
    }
    #[inline]
    pub fn is_set(&self) -> bool { self.0 }
}

// ── Layout Zones ─────────────────────────────────────────────────────────────

/// Rectangular zones computed by the layout pass.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct LayoutZones {
    pub header: Rect,
    pub chat: Rect,
    pub status: Rect,
    pub input: Rect,
}

// ── Component Trait ──────────────────────────────────────────────────────────

/// A single UI component that can render itself and handle input.
#[allow(dead_code)]
pub trait Component {
    /// Sync component state from the shared App. Called every tick.
    fn on_app_update(&mut self, app: &App) -> bool;

    /// Handle a key event. Returns `Consumed` if handled.
    fn handle_key(
        &mut self,
        _key: crossterm::event::KeyEvent,
        _app: &mut App,
    ) -> EventResult {
        EventResult::PassThrough
    }

    /// Render this component into the given zones.
    fn render(&self, frame: &mut ratatui::Frame, zones: &LayoutZones, app: &App);

    /// Whether this component needs re-rendering.
    fn is_dirty(&self) -> bool;

    /// Mark as clean after rendering.
    fn mark_clean(&mut self);
}

// ── Shared Helpers ───────────────────────────────────────────────────────────

#[allow(dead_code)]
#[inline]
pub fn is_narrow(area: Rect) -> bool { area.width < 60 }
