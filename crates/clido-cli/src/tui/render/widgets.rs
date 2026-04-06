//! Shared modal primitives: popups above the input, scroll/filter affordances, tool colors.
//!
//! **Overlay hints** use a middle dot (`·`) between clauses so footers read the same
//! everywhere (pickers, errors, permissions, choice dialogs).

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders},
};

use crate::tui::state::StatusEntry;
use crate::tui::{
    TUI_ACCENT, TUI_BORDER_UI, TUI_GUTTER, TUI_MUTED, TUI_ROW_DIM, TUI_SELECTION_BG, TUI_SEP,
    TUI_SOFT_ACCENT, TUI_STATE_ERR, TUI_STATE_INFO, TUI_STATE_OK, TUI_STATUS_RUN,
    TUI_SURFACE_INSET, TUI_TEXT, TUI_TOAST_BG,
};

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

/// Wall-clock start time in local timezone for session lists (explicit calendar context).
pub(crate) fn session_wall_clock(ts: &str) -> String {
    use chrono::Local;
    let parsed = chrono::DateTime::parse_from_rfc3339(ts).or_else(|_| {
        chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.f")
            .map(|dt: chrono::NaiveDateTime| dt.and_utc().fixed_offset())
    });
    match parsed {
        Ok(dt) => {
            let local = dt.with_timezone(&Local);
            local.format("%Y-%m-%d %H:%M").to_string()
        }
        Err(_) => {
            if ts.len() >= 16 {
                format!("{} {}", &ts[0..10], &ts[11..16])
            } else {
                ts.to_string()
            }
        }
    }
}

/// Styled popup block — same structure for every modal.
pub(crate) fn modal_block(title: &str, border_color: Color) -> Block<'static> {
    Block::default()
        .style(Style::default().bg(TUI_TOAST_BG))
        .title(title.to_string())
        .title_alignment(Alignment::Left)
        .border_type(BorderType::Rounded)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

pub(crate) fn modal_block_with_hint(
    title: &str,
    hint: &str,
    border_color: Color,
) -> Block<'static> {
    let hint_trim = hint.trim();
    Block::default()
        .style(Style::default().bg(TUI_TOAST_BG))
        .title(title.to_string())
        .title_alignment(Alignment::Left)
        .title_bottom(Line::from(Span::styled(
            format!(" {hint_trim} "),
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        )))
        .border_type(BorderType::Rounded)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

/// Filter row for picker popups (no emoji — works everywhere).
pub(crate) fn filter_indicator_line(filter_text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{TUI_GUTTER}Filter "),
            Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("› {}", filter_text),
            Style::default().fg(TUI_SOFT_ACCENT),
        ),
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
        format!("{TUI_GUTTER}{}", parts.join(TUI_SEP)),
        Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM),
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
        Span::styled("  ", Style::default().bg(bg)),
        Span::styled(right, Style::default().fg(right_color).bg(bg)),
    ])
}

/// Default border for list-style modals when callers do not need a semantic hue.
pub(crate) fn modal_border_default() -> Color {
    TUI_BORDER_UI
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

/// Status strip: fixed columns (icon · tool name · detail · time) for stable scanning.
/// `max_entries`: when `Some(n)`, only the last `n` entries (oldest dropped) so a 2-row strip
/// does not overflow; when `None`, render all entries (status rail activity section).
pub(crate) fn status_strip_lines(
    entries: &std::collections::VecDeque<StatusEntry>,
    strip_width: u16,
    spinner: &str,
    max_entries: Option<usize>,
) -> Vec<Line<'static>> {
    let w = strip_width as usize;
    const ICON_W: usize = 3;
    const TIME_W: usize = 8;
    const NAME_W: usize = 12;
    let detail_max = w.saturating_sub(ICON_W + NAME_W + TIME_W + 4).max(6);

    let status_style = Style::default().fg(TUI_MUTED).add_modifier(Modifier::DIM);

    let pad_right = |s: String, cols: usize| -> String {
        let n = s.chars().count();
        if n >= cols {
            s
        } else {
            format!("{s}{}", " ".repeat(cols - n))
        }
    };

    let skip = max_entries
        .map(|n| entries.len().saturating_sub(n))
        .unwrap_or(0);

    let mut slines: Vec<Line<'static>> = Vec::new();
    for entry in entries.iter().skip(skip) {
        let time_s = if entry.done {
            let ms = entry.elapsed_ms.unwrap_or(0);
            if ms < 1000 {
                format!("{ms}ms")
            } else {
                format!("{:.1}s", ms as f64 / 1000.0)
            }
        } else {
            let elapsed = entry.start.elapsed();
            let secs = elapsed.as_secs_f64();
            if secs < 1.0 {
                format!("{}ms", elapsed.as_millis())
            } else {
                format!("{:.1}s", secs)
            }
        };
        let time_cell = pad_right(truncate_chars(&time_s, TIME_W), TIME_W);

        let (icon, icon_style) = if entry.done {
            if entry.is_error {
                (
                    "✗",
                    Style::default()
                        .fg(TUI_STATE_ERR)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    "✓",
                    Style::default()
                        .fg(TUI_STATE_OK)
                        .add_modifier(Modifier::DIM),
                )
            }
        } else {
            (
                spinner,
                Style::default()
                    .fg(TUI_STATUS_RUN)
                    .add_modifier(Modifier::DIM),
            )
        };

        let name_cell = pad_right(
            truncate_chars(tool_display_name(&entry.name), NAME_W),
            NAME_W,
        );
        let det = truncate_chars(&entry.detail, detail_max);

        slines.push(Line::from(vec![
            Span::styled(format!(" {} ", icon), icon_style),
            Span::styled(name_cell, icon_style),
            Span::styled(format!(" {}", det), status_style),
            Span::styled(format!(" {}", time_cell), status_style),
        ]));
    }
    while slines.len() < 2 {
        slines.push(Line::raw(""));
    }
    slines
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

/// Tool rows in the transcript: bold status + name, then wrapped detail for scanability.
pub(crate) fn tool_event_lines(
    width: usize,
    name: &str,
    detail: &str,
    done: bool,
    is_error: bool,
) -> Vec<Line<'static>> {
    let color = tool_color(name, done, is_error);
    let icon = if is_error {
        "✗"
    } else if done {
        "✓"
    } else {
        "◌"
    };
    let display_name = tool_display_name(name).to_string();
    let accent = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let detail_style = Style::default().fg(TUI_ROW_DIM).add_modifier(Modifier::DIM);
    let gutter = TUI_GUTTER;

    let mut lines = Vec::new();
    if is_error {
        lines.push(Line::from(vec![Span::styled(
            format!("{gutter}Tool failed — output may be incomplete"),
            Style::default()
                .fg(TUI_STATE_ERR)
                .bg(TUI_SURFACE_INSET)
                .add_modifier(Modifier::DIM),
        )]));
    }
    lines.push(Line::from(vec![
        Span::styled(format!("{gutter}{icon}  "), accent),
        Span::styled(display_name.clone(), accent),
    ]));

    let detail_trim = detail.trim();
    if detail_trim.is_empty() {
        return lines;
    }

    let indent = gutter.chars().count() + 4;
    let wrap_w = width.saturating_sub(indent).max(12);
    for wline in word_wrap(detail_trim, wrap_w) {
        lines.push(Line::from(vec![
            Span::styled(" ".repeat(indent), detail_style),
            Span::styled(wline, detail_style),
        ]));
    }
    lines
}

