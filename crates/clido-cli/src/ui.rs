//! Terminal UI helpers: ANSI codes, color detection, banners.

use std::env;
use std::io::{self, IsTerminal};

/// ANSI codes for CLI UI (only when cli_use_color() or setup_use_color()).
/// ux-requirements §7.3: color supports, does not replace, text.
pub mod ansi {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const CYAN: &str = "\x1b[36m";
    pub const BRIGHT_CYAN: &str = "\x1b[96m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const RED: &str = "\x1b[31m";
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

/// ASCII banner shown on interactive text startup.
pub const BANNER: &str = r#"          ▄▄   ▄▄                ▄▄
        ▀███   ██              ▀███
          ██                     ██
 ▄██▀██   ██ ▀███    ██     ▄█▀▀███   ▄██▀██▄
██▀  ██   ██   ██    ▀▀   ▄██    ██  ██▀   ▀██
██        ██   ██         ███    ██  ██     ██
██▄    ▄  ██   ██    ▄▄   ▀██    ██  ██▄   ▄██
 █████▀ ▄████▄████▄  ▀█    ▀████▀███▄ ▀█████▀
                      ▀

"#;

/// ASCII fallback for setup when not a TTY.
pub const SETUP_BANNER_ASCII: &str = "  --- Clido setup ---\n  Answer each question: type your choice, then press Enter. Defaults in [brackets].";

/// Inner width of the setup box.
const SETUP_BOX_WIDTH: usize = 59;

/// Build the rich setup box with exactly SETUP_BOX_WIDTH chars per line so borders align.
pub fn setup_banner_rich() -> String {
    let pad = |s: &str| {
        let n = s.chars().count();
        format!("{}{}", s, " ".repeat(SETUP_BOX_WIDTH.saturating_sub(n)))
    };
    let wrap = |s: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut cur_len = 0usize;
        for word in s.split_whitespace() {
            let wl = word.chars().count();
            let sep = if cur.is_empty() { 0 } else { 1 };
            if cur_len + sep + wl > SETUP_BOX_WIDTH {
                out.push(cur);
                cur = word.to_string();
                cur_len = wl;
            } else {
                if !cur.is_empty() {
                    cur.push(' ');
                    cur_len += 1;
                }
                cur.push_str(word);
                cur_len += wl;
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    };
    let mut lines = vec![
        "  Clido setup".to_string(),
        "  Choose a provider and where to store your API key.".to_string(),
    ];
    lines.extend(wrap(
        "  Answer the questions below; use arrow keys or type, then Enter.",
    ));
    lines.push("  Defaults are in brackets.".to_string());
    let body = lines
        .iter()
        .map(|l| format!("║{}║", pad(l)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "╔{}╗\n{}\n╚{}╝",
        "═".repeat(SETUP_BOX_WIDTH),
        body,
        "═".repeat(SETUP_BOX_WIDTH),
    )
}
