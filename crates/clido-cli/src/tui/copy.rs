//! User-visible strings for the TUI (single place to tweak tone and future i18n).
//!
//! Info lines use a two-space prefix — keep in sync with [`crate::tui::TUI_GUTTER`].

pub mod info {
    pub const BARE_SLASH: &str = "  Type a message or command — bare '/' alone is not sent";
    pub const EMPTY_HINT: &str = "  Type a message to start, or /help for available commands";
    pub const AGENT_NOT_RUNNING: &str = "  ✗ Agent is not running — try restarting clido.";
    #[allow(dead_code)]
    pub const INTERRUPT_QUEUE: &str =
        "  ↻ Interrupt requested — will send after current response completes";
    pub const STOPPING: &str = "  ↻ Stopping...";
    pub const NO_ACTIVE_STOP: &str = "  ✗ No active run to stop";
}

pub mod diff {
    pub fn truncated_banner(total_lines: usize, shown: usize) -> String {
        format!("  … Diff truncated: {total_lines} lines total, showing first {shown}.")
    }

    pub const TRUNCATE_HINT: &str = "  Use git diff or your editor for the full patch.";
}
