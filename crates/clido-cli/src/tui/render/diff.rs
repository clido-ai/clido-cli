//! Side-by-side and unified diff rendering for the TUI chat.
//!
//! When the terminal is wide enough (≥120 columns), diffs are rendered in a
//! GitHub-style two-column layout with old content on the left and new content
//! on the right. Below the threshold, falls back to the standard inline unified
//! diff view.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::*;

/// Minimum terminal width (columns) to use side-by-side rendering.
const SIDE_BY_SIDE_MIN_WIDTH: usize = 120;

/// Width reserved for each line-number gutter (e.g. " 123 ").
const GUTTER_W: usize = 5;

/// The center divider string.
const DIVIDER: &str = "│";

// ── Structured diff representation ──────────────────────────────────────────

/// A single row in a side-by-side view.
struct SbsRow {
    left_lineno: Option<u32>,
    left_text: String,
    right_lineno: Option<u32>,
    right_text: String,
    kind: RowKind,
}

#[derive(Clone, Copy, PartialEq)]
enum RowKind {
    Context,
    Modified,
    Added,
    Deleted,
    Header,
}

// ── Public entry point ──────────────────────────────────────────────────────

/// Render a unified diff string into styled TUI lines.
/// Automatically chooses side-by-side when `width` ≥ 120.
pub(crate) fn render_diff(text: &str, width: usize) -> Vec<Line<'static>> {
    if width >= SIDE_BY_SIDE_MIN_WIDTH {
        render_side_by_side(text, width)
    } else {
        render_unified(text)
    }
}

// ── Unified (inline) renderer — existing logic extracted ────────────────────

fn render_unified(text: &str) -> Vec<Line<'static>> {
    let dim = Modifier::DIM;
    let gutter_style = Style::default().fg(Color::DarkGray).add_modifier(dim);
    let mut out = Vec::new();
    let mut old_lineno: u32 = 0;
    let mut new_lineno: u32 = 0;

    for line in text.lines() {
        if line.starts_with("@@") {
            if let Some((o, n)) = parse_hunk_header(line) {
                old_lineno = o;
                new_lineno = n;
            }
            out.push(Line::from(vec![Span::styled(
                format!("  {}", line),
                Style::default().fg(Color::Cyan).add_modifier(dim),
            )]));
        } else if line.starts_with("---") || line.starts_with("+++") {
            out.push(Line::from(vec![Span::styled(
                format!("  {}", line),
                Style::default().fg(Color::DarkGray).add_modifier(dim),
            )]));
        } else if line.starts_with('+') {
            let ln = new_lineno;
            new_lineno += 1;
            out.push(Line::from(vec![
                Span::styled(format!("     {:>4} ", ln), gutter_style),
                Span::styled(
                    line.to_string(),
                    Style::default().fg(TUI_DIFF_ADD_FG).add_modifier(dim),
                ),
            ]));
        } else if line.starts_with('-') {
            let ln = old_lineno;
            old_lineno += 1;
            out.push(Line::from(vec![
                Span::styled(format!("  {:>4} ", ln), gutter_style),
                Span::styled(
                    line.to_string(),
                    Style::default().fg(TUI_DIFF_DEL_FG).add_modifier(dim),
                ),
            ]));
        } else {
            let ln = new_lineno;
            old_lineno += 1;
            new_lineno += 1;
            out.push(Line::from(vec![
                Span::styled(format!("  {:>4} ", ln), gutter_style),
                Span::styled(line.to_string(), gutter_style),
            ]));
        }
    }
    out.push(Line::raw(""));
    out
}

// ── Side-by-side renderer ───────────────────────────────────────────────────

fn render_side_by_side(text: &str, width: usize) -> Vec<Line<'static>> {
    let rows = parse_into_sbs_rows(text);
    let mut out = Vec::new();

    // Usable width per side: total minus gutters and divider.
    //   Layout: "  " + gutter(5) + content_left + " │ " + " " + gutter(5) + content_right
    let chrome = 2 + GUTTER_W + 3 + 1 + GUTTER_W; // "  NNNN " + " │ " + " NNNN " = 16
    let half = width.saturating_sub(chrome) / 2;

    for row in &rows {
        match row.kind {
            RowKind::Header => {
                // Render file/hunk headers full-width.
                let header = if !row.left_text.is_empty() {
                    &row.left_text
                } else {
                    &row.right_text
                };
                out.push(Line::from(vec![Span::styled(
                    format!("  {}", header),
                    Style::default()
                        .fg(if header.starts_with("@@") {
                            TUI_DIFF_HEADER
                        } else {
                            Color::DarkGray
                        })
                        .add_modifier(Modifier::DIM),
                )]));
            }
            _ => {
                out.push(build_sbs_line(row, half));
            }
        }
    }
    out.push(Line::raw(""));
    out
}

