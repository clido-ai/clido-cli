//! Overlay stack system: unified dispatch for popups, pickers, and editors.
//!
//! Replaces the 13 `Option<XxxState>` fields and 12-layer if-cascade in tui.rs
//! with a single `Vec<OverlayKind>` stack. Input routes to the topmost overlay;
//! rendering paints overlays bottom-to-top.
//!
//! Migration is incremental — overlays move here one at a time while legacy
//! `Option<State>` fields continue to work via the existing cascade.

use crossterm::event::KeyEvent;

// ── Actions returned by overlay key handlers ──────────────────────────────────

/// What should happen after an overlay processes a key event.
pub enum OverlayAction {
    /// Key was handled; overlay stays open.
    Consumed,
    /// Close this overlay (pop from stack).
    Dismiss,
    /// Open a new overlay on top.
    Push(OverlayKind),
    /// Replace this overlay with another (wizard step transitions).
    Replace(OverlayKind),
    /// Trigger an app-level side effect, then dismiss.
    ActionAndDismiss(AppAction),
    /// Trigger an app-level side effect, stay open.
    Action(AppAction),
    /// Key was not handled — let parent/main input process it.
    NotHandled,
}

/// App-level side effects requested by overlays.
///
/// Overlays never touch App directly. They return these values, and
/// `App::handle_overlay_action()` applies them.
#[derive(Debug, Clone)]
pub enum AppAction {
    /// Switch to a different model by ID.
    SwitchModel {
        model_id: String,
        /// If true, persist to config file.
        save: bool,
    },
    /// Switch to a different profile (triggers TUI restart).
    SwitchProfile { profile_name: String },
    /// Resume a previous session.
    ResumeSession { session_id: String },
    /// Grant or deny a pending permission request.
    GrantPermission(PermissionGrant),
    /// Show an error overlay.
    ShowError(String),
    /// Run a slash command.
    RunCommand(String),
    /// Quit the TUI.
    Quit,
}

/// Permission decision variants (mirrors the existing PermGrant).
#[derive(Debug, Clone)]
pub enum PermissionGrant {
    Once,
    Session,
    Workdir,
    Deny,
    DenyWithFeedback(String),
}

// ── Simple overlay implementations ────────────────────────────────────────────

/// Error message overlay — dismissed with Enter/Esc/Space.
#[derive(Debug, Clone)]
pub struct ErrorOverlay {
    pub title: String,
    pub message: String,
}

impl ErrorOverlay {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            title: "Error".into(),
            message: message.into(),
        }
    }

    pub fn with_title(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') => OverlayAction::Dismiss,
            _ => OverlayAction::Consumed,
        }
    }
}

/// Read-only scrollable text overlay (rules, help, etc.)
#[derive(Debug, Clone)]
pub struct ReadOnlyOverlay {
    pub title: String,
    pub lines: Vec<(String, String)>, // (heading, body) pairs
    pub scroll_offset: usize,
    pub visible_rows: usize,
}

impl ReadOnlyOverlay {
    pub fn new(title: impl Into<String>, lines: Vec<(String, String)>) -> Self {
        Self {
            title: title.into(),
            lines,
            scroll_offset: 0,
            visible_rows: 20,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Enter | KeyCode::Esc => OverlayAction::Dismiss,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = self.lines.len().saturating_sub(self.visible_rows);
                if self.scroll_offset < max {
                    self.scroll_offset += 1;
                }
                OverlayAction::Consumed
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(self.visible_rows);
                OverlayAction::Consumed
            }
            KeyCode::PageDown => {
                let max = self.lines.len().saturating_sub(self.visible_rows);
                self.scroll_offset = (self.scroll_offset + self.visible_rows).min(max);
                OverlayAction::Consumed
            }
            _ => OverlayAction::Consumed,
        }
    }
}

/// Yes/No/Cancel choice overlay.
#[derive(Debug, Clone)]
pub struct ChoiceOverlay {
    pub title: String,
    pub message: String,
    pub choices: Vec<ChoiceItem>,
    pub selected: usize,
}

/// One option in a choice overlay.
#[derive(Debug, Clone)]
pub struct ChoiceItem {
    pub label: String,
    pub action: AppAction,
}

