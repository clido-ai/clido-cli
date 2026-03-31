use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
};

use crate::tui::{TUI_SELECTION_BG, TUI_SOFT_ACCENT};

// ── Modal component helpers ───────────────────────────────────────────────────

/// Rect anchored just above the input field (grows upward).
pub(crate) fn popup_above_input(input_area: Rect, h: u16, w: u16) -> Rect {
    let w = w.min(input_area.width);
    let x = input_area.x + (input_area.width.saturating_sub(w)) / 2;
    let y = input_area.y.saturating_sub(h);
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Format a timestamp (RFC-3339 or similar) as a relative duration from now.
pub(crate) fn relative_time(ts: &str) -> String {
    let parsed = chrono::DateTime::parse_from_rfc3339(ts).or_else(|_| {
        chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.f")
            .map(|dt: chrono::NaiveDateTime| dt.and_utc().fixed_offset())
    });
    let dt = match parsed {
        Ok(dt) => dt,
        Err(_) => {
            // Fallback: show truncated original
            return if ts.len() >= 16 {
                format!("{} {}", &ts[5..10], &ts[11..16])
            } else {
                ts.to_string()
            };
        }
    };
    let now = chrono::Utc::now();
    let delta = now.signed_duration_since(dt);
    let secs = delta.num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;
    if secs < 60 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{}m ago", mins)
    } else if hours < 24 {
        format!("{}h ago", hours)
    } else if days < 7 {
        format!("{}d ago", days)
    } else if days < 30 {
        format!("{}w ago", days / 7)
    } else if days < 365 {
        format!("{}mo ago", days / 30)
    } else {
        format!("{}y ago", days / 365)
    }
}

/// Styled popup block — same structure for every modal.
pub(crate) fn modal_block(title: &str, border_color: Color) -> Block<'static> {
    Block::default()
        .title(title.to_string())
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

pub(crate) fn modal_block_with_hint(
    title: &str,
    hint: &str,
    border_color: Color,
) -> Block<'static> {
    Block::default()
        .title(title.to_string())
        .title_alignment(Alignment::Left)
        .title_bottom(Line::from(Span::styled(
            hint.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

/// Build a filter indicator line (🔍 + yellow text) for picker popups.
pub(crate) fn filter_indicator_line(filter_text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  🔍 ", Style::default().fg(Color::DarkGray)),
        Span::styled(filter_text.to_string(), Style::default().fg(Color::Yellow)),
    ])
}

/// Build a scroll indicator line showing how many items are above/below the visible window.
pub(crate) fn scroll_indicator_line(above: usize, below: usize) -> Option<Line<'static>> {
    if above == 0 && below == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if above > 0 {
        parts.push(format!("↑↑ {} more", above));
    }
    if below > 0 {
        parts.push(format!("↓↓ {} more", below));
    }
    Some(Line::from(Span::styled(
        format!("  {}", parts.join("  ")),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )))
}

/// Two-column row (e.g. for slash completions): cmd | description, with highlight on selection.
pub(crate) fn modal_row_two_col(
    left: String,
    right: String,
    left_color: Color,
    right_color: Color,
    selected: bool,
) -> Line<'static> {
    let bg = if selected {
        TUI_SELECTION_BG
    } else {
        Color::Reset
    };
    Line::from(vec![
        Span::styled(
            left,
            Style::default()
                .fg(left_color)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(right, Style::default().fg(right_color).bg(bg)),
    ])
}
pub(crate) fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