/// Build a single side-by-side line with gutters, content, and divider.
fn build_sbs_line(row: &SbsRow, half_w: usize) -> Line<'static> {
    let dim = Modifier::DIM;
    let gutter_style = Style::default().fg(Color::DarkGray).add_modifier(dim);
    let divider_style = Style::default().fg(Color::DarkGray).add_modifier(dim);

    // Left gutter
    let left_gutter = match row.left_lineno {
        Some(n) => format!("  {:>4} ", n),
        None => "       ".to_string(),
    };

    // Right gutter
    let right_gutter = match row.right_lineno {
        Some(n) => format!(" {:>4} ", n),
        None => "      ".to_string(),
    };

    // Content styling per kind — use the shared palette for consistent diff colors.
    let (left_style, right_style, left_bg, right_bg) = match row.kind {
        RowKind::Deleted => (
            Style::default().fg(TUI_DIFF_DEL_FG).add_modifier(dim),
            gutter_style,
            Some(TUI_DIFF_DEL_BG),
            None,
        ),
        RowKind::Added => (
            gutter_style,
            Style::default().fg(TUI_DIFF_ADD_FG).add_modifier(dim),
            None,
            Some(TUI_DIFF_ADD_BG),
        ),
        RowKind::Modified => (
            Style::default().fg(TUI_DIFF_DEL_FG).add_modifier(dim),
            Style::default().fg(TUI_DIFF_ADD_FG).add_modifier(dim),
            Some(TUI_DIFF_DEL_BG),
            Some(TUI_DIFF_ADD_BG),
        ),
        RowKind::Context | RowKind::Header => (gutter_style, gutter_style, None, None),
    };

    // Pad/truncate content to fixed column width.
    let left_content = pad_or_truncate(&row.left_text, half_w);
    let right_content = pad_or_truncate(&row.right_text, half_w);

    let mut left_content_style = left_style;
    if let Some(bg) = left_bg {
        left_content_style = left_content_style.bg(bg);
    }
    let mut right_content_style = right_style;
    if let Some(bg) = right_bg {
        right_content_style = right_content_style.bg(bg);
    }

    Line::from(vec![
        Span::styled(left_gutter, gutter_style),
        Span::styled(left_content, left_content_style),
        Span::styled(format!(" {} ", DIVIDER), divider_style),
        Span::styled(right_gutter, gutter_style),
        Span::styled(right_content, right_content_style),
    ])
}

/// Pad string to exactly `w` chars, or truncate with `…` if too long.
fn pad_or_truncate(s: &str, w: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= w {
        let mut out = s.to_string();
        for _ in 0..(w - chars.len()) {
            out.push(' ');
        }
        out
    } else if w > 1 {
        let mut out: String = chars[..w - 1].iter().collect();
        out.push('…');
        out
    } else {
        "…".to_string()
    }
}

// ── Diff parser: unified → side-by-side rows ────────────────────────────────

/// Parse a unified diff string into side-by-side rows, pairing consecutive
/// delete/insert blocks as modifications.
fn parse_into_sbs_rows(text: &str) -> Vec<SbsRow> {
    let mut rows = Vec::new();
    let mut old_lineno: u32 = 0;
    let mut new_lineno: u32 = 0;

    // Buffer for consecutive `-` lines waiting to be paired with `+` lines.
    let mut del_buf: Vec<(u32, String)> = Vec::new();

    for line in text.lines() {
        if line.starts_with("@@") {
            flush_del_buf(&mut del_buf, &mut rows);
            if let Some((o, n)) = parse_hunk_header(line) {
                old_lineno = o;
                new_lineno = n;
            }
            rows.push(SbsRow {
                left_lineno: None,
                left_text: line.to_string(),
                right_lineno: None,
                right_text: String::new(),
                kind: RowKind::Header,
            });
        } else if line.starts_with("---") || line.starts_with("+++") {
            flush_del_buf(&mut del_buf, &mut rows);
            rows.push(SbsRow {
                left_lineno: None,
                left_text: line.to_string(),
                right_lineno: None,
                right_text: String::new(),
                kind: RowKind::Header,
            });
        } else if line.starts_with('-') {
            let ln = old_lineno;
            old_lineno += 1;
            // Strip the leading `-` for display.
            let content = line.get(1..).unwrap_or("").to_string();
            del_buf.push((ln, content));
        } else if line.starts_with('+') {
            let ln = new_lineno;
            new_lineno += 1;
            let content = line.get(1..).unwrap_or("").to_string();

            if let Some((old_ln, old_content)) = del_buf.first().cloned() {
                // Pair with a buffered deletion → modified line.
                del_buf.remove(0);
                rows.push(SbsRow {
                    left_lineno: Some(old_ln),
                    left_text: old_content,
                    right_lineno: Some(ln),
                    right_text: content,
                    kind: RowKind::Modified,
                });
            } else {
                // Pure insertion (no paired deletion).
                rows.push(SbsRow {
                    left_lineno: None,
                    left_text: String::new(),
                    right_lineno: Some(ln),
                    right_text: content,
                    kind: RowKind::Added,
                });
            }
        } else {
            // Context line.
            flush_del_buf(&mut del_buf, &mut rows);
            let content = line.get(1..).unwrap_or(line).to_string();
            let oln = old_lineno;
            let nln = new_lineno;
            old_lineno += 1;
            new_lineno += 1;
            rows.push(SbsRow {
                left_lineno: Some(oln),
                left_text: content.clone(),
                right_lineno: Some(nln),
                right_text: content,
                kind: RowKind::Context,
            });
        }
    }
    flush_del_buf(&mut del_buf, &mut rows);
    rows
}