impl ChoiceOverlay {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        choices: Vec<ChoiceItem>,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            choices,
            selected: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => OverlayAction::Dismiss,
            KeyCode::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                } else {
                    self.selected = self.choices.len().saturating_sub(1);
                }
                OverlayAction::Consumed
            }
            KeyCode::Down => {
                if self.selected + 1 < self.choices.len() {
                    self.selected += 1;
                } else {
                    self.selected = 0;
                }
                OverlayAction::Consumed
            }
            KeyCode::Enter => {
                if let Some(choice) = self.choices.get(self.selected) {
                    OverlayAction::ActionAndDismiss(choice.action.clone())
                } else {
                    OverlayAction::Dismiss
                }
            }
            // Number keys for quick selection
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = c as usize - '0' as usize;
                if idx > 0 && idx <= self.choices.len() {
                    self.selected = idx - 1;
                    if let Some(choice) = self.choices.get(self.selected) {
                        OverlayAction::ActionAndDismiss(choice.action.clone())
                    } else {
                        OverlayAction::Consumed
                    }
                } else {
                    OverlayAction::Consumed
                }
            }
            _ => OverlayAction::Consumed,
        }
    }
}

// ── Master overlay enum ───────────────────────────────────────────────────────

/// All possible overlay types. New overlays are added as variants here.
///
/// Complex overlays (ProfileEditor, Settings, PlanEditor) will be migrated
/// incrementally. Until then, they remain as `Option<XxxState>` in App and
/// are checked BEFORE the overlay stack in handle_key.
pub enum OverlayKind {
    /// Error message (dismiss with Enter/Esc/Space).
    Error(ErrorOverlay),
    /// Read-only scrollable content (rules, help).
    ReadOnly(ReadOnlyOverlay),
    /// Multiple-choice dialog.
    Choice(ChoiceOverlay),
    // Future variants as overlays migrate:
    // ModelPicker(ModelPickerOverlay),
    // SessionPicker(SessionPickerOverlay),
    // ProfilePicker(ProfilePickerOverlay),
    // RolePicker(RolePickerOverlay),
    // Permission(PermissionOverlay),
    // ProfileEditor(ProfileEditorOverlay),
    // Settings(SettingsOverlay),
    // PlanEditor(PlanEditorOverlay),
    // PlanTextEditor(PlanTextEditorOverlay),
}

impl OverlayKind {
    /// Route a key event to the active overlay.
    pub fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match self {
            Self::Error(o) => o.handle_key(key),
            Self::ReadOnly(o) => o.handle_key(key),
            Self::Choice(o) => o.handle_key(key),
        }
    }

    /// Route a paste event to the active overlay. Returns true if consumed.
    pub fn handle_paste(&mut self, _text: &str) -> bool {
        match self {
            Self::Error(_) | Self::ReadOnly(_) | Self::Choice(_) => false,
        }
    }

    /// Display title for the overlay.
    pub fn title(&self) -> &str {
        match self {
            Self::Error(o) => &o.title,
            Self::ReadOnly(o) => &o.title,
            Self::Choice(o) => &o.title,
        }
    }
}

// ── Overlay stack ─────────────────────────────────────────────────────────────

/// A stack of overlays. The topmost overlay receives all input.
/// Rendering paints from bottom to top (topmost last = visually on top).
pub struct OverlayStack {
    stack: Vec<OverlayKind>,
}

impl OverlayStack {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    pub fn push(&mut self, overlay: OverlayKind) {
        self.stack.push(overlay);
    }

    pub fn pop(&mut self) -> Option<OverlayKind> {
        self.stack.pop()
    }

    /// Reference to the topmost overlay (receives input).
    pub fn top(&self) -> Option<&OverlayKind> {
        self.stack.last()
    }

    /// Mutable reference to the topmost overlay.
    pub fn top_mut(&mut self) -> Option<&mut OverlayKind> {
        self.stack.last_mut()
    }