/// Word-wrap `text` to lines of at most `width` characters.
pub(crate) fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut cur = String::new();
        for word in paragraph.split_whitespace() {
            if cur.is_empty() {
                cur = word.to_string();
            } else if cur.len() + 1 + word.len() <= width {
                cur.push(' ');
                cur.push_str(word);
            } else {
                lines.push(cur);
                cur = word.to_string();
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

/// Drop spans from the right until the total char width fits within `max_width`.
/// Prevents mid-span clipping in single-line bars.
pub(crate) fn fit_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    let mut used = 0usize;
    let mut out = Vec::new();
    for span in spans {
        let w = span.content.chars().count();
        if used + w > max_width {
            break;
        }
        used += w;
        out.push(span);
    }
    out
}

/// Return the semantic color for a tool call based on its type and state.
pub(crate) fn tool_color(name: &str, done: bool, is_error: bool) -> Color {
    if is_error {
        return Color::Red;
    }
    if done {
        return Color::DarkGray;
    }
    match name {
        "Read" | "Glob" | "Grep" => TUI_SOFT_ACCENT,
        "Write" | "Edit" => Color::Green,
        "Bash" => Color::Yellow,
        "SemanticSearch" => Color::Cyan,
        "WebFetch" | "WebSearch" => Color::Magenta,
        "SpawnWorker" | "SpawnReviewer" => Color::LightCyan,
        _ => Color::White,
    }
}

/// Maps internal tool names to human-readable display labels.
pub(crate) fn tool_display_name(name: &str) -> &str {
    match name {
        "SemanticSearch" => "Search",
        "SpawnWorker" => "Worker",
        "SpawnReviewer" => "Reviewer",
        "TodoWrite" => "Todo",
        "WebFetch" => "Fetch",
        "WebSearch" => "Web",
        other => other,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    // ── word_wrap ────────────────────────────────────────────────────────────

    #[test]
    fn word_wrap_short_line_fits() {
        let lines = word_wrap("hello world", 20);
        assert_eq!(lines, vec!["hello world"]);
    }

    #[test]
    fn word_wrap_breaks_at_width() {
        let lines = word_wrap("hello world foo bar", 11);
        assert_eq!(lines, vec!["hello world", "foo bar"]);
    }

    #[test]
    fn word_wrap_single_long_word() {
        let lines = word_wrap("superlongword", 5);
        // A word longer than width is placed on its own line
        assert_eq!(lines, vec!["superlongword"]);
    }

    #[test]
    fn word_wrap_preserves_paragraph_breaks() {
        let lines = word_wrap("first paragraph\nsecond paragraph", 50);
        assert_eq!(lines, vec!["first paragraph", "second paragraph"]);
    }

    #[test]
    fn word_wrap_empty_input() {
        let lines = word_wrap("", 20);
        assert!(lines.is_empty());
    }

    #[test]
    fn word_wrap_multiple_words_wrap_correctly() {
        let lines = word_wrap("a b c d e f", 5);
        // "a b c" fits in 5, "d e f" fits in 5
        assert_eq!(lines, vec!["a b c", "d e f"]);
    }

    // ── truncate_chars ──────────────────────────────────────────────────────

    #[test]
    fn truncate_chars_short_string_unchanged() {
        assert_eq!(truncate_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_chars_exact_length_unchanged() {
        assert_eq!(truncate_chars("hello", 5), "hello");
    }

    #[test]
    fn truncate_chars_long_string_truncated() {
        let result = truncate_chars("hello world", 8);
        assert_eq!(result, "hello w…");
        assert!(result.chars().count() <= 8);
    }

    #[test]
    fn truncate_chars_respects_unicode_boundaries() {
        // "你好世界" is 4 chars; truncate to 3 should give "你好…"
        let result = truncate_chars("你好世界", 3);
        assert_eq!(result, "你好…");
    }

    #[test]
    fn truncate_chars_max_1_gives_ellipsis() {
        let result = truncate_chars("hello", 1);
        assert_eq!(result, "…");
    }

    // ── tool_color ──────────────────────────────────────────────────────────

    #[test]
    fn tool_color_error_is_red() {
        assert_eq!(tool_color("Read", false, true), Color::Red);
        assert_eq!(tool_color("Bash", false, true), Color::Red);
    }

    #[test]
    fn tool_color_done_is_dark_gray() {
        assert_eq!(tool_color("Read", true, false), Color::DarkGray);
        assert_eq!(tool_color("Write", true, false), Color::DarkGray);
    }

    #[test]
    fn tool_color_active_returns_semantic_colors() {
        assert_eq!(tool_color("Read", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Glob", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Grep", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Write", false, false), Color::Green);
        assert_eq!(tool_color("Edit", false, false), Color::Green);
        assert_eq!(tool_color("Bash", false, false), Color::Yellow);
        assert_eq!(tool_color("SemanticSearch", false, false), Color::Cyan);
        assert_eq!(tool_color("WebFetch", false, false), Color::Magenta);
        assert_eq!(tool_color("WebSearch", false, false), Color::Magenta);
        assert_eq!(tool_color("SpawnWorker", false, false), Color::LightCyan);
        assert_eq!(tool_color("SpawnReviewer", false, false), Color::LightCyan);
    }

    #[test]
    fn tool_color_unknown_tool_is_white() {
        assert_eq!(tool_color("UnknownTool", false, false), Color::White);
    }

    #[test]
    fn tool_color_consistent_across_calls() {
        let c1 = tool_color("Bash", false, false);
        let c2 = tool_color("Bash", false, false);
        assert_eq!(c1, c2);
    }

    // ── tool_display_name ───────────────────────────────────────────────────

    #[test]
    fn tool_display_name_maps_known_tools() {
        assert_eq!(tool_display_name("SemanticSearch"), "Search");
        assert_eq!(tool_display_name("SpawnWorker"), "Worker");
        assert_eq!(tool_display_name("SpawnReviewer"), "Reviewer");
        assert_eq!(tool_display_name("TodoWrite"), "Todo");
        assert_eq!(tool_display_name("WebFetch"), "Fetch");
        assert_eq!(tool_display_name("WebSearch"), "Web");
    }

    #[test]
    fn tool_display_name_passes_through_unknown() {
        assert_eq!(tool_display_name("Read"), "Read");
        assert_eq!(tool_display_name("Bash"), "Bash");
        assert_eq!(tool_display_name("CustomTool"), "CustomTool");
    }

    // ── fit_spans ───────────────────────────────────────────────────────────

    #[test]
    fn fit_spans_all_fit() {
        let spans = vec![Span::raw("hello"), Span::raw(" "), Span::raw("world")];
        let result = fit_spans(spans, 20);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn fit_spans_drops_overflow() {
        let spans = vec![Span::raw("hello"), Span::raw(" "), Span::raw("world")];
        let result = fit_spans(spans, 6);
        // "hello" (5) + " " (1) = 6, "world" would overflow
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn fit_spans_empty_input() {
        let result = fit_spans(vec![], 10);
        assert!(result.is_empty());
    }

    // ── relative_time ───────────────────────────────────────────────────────

    #[test]
    fn relative_time_just_now() {
        let now = chrono::Utc::now().to_rfc3339();
        assert_eq!(relative_time(&now), "just now");
    }

    #[test]
    fn relative_time_minutes_ago() {
        let five_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let result = relative_time(&five_min_ago);
        assert!(result.contains("m ago"), "expected minutes, got: {result}");
    }

    #[test]
    fn relative_time_hours_ago() {
        let three_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
        let result = relative_time(&three_hours_ago);
        assert!(result.contains("h ago"), "expected hours, got: {result}");
    }

    #[test]
    fn relative_time_days_ago() {
        let two_days_ago = (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339();
        let result = relative_time(&two_days_ago);
        assert!(result.contains("d ago"), "expected days, got: {result}");
    }

    #[test]
    fn relative_time_future_timestamp_shows_just_now() {
        let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        assert_eq!(relative_time(&future), "just now");
    }

    #[test]
    fn relative_time_invalid_string_fallback() {
        assert_eq!(relative_time("not a timestamp"), "not a timestamp");
    }

    #[test]
    fn relative_time_truncated_fallback_for_long_invalid() {
        let result = relative_time("2024-01-15T10:30:00 not-rfc3339");
        // Should fall through to truncation fallback (len >= 16)
        assert!(!result.is_empty());
    }
}