/// Return the semantic color for a tool call based on its type and state.
pub(crate) fn tool_color(name: &str, done: bool, is_error: bool) -> Color {
    if is_error {
        return TUI_STATE_ERR;
    }
    if done {
        return TUI_MUTED;
    }
    match name {
        "Read" | "Glob" | "Grep" => TUI_SOFT_ACCENT,
        "Write" | "Edit" => TUI_ACCENT,
        "Bash" => TUI_STATUS_RUN,
        "SemanticSearch" => TUI_STATE_INFO,
        "WebFetch" | "WebSearch" => Color::Rgb(200, 155, 235),
        "SpawnWorker" | "SpawnReviewer" => Color::Rgb(130, 210, 215),
        _ => TUI_TEXT,
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
    use crate::tui::{
        TUI_ACCENT, TUI_MUTED, TUI_SOFT_ACCENT, TUI_STATE_ERR, TUI_STATE_INFO, TUI_STATUS_RUN,
        TUI_TEXT,
    };

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
    fn tool_color_error_matches_theme() {
        assert_eq!(tool_color("Read", false, true), TUI_STATE_ERR);
        assert_eq!(tool_color("Bash", false, true), TUI_STATE_ERR);
    }

    #[test]
    fn tool_color_done_is_muted() {
        assert_eq!(tool_color("Read", true, false), TUI_MUTED);
        assert_eq!(tool_color("Write", true, false), TUI_MUTED);
    }

    #[test]
    fn tool_color_active_returns_semantic_colors() {
        assert_eq!(tool_color("Read", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Glob", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Grep", false, false), TUI_SOFT_ACCENT);
        assert_eq!(tool_color("Write", false, false), TUI_ACCENT);
        assert_eq!(tool_color("Edit", false, false), TUI_ACCENT);
        assert_eq!(tool_color("Bash", false, false), TUI_STATUS_RUN);
        assert_eq!(tool_color("SemanticSearch", false, false), TUI_STATE_INFO);
        assert_eq!(
            tool_color("WebFetch", false, false),
            Color::Rgb(200, 155, 235)
        );
        assert_eq!(
            tool_color("WebSearch", false, false),
            Color::Rgb(200, 155, 235)
        );
        assert_eq!(
            tool_color("SpawnWorker", false, false),
            Color::Rgb(130, 210, 215)
        );
        assert_eq!(
            tool_color("SpawnReviewer", false, false),
            Color::Rgb(130, 210, 215)
        );
    }

    #[test]
    fn tool_color_unknown_tool_is_body_text() {
        assert_eq!(tool_color("UnknownTool", false, false), TUI_TEXT);
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

    // ── session_wall_clock ──────────────────────────────────────────────────

    #[test]
    fn session_wall_clock_formats_rfc3339() {
        let s = session_wall_clock("2025-06-15T14:30:00+00:00");
        assert!(
            s.contains("2025") && s.contains(':'),
            "expected calendar date and time in local formatting, got: {s}"
        );
    }
}