    /// Replace the topmost overlay (wizard step transitions).
    pub fn replace_top(&mut self, overlay: OverlayKind) {
        if self.stack.is_empty() {
            self.stack.push(overlay);
        } else {
            let last = self.stack.len() - 1;
            self.stack[last] = overlay;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Iterate overlays bottom-to-top (for rendering).
    pub fn iter(&self) -> impl Iterator<Item = &OverlayKind> {
        self.stack.iter()
    }

    /// Clear all overlays.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Handle a key event: route to topmost overlay, then process the action.
    /// Returns the AppAction if the overlay produced one, or None.
    pub fn handle_key(&mut self, key: KeyEvent) -> OverlayKeyResult {
        let action = if let Some(overlay) = self.stack.last_mut() {
            overlay.handle_key(key)
        } else {
            return OverlayKeyResult::NoOverlay;
        };

        match action {
            OverlayAction::Consumed => OverlayKeyResult::Consumed,
            OverlayAction::NotHandled => OverlayKeyResult::NotHandled,
            OverlayAction::Dismiss => {
                self.stack.pop();
                OverlayKeyResult::Consumed
            }
            OverlayAction::Push(o) => {
                self.stack.push(o);
                OverlayKeyResult::Consumed
            }
            OverlayAction::Replace(o) => {
                self.replace_top(o);
                OverlayKeyResult::Consumed
            }
            OverlayAction::Action(a) => OverlayKeyResult::Action(a),
            OverlayAction::ActionAndDismiss(a) => {
                self.stack.pop();
                OverlayKeyResult::Action(a)
            }
        }
    }

    /// Handle a paste event: route to topmost overlay.
    /// Returns true if consumed (an overlay was active and accepted the paste).
    pub fn handle_paste(&mut self, text: &str) -> bool {
        if let Some(overlay) = self.stack.last_mut() {
            overlay.handle_paste(text)
        } else {
            false
        }
    }
}

impl Default for OverlayStack {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of routing a key event through the overlay stack.
pub enum OverlayKeyResult {
    /// No overlays are active; main input should handle this key.
    NoOverlay,
    /// An overlay consumed the key event.
    Consumed,
    /// The topmost overlay did not handle this key.
    NotHandled,
    /// An overlay produced an app-level action.
    Action(AppAction),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn error_overlay_dismisses_on_enter() {
        let mut overlay = ErrorOverlay::new("test error");
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Enter)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn error_overlay_dismisses_on_esc() {
        let mut overlay = ErrorOverlay::new("test error");
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn error_overlay_consumes_other_keys() {
        let mut overlay = ErrorOverlay::new("test error");
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Char('x'))),
            OverlayAction::Consumed
        ));
    }

    #[test]
    fn readonly_overlay_scrolls() {
        let mut overlay = ReadOnlyOverlay::new(
            "Rules",
            vec![
                ("rule1".into(), "body1".into()),
                ("rule2".into(), "body2".into()),
            ],
        );
        overlay.visible_rows = 1;

        // Scroll down
        overlay.handle_key(key(KeyCode::Down));
        assert_eq!(overlay.scroll_offset, 1);

        // Scroll up
        overlay.handle_key(key(KeyCode::Up));
        assert_eq!(overlay.scroll_offset, 0);

        // Dismiss
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn choice_overlay_navigation() {
        let choices = vec![
            ChoiceItem {
                label: "Option A".into(),
                action: AppAction::Quit,
            },
            ChoiceItem {
                label: "Option B".into(),
                action: AppAction::Quit,
            },
        ];
        let mut overlay = ChoiceOverlay::new("Choose", "Pick one", choices);

        assert_eq!(overlay.selected, 0);
        overlay.handle_key(key(KeyCode::Down));
        assert_eq!(overlay.selected, 1);

        // Wraps
        overlay.handle_key(key(KeyCode::Down));
        assert_eq!(overlay.selected, 0);

        // ESC dismisses
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn choice_overlay_enter_selects() {
        let choices = vec![ChoiceItem {
            label: "Quit".into(),
            action: AppAction::Quit,
        }];
        let mut overlay = ChoiceOverlay::new("Confirm", "Sure?", choices);
        let result = overlay.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            result,
            OverlayAction::ActionAndDismiss(AppAction::Quit)
        ));
    }