/// Flush remaining unpaired deletions from the buffer.
fn flush_del_buf(buf: &mut Vec<(u32, String)>, rows: &mut Vec<SbsRow>) {
    for (ln, content) in buf.drain(..) {
        rows.push(SbsRow {
            left_lineno: Some(ln),
            left_text: content,
            right_lineno: None,
            right_text: String::new(),
            kind: RowKind::Deleted,
        });
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DIFF: &str = "\
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,5 +1,5 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"world\");
     let x = 1;
 }";

    #[test]
    fn unified_fallback_for_narrow() {
        let lines = render_diff(SAMPLE_DIFF, 80);
        // Should use unified format — look for +/- prefixed content.
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("+"), "expected unified + lines");
        assert!(text.contains("-"), "expected unified - lines");
    }

    #[test]
    fn side_by_side_for_wide() {
        let lines = render_diff(SAMPLE_DIFF, 140);
        // Should use side-by-side — look for the │ divider.
        let has_divider = lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('│')));
        assert!(has_divider, "expected │ divider in side-by-side view");
    }

    #[test]
    fn parse_pairs_modifications() {
        let rows = parse_into_sbs_rows(SAMPLE_DIFF);
        let modified: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == RowKind::Modified)
            .collect();
        assert_eq!(modified.len(), 1, "one modified pair expected");
        assert!(modified[0].left_text.contains("hello"));
        assert!(modified[0].right_text.contains("world"));
    }

    #[test]
    fn parse_pure_addition() {
        let diff = "@@ -1,2 +1,3 @@\n fn a() {\n+    new_line();\n }";
        let rows = parse_into_sbs_rows(diff);
        let added: Vec<_> = rows.iter().filter(|r| r.kind == RowKind::Added).collect();
        assert_eq!(added.len(), 1);
        assert!(added[0].right_text.contains("new_line"));
        assert!(added[0].left_lineno.is_none());
    }

    #[test]
    fn parse_pure_deletion() {
        let diff = "@@ -1,3 +1,2 @@\n fn a() {\n-    old_line();\n }";
        let rows = parse_into_sbs_rows(diff);
        let deleted: Vec<_> = rows.iter().filter(|r| r.kind == RowKind::Deleted).collect();
        assert_eq!(deleted.len(), 1);
        assert!(deleted[0].left_text.contains("old_line"));
        assert!(deleted[0].right_lineno.is_none());
    }

    #[test]
    fn pad_or_truncate_short() {
        assert_eq!(pad_or_truncate("hi", 5), "hi   ");
    }

    #[test]
    fn pad_or_truncate_exact() {
        assert_eq!(pad_or_truncate("hello", 5), "hello");
    }

    #[test]
    fn pad_or_truncate_long() {
        assert_eq!(pad_or_truncate("hello world", 5), "hell…");
    }

    #[test]
    fn context_lines_appear_both_sides() {
        let rows = parse_into_sbs_rows(SAMPLE_DIFF);
        let ctx: Vec<_> = rows.iter().filter(|r| r.kind == RowKind::Context).collect();
        assert!(ctx.len() >= 2, "at least 2 context lines expected");
        for r in &ctx {
            assert!(r.left_lineno.is_some());
            assert!(r.right_lineno.is_some());
            assert_eq!(r.left_text, r.right_text);
        }
    }
}
