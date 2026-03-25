//! Terminal UI helpers: ANSI codes, color detection.

use std::env;
use std::io::{self, IsTerminal};

/// ANSI codes for CLI UI (only when cli_use_color() or setup_use_color()).
/// ux-requirements §7.3: color supports, does not replace, text.
pub mod ansi {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";

    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const CYAN: &str = "\x1b[36m";
    pub const WHITE: &str = "\x1b[37m";
    pub const RED: &str = "\x1b[31m";
    pub const DARK_GRAY: &str = "\x1b[90m";
}

/// Use color for CLI output when any standard stream is a TTY and NO_COLOR is not set.
pub fn cli_use_color() -> bool {
    (io::stdin().is_terminal() || io::stderr().is_terminal() || io::stdout().is_terminal())
        && env::var("NO_COLOR").is_err()
}

/// Use color in setup flow (stdin or stderr TTY).
pub fn setup_use_color() -> bool {
    (io::stdin().is_terminal() || io::stderr().is_terminal()) && env::var("NO_COLOR").is_err()
}

/// True if we should show the rich setup UI (box, welcome line).
pub fn setup_use_rich_ui() -> bool {
    io::stdin().is_terminal() || io::stderr().is_terminal()
}

/// ASCII fallback for setup when not a TTY.
pub const SETUP_BANNER_ASCII: &str = "  --- Clido setup ---\n  Answer each question: type your choice, then press Enter. Defaults in [brackets].";