    #[test]
    fn stack_empty_returns_no_overlay() {
        let mut stack = OverlayStack::new();
        assert!(matches!(
            stack.handle_key(key(KeyCode::Enter)),
            OverlayKeyResult::NoOverlay
        ));
    }

    #[test]
    fn stack_routes_to_topmost() {
        let mut stack = OverlayStack::new();
        stack.push(OverlayKind::Error(ErrorOverlay::new("first")));
        stack.push(OverlayKind::Error(ErrorOverlay::new("second")));

        // Should dismiss the topmost ("second")
        let result = stack.handle_key(key(KeyCode::Enter));
        assert!(matches!(result, OverlayKeyResult::Consumed));
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.top().unwrap().title(), "Error");
    }

    #[test]
    fn stack_push_from_overlay() {
        let mut stack = OverlayStack::new();
        // Manually test push action
        stack.push(OverlayKind::Error(ErrorOverlay::new("base")));
        assert_eq!(stack.len(), 1);

        stack.push(OverlayKind::ReadOnly(ReadOnlyOverlay::new("Help", vec![])));
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.top().unwrap().title(), "Help");
    }

    #[test]
    fn stack_replace_top() {
        let mut stack = OverlayStack::new();
        stack.push(OverlayKind::Error(ErrorOverlay::new("old")));
        stack.replace_top(OverlayKind::ReadOnly(ReadOnlyOverlay::new("new", vec![])));
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.top().unwrap().title(), "new");
    }

    #[test]
    fn stack_paste_when_empty() {
        let mut stack = OverlayStack::new();
        assert!(!stack.handle_paste("text"));
    }

    #[test]
    fn stack_clear() {
        let mut stack = OverlayStack::new();
        stack.push(OverlayKind::Error(ErrorOverlay::new("a")));
        stack.push(OverlayKind::Error(ErrorOverlay::new("b")));
        stack.clear();
        assert!(stack.is_empty());
    }

    // ── OverlayStack: depth & pop ────────────────────────────────────────────

    #[test]
    fn stack_push_multiple_verify_depth() {
        let mut stack = OverlayStack::new();
        stack.push(OverlayKind::Error(ErrorOverlay::new("a")));
        stack.push(OverlayKind::Error(ErrorOverlay::new("b")));
        stack.push(OverlayKind::ReadOnly(ReadOnlyOverlay::new("c", vec![])));
        assert_eq!(stack.len(), 3);
        assert!(!stack.is_empty());
    }

    #[test]
    fn stack_pop_returns_correct_overlay() {
        let mut stack = OverlayStack::new();
        stack.push(OverlayKind::Error(ErrorOverlay::new("bottom")));
        stack.push(OverlayKind::ReadOnly(ReadOnlyOverlay::new("top", vec![])));

        let popped = stack.pop().expect("should pop an overlay");
        assert_eq!(popped.title(), "top");
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.top().unwrap().title(), "Error");
    }

    #[test]
    fn stack_handle_key_dispatches_to_top_overlay() {
        let mut stack = OverlayStack::new();
        // Bottom: ReadOnly with 5 lines, visible_rows = 2
        let mut ro = ReadOnlyOverlay::new(
            "Bottom",
            (0..5).map(|i| (format!("h{i}"), format!("b{i}"))).collect(),
        );
        ro.visible_rows = 2;
        stack.push(OverlayKind::ReadOnly(ro));

        // Top: Error overlay
        stack.push(OverlayKind::Error(ErrorOverlay::new("Top")));

        // Down key on Error → Consumed (Error consumes all non-dismiss keys)
        let result = stack.handle_key(key(KeyCode::Down));
        assert!(matches!(result, OverlayKeyResult::Consumed));

        // The ReadOnly underneath should NOT have scrolled
        if let OverlayKind::ReadOnly(ro) = &stack.stack[0] {
            assert_eq!(ro.scroll_offset, 0);
        } else {
            panic!("expected ReadOnly at bottom");
        }
    }

    #[test]
    fn stack_handle_key_esc_pops_error_overlay() {
        let mut stack = OverlayStack::new();
        stack.push(OverlayKind::Error(ErrorOverlay::new("will dismiss")));
        assert_eq!(stack.len(), 1);

        let result = stack.handle_key(key(KeyCode::Esc));
        assert!(matches!(result, OverlayKeyResult::Consumed));
        assert!(stack.is_empty());
    }

    #[test]
    fn stack_top_reflects_topmost_overlay() {
        let mut stack = OverlayStack::new();
        stack.push(OverlayKind::Error(ErrorOverlay::with_title("First", "msg")));
        stack.push(OverlayKind::ReadOnly(ReadOnlyOverlay::new(
            "Second",
            vec![],
        )));
        stack.push(OverlayKind::Error(ErrorOverlay::with_title("Third", "msg")));

        assert_eq!(stack.top().unwrap().title(), "Third");
        // iter() paints bottom-to-top; last item is the top overlay
        let titles: Vec<&str> = stack.iter().map(|o| o.title()).collect();
        assert_eq!(titles, vec!["First", "Second", "Third"]);
    }

    #[test]
    fn stack_empty_iter_is_empty() {
        let stack = OverlayStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.iter().count(), 0);
    }

    // ── ErrorOverlay ─────────────────────────────────────────────────────────

    #[test]
    fn error_overlay_with_title_stores_fields() {
        let overlay = ErrorOverlay::with_title("Oops", "something broke");
        assert_eq!(overlay.title, "Oops");
        assert_eq!(overlay.message, "something broke");
    }

    #[test]
    fn error_overlay_new_defaults_title_to_error() {
        let overlay = ErrorOverlay::new("details");
        assert_eq!(overlay.title, "Error");
        assert_eq!(overlay.message, "details");
    }

    #[test]
    fn error_overlay_dismisses_on_space() {
        let mut overlay = ErrorOverlay::new("test");
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Char(' '))),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn error_overlay_consumes_arrow_keys() {
        let mut overlay = ErrorOverlay::new("test");
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Up)),
            OverlayAction::Consumed
        ));
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Down)),
            OverlayAction::Consumed
        ));
    }

    // ── ReadOnlyOverlay ──────────────────────────────────────────────────────

    #[test]
    fn readonly_overlay_scroll_stays_in_bounds() {
        let mut overlay = ReadOnlyOverlay::new(
            "Rules",
            (0..10)
                .map(|i| (format!("h{i}"), format!("b{i}")))
                .collect(),
        );
        overlay.visible_rows = 4;

        // Scroll up at top does nothing
        overlay.handle_key(key(KeyCode::Up));
        assert_eq!(overlay.scroll_offset, 0);

        // Scroll down to max (10 - 4 = 6)
        for _ in 0..20 {
            overlay.handle_key(key(KeyCode::Down));
        }
        assert_eq!(overlay.scroll_offset, 6);

        // k/j vim keys also work
        overlay.handle_key(key(KeyCode::Char('k')));
        assert_eq!(overlay.scroll_offset, 5);
        overlay.handle_key(key(KeyCode::Char('j')));
        assert_eq!(overlay.scroll_offset, 6);
    }

    #[test]
    fn readonly_overlay_page_up_down() {
        let mut overlay = ReadOnlyOverlay::new(
            "Help",
            (0..30)
                .map(|i| (format!("h{i}"), format!("b{i}")))
                .collect(),
        );
        overlay.visible_rows = 5;
        // max scroll = 30 - 5 = 25

        overlay.handle_key(key(KeyCode::PageDown));
        assert_eq!(overlay.scroll_offset, 5);

        overlay.handle_key(key(KeyCode::PageDown));
        assert_eq!(overlay.scroll_offset, 10);

        overlay.handle_key(key(KeyCode::PageUp));
        assert_eq!(overlay.scroll_offset, 5);

        // PageUp past top clamps to 0
        overlay.handle_key(key(KeyCode::PageUp));
        assert_eq!(overlay.scroll_offset, 0);
        overlay.handle_key(key(KeyCode::PageUp));
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn readonly_overlay_esc_and_enter_dismiss() {
        let mut overlay = ReadOnlyOverlay::new("T", vec![]);
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));

        let mut overlay2 = ReadOnlyOverlay::new("T", vec![]);
        assert!(matches!(
            overlay2.handle_key(key(KeyCode::Enter)),
            OverlayAction::Dismiss
        ));
    }
}
