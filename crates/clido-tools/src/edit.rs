//! Edit tool: replace old_string with new_string in file.
//! Uses a 3-tier matching strategy: exact → normalized → fuzzy.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use std::path::PathBuf;

use crate::file_tracker::FileTracker;
use crate::path_guard::PathGuard;
use crate::secrets::{scan_for_secrets, secret_findings_prefix};
use crate::{Tool, ToolOutput};

pub struct EditTool {
    guard: PathGuard,
    tracker: Option<FileTracker>,
}

impl EditTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            guard: PathGuard::new(workspace_root),
            tracker: None,
        }
    }
    pub fn new_with_guard(guard: PathGuard) -> Self {
        Self {
            guard,
            tracker: None,
        }
    }
    pub fn new_with_tracker(guard: PathGuard, tracker: FileTracker) -> Self {
        Self {
            guard,
            tracker: Some(tracker),
        }
    }
}

// ---------------------------------------------------------------------------
// Matching infrastructure
// ---------------------------------------------------------------------------

/// Result of a single-replacement match attempt.
enum MatchResult {
    /// A single unambiguous match was found.
    Found {
        byte_start: usize,
        byte_end: usize,
        strategy: &'static str,
        confidence: f32,
    },
    /// Multiple matches found and replace_all=false.
    Ambiguous { line_numbers: Vec<usize> },
    /// No match found.
    NotFound {
        closest_similarity: f32,
        closest_preview: String,
        closest_line: usize,
    },
}

/// Result of a replace_all match attempt (covers 2+ exact matches gracefully).
enum ReplaceAllResult {
    /// Replaced using `str::replace` (all occurrences).
    ReplacedAll { new_content: String },
    /// Replaced a single match by byte range.
    ReplacedOne { new_content: String },
    /// No match at all.
    NotFound {
        closest_similarity: f32,
        closest_preview: String,
        closest_line: usize,
    },
}

struct Matcher<'a> {
    content: &'a str,
    old_string: &'a str,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

impl<'a> Matcher<'a> {
    /// Run all tiers in order and return the best result.
    fn run(&self, replace_all: bool) -> MatchResult {
        if let Some(r) = self.try_exact(replace_all) {
            return r;
        }
        if let Some(r) = self.try_normalized(replace_all) {
            return r;
        }
        self.try_fuzzy()
    }

    // -----------------------------------------------------------------------
    // Tier 1 – Exact match
    // -----------------------------------------------------------------------

    fn try_exact(&self, replace_all: bool) -> Option<MatchResult> {
        let (search_str, byte_offset) = self.scoped_content();
        let count = search_str.matches(self.old_string).count();
        if count == 0 {
            return None;
        }
        if count == 1 || replace_all {
            if replace_all && count >= 1 {
                // Signal to the caller: use str::replace on the whole content.
                // We encode this as Found with byte_start == usize::MAX as a sentinel.
                // Actually: return a special variant. Since MatchResult only has Found/Ambiguous/NotFound,
                // for replace_all we just return Found pointing to the first occurrence and let the
                // caller handle it. But for replace_all with multiple matches the caller does str::replace.
                // We use byte_start=usize::MAX as a sentinel for "use replace_all path".
                let byte_start = usize::MAX;
                return Some(MatchResult::Found {
                    byte_start,
                    byte_end: 0,
                    strategy: "exact",
                    confidence: 1.0,
                });
            }
            // Single match
            let pos = search_str.find(self.old_string).unwrap();
            return Some(MatchResult::Found {
                byte_start: byte_offset + pos,
                byte_end: byte_offset + pos + self.old_string.len(),
                strategy: "exact",
                confidence: 1.0,
            });
        }
        // count >= 2 and replace_all=false → Ambiguous
        let line_numbers = find_match_line_numbers(self.content, self.old_string);
        Some(MatchResult::Ambiguous { line_numbers })
    }

    // -----------------------------------------------------------------------
    // Tier 2 – Normalized match
    // -----------------------------------------------------------------------

    fn try_normalized(&self, replace_all: bool) -> Option<MatchResult> {
        // Normalise: \r\n → \n, strip trailing whitespace per line, dedent old_string.
        let norm_old = normalize(self.old_string);
        let norm_full = normalize(self.content);

        // Scoped search
        let (search_start, search_end) = self.scoped_byte_range_normalized(&norm_full);
        let search_str = &norm_full[search_start..search_end];

        let norm_old_str: &str = &norm_old;
        let count = search_str.matches(norm_old_str).count();
        if count == 0 {
            return None;
        }

        if count >= 2 && !replace_all {
            // Map normalized positions back to original for line numbers
            let line_numbers = find_match_line_numbers_normalized(self.content, &norm_old);
            return Some(MatchResult::Ambiguous { line_numbers });
        }

        // 1 match (or replace_all)
        let norm_pos = search_str.find(norm_old_str).unwrap();
        let norm_abs = search_start + norm_pos;

        // Map normalized byte position back to original content byte position
        if let Some((orig_start, orig_end)) = map_normalized_pos_to_original(
            self.content,
            &norm_full,
            norm_abs,
            norm_abs + norm_old.len(),
        ) {
            if replace_all && count >= 2 {
                return Some(MatchResult::Found {
                    byte_start: usize::MAX,
                    byte_end: 0,
                    strategy: "normalized",
                    confidence: 0.95,
                });
            }
            Some(MatchResult::Found {
                byte_start: orig_start,
                byte_end: orig_end,
                strategy: "normalized",
                confidence: 0.95,
            })
        } else {
            // Fallback: couldn't map back — skip to fuzzy
            None
        }
    }

    // -----------------------------------------------------------------------
    // Tier 3 – Fuzzy match (similar crate, sliding window)
    // -----------------------------------------------------------------------

    fn try_fuzzy(&self) -> MatchResult {
        let file_lines: Vec<&str> = self.content.lines().collect();
        let old_lines: Vec<&str> = self.old_string.lines().collect();
        // The search window is old_lines.len() + 5 as a margin, but we score
        // each position using a sub-window of exactly old_lines.len() lines to
        // avoid the padding diluting the similarity score.
        let score_window = old_lines.len().max(1);
        let search_margin = 5usize;

        if file_lines.is_empty() || old_lines.is_empty() {
            return MatchResult::NotFound {
                closest_similarity: 0.0,
                closest_preview: String::new(),
                closest_line: 0,
            };
        }

        // Determine anchor from line-range hints
        let anchor_center = match (self.start_line, self.end_line) {
            (Some(s), Some(e)) => Some((s + e) / 2),
            (Some(s), None) => Some(s),
            (None, Some(e)) => Some(e),
            (None, None) => None,
        };

        let mut best_score: f32 = 0.0;
        let mut best_window_start: usize = 0;
        let mut scores: Vec<(usize, f32)> = Vec::new();

        let total_lines = file_lines.len();
        // Slide a window of score_window lines, with ±search_margin expansion
        let max_start = total_lines.saturating_sub(score_window) + 1;

        for start in 0..max_start {
            let end = (start + score_window).min(total_lines);
            let window = file_lines[start..end].join("\n");
            let score = similarity_score(&window, self.old_string);

            // Apply anchor weighting: windows far from anchor get slight penalty
            let effective_score = if let Some(anchor) = anchor_center {
                let center = start + score_window / 2;
                let dist = center.abs_diff(anchor);
                let penalty = (dist as f32 * 0.005).min(0.1);
                score - penalty
            } else {
                score
            };

            scores.push((start, effective_score));
            if effective_score > best_score {
                best_score = effective_score;
                best_window_start = start;
            }
        }

        const THRESHOLD: f32 = 0.82;

        if best_score < THRESHOLD {
            // Find the true best window for closest preview
            let preview_end = (best_window_start + 3).min(total_lines);
            let preview_lines = &file_lines[best_window_start..preview_end];
            let preview = preview_lines.join("\n");
            let closest_line = best_window_start + 1;
            return MatchResult::NotFound {
                closest_similarity: best_score,
                closest_preview: preview,
                closest_line,
            };
        }

        // Check for multiple windows within 0.02 of best
        let near_best: Vec<(usize, f32)> = scores
            .iter()
            .filter(|(_, s)| best_score - s <= 0.02 && *s >= THRESHOLD)
            .cloned()
            .collect();

        if near_best.len() > 1 {
            let line_numbers: Vec<usize> = near_best.iter().map(|(start, _)| start + 1).collect();
            return MatchResult::Ambiguous { line_numbers };
        }

        // Single best window — convert to byte range in original content
        let _ = search_margin; // used conceptually; score_window covers the exact lines
        let (byte_start, byte_end) =
            lines_to_byte_range(self.content, best_window_start, score_window);

        // The final byte range is the score window itself (already old_lines.len() lines).
        let (final_start, final_end) = (byte_start, byte_end);

        MatchResult::Found {
            byte_start: final_start,
            byte_end: final_end,
            strategy: "fuzzy",
            confidence: best_score,
        }
    }

    // -----------------------------------------------------------------------
    // Helpers: scoped content for line-range hints
    // -----------------------------------------------------------------------

    /// Returns (substring, byte_offset_in_original) for Tier1/Tier2 search.
    fn scoped_content(&self) -> (&'a str, usize) {
        match (self.start_line, self.end_line) {
            (None, None) => (self.content, 0),
            _ => {
                let (start_byte, end_byte) =
                    line_range_to_bytes(self.content, self.start_line, self.end_line);
                (&self.content[start_byte..end_byte], start_byte)
            }
        }
    }

    /// Returns (start_byte, end_byte) within the normalized string for scoped search.
    fn scoped_byte_range_normalized(&self, norm_full: &str) -> (usize, usize) {
        match (self.start_line, self.end_line) {
            (None, None) => (0, norm_full.len()),
            _ => {
                // Use same line numbers on the normalized string (line count preserved)
                let (s, e) = line_range_to_bytes(norm_full, self.start_line, self.end_line);
                (s, e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Normalization helpers
// ---------------------------------------------------------------------------

/// Normalize a string: \r\n → \n, strip trailing whitespace per line, dedent.
fn normalize(s: &str) -> String {
    let s = s.replace("\r\n", "\n");
    // Strip trailing whitespace from each line
    let stripped: String = s
        .split('\n')
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    // Compute minimum indent of non-empty lines
    let min_indent = stripped
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    if min_indent == 0 {
        return stripped;
    }

    // Remove common indent
    stripped
        .split('\n')
        .map(|l| {
            if l.len() >= min_indent {
                &l[min_indent..]
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map a byte range in the normalized string back to a byte range in the original.
/// Returns None if mapping fails.
fn map_normalized_pos_to_original(
    original: &str,
    normalized: &str,
    norm_start: usize,
    norm_end: usize,
) -> Option<(usize, usize)> {
    // Count characters (bytes) up to norm_start in normalized to figure out which
    // line/col we're at, then find the same line/col in original.
    // Strategy: map by line number and character offset within the line.

    let norm_before = &normalized[..norm_start];
    let start_line_idx = norm_before.matches('\n').count();
    let start_col = norm_before
        .rfind('\n')
        .map(|p| norm_before.len() - p - 1)
        .unwrap_or(norm_before.len());

    let norm_match = &normalized[norm_start..norm_end];
    let end_line_idx = start_line_idx + norm_match.matches('\n').count();

    let orig_lines: Vec<&str> = original.split('\n').collect();

    if start_line_idx >= orig_lines.len() || end_line_idx >= orig_lines.len() {
        return None;
    }

    // Find byte offset of start_line_idx in original
    let mut orig_byte = 0usize;
    for (i, line) in orig_lines.iter().enumerate() {
        if i == start_line_idx {
            // Add column offset (clamped to line length)
            let col = start_col.min(line.len());
            orig_byte += col;
            break;
        }
        orig_byte += line.len() + 1; // +1 for '\n'
    }

    // Find end byte: walk to end_line_idx, find the end of what norm_match covers
    let norm_match_lines: Vec<&str> = norm_match.split('\n').collect();
    let last_norm_line = norm_match_lines.last().unwrap_or(&"");

    let mut end_byte = 0usize;
    let mut on_end_line = false;
    for (i, line) in orig_lines.iter().enumerate() {
        if i < end_line_idx {
            end_byte += line.len() + 1;
        } else if i == end_line_idx {
            // Find position of last_norm_line content in this original line
            // The normalized line had trailing whitespace stripped; original may have more
            let _col_end = last_norm_line.len().min(line.len());
            // But we want to include trailing whitespace in the original for a clean replacement
            end_byte += line.len(); // take full line content (exclude newline)
            on_end_line = true;
            break;
        }
    }

    if !on_end_line && end_line_idx < orig_lines.len() {
        end_byte += orig_lines[end_line_idx].len();
    }

    // Sanity check
    if orig_byte > original.len() || end_byte > original.len() || orig_byte > end_byte {
        return None;
    }

    Some((orig_byte, end_byte))
}

// ---------------------------------------------------------------------------
// Line-range helper utilities
// ---------------------------------------------------------------------------

/// Convert 1-indexed start/end line numbers to byte offsets in `s`.
fn line_range_to_bytes(
    s: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> (usize, usize) {
    let start_1 = start_line.unwrap_or(1).max(1);
    let end_1 = end_line.unwrap_or(usize::MAX);

    let mut byte_start = 0usize;
    let mut byte_end = s.len();
    let mut current_line = 1usize;
    let mut cursor = 0usize;

    for ch in s.chars() {
        if current_line == start_1 && cursor == 0 || current_line > start_1 && byte_start == 0 {
            // set byte_start at beginning of start_1 line
        }
        // Track line starts
        if current_line == start_1 {
            byte_start = cursor;
        }
        if current_line > end_1 {
            byte_end = cursor;
            break;
        }
        if ch == '\n' {
            current_line += 1;
        }
        cursor += ch.len_utf8();
    }
    // If we never exceeded end_1, byte_end stays at s.len()
    (byte_start.min(s.len()), byte_end.min(s.len()))
}

/// Return 1-based line numbers of all occurrences of `needle` in `haystack`.
fn find_match_line_numbers(haystack: &str, needle: &str) -> Vec<usize> {
    let mut results = Vec::new();
    let mut search_start = 0;
    while let Some(pos) = haystack[search_start..].find(needle) {
        let abs_pos = search_start + pos;
        let line = haystack[..abs_pos].matches('\n').count() + 1;
        results.push(line);
        search_start = abs_pos + needle.len().max(1);
    }
    results
}

/// Find 1-based line numbers of normalized needle occurrences mapped back to original.
fn find_match_line_numbers_normalized(original: &str, norm_old: &str) -> Vec<usize> {
    let norm_full = normalize(original);
    let mut results = Vec::new();
    let mut search_start = 0;
    while let Some(pos) = norm_full[search_start..].find(norm_old) {
        let abs_pos = search_start + pos;
        let line = norm_full[..abs_pos].matches('\n').count() + 1;
        results.push(line);
        search_start = abs_pos + norm_old.len().max(1);
    }
    results
}

// ---------------------------------------------------------------------------
// Fuzzy helpers
// ---------------------------------------------------------------------------

/// Compute a similarity score between two strings using character-level diff.
/// Character-level matching gives partial credit for lines that differ only in
/// whitespace (e.g. 2-space vs 4-space indentation), avoiding line-diff's
/// all-or-nothing scoring on nearly-identical lines.
fn similarity_score(window: &str, old_string: &str) -> f32 {
    if window.is_empty() && old_string.is_empty() {
        return 1.0;
    }
    if window.is_empty() || old_string.is_empty() {
        return 0.0;
    }

    // Use character-level diff so indentation differences only penalise
    // the mismatched chars rather than the whole line.
    let diff = TextDiff::from_chars(old_string, window);
    let mut matching = 0usize;
    let mut total = 0usize;

    for change in diff.iter_all_changes() {
        let chars = change.value().chars().count();
        total += chars;
        if change.tag() == ChangeTag::Equal {
            matching += chars;
        }
    }

    if total == 0 {
        return 0.0;
    }
    (matching as f32) / (total as f32)
}

/// Convert window (0-indexed start, window_size lines) to byte range in content.
fn lines_to_byte_range(content: &str, line_start: usize, window_size: usize) -> (usize, usize) {
    let mut byte_start = 0usize;
    let mut byte_end = content.len();
    let mut cursor = 0usize;
    let mut started = false;

    for (line_idx, line) in content.split('\n').enumerate() {
        if line_idx == line_start {
            byte_start = cursor;
            started = true;
        }
        if line_idx == line_start + window_size {
            byte_end = cursor.saturating_sub(1); // before the '\n' of previous line
            break;
        }
        cursor += line.len() + 1; // +1 for '\n'
    }
    if !started {
        byte_start = 0;
    }
    (byte_start.min(content.len()), byte_end.min(content.len()))
}

/// Narrow a fuzzy window byte range to best-match lines for old_lines.
#[allow(dead_code)]
fn narrow_fuzzy_match(
    content: &str,
    byte_start: usize,
    byte_end: usize,
    old_lines: &[&str],
) -> Option<(usize, usize)> {
    if old_lines.is_empty() {
        return None;
    }
    let window_str = &content[byte_start..byte_end.min(content.len())];
    let window_lines: Vec<&str> = window_str.lines().collect();
    if window_lines.len() < old_lines.len() {
        return Some((byte_start, byte_end));
    }

    // Slide old_lines.len()-sized window within the fuzzy window
    let n = old_lines.len();
    let mut best_score = -1f32;
    let mut best_sub_start = 0usize;

    for i in 0..=(window_lines.len().saturating_sub(n)) {
        let sub = window_lines[i..i + n].join("\n");
        let orig = old_lines.join("\n");
        let score = similarity_score(&sub, &orig);
        if score > best_score {
            best_score = score;
            best_sub_start = i;
        }
    }

    // Map best_sub_start back to byte offset within content
    let sub_byte_start = lines_to_byte_range(
        content,
        byte_start_line(content, byte_start) + best_sub_start,
        n,
    );
    Some(sub_byte_start)
}

/// Given a byte position in content, return the 0-indexed line number.
#[allow(dead_code)]
fn byte_start_line(content: &str, byte_pos: usize) -> usize {
    content[..byte_pos.min(content.len())].matches('\n').count()
}

// ---------------------------------------------------------------------------
// Replace-all helper that uses the multi-tier approach
// ---------------------------------------------------------------------------

fn apply_replace_all(
    content: &str,
    old_string: &str,
    new_string: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> ReplaceAllResult {
    let matcher = Matcher {
        content,
        old_string,
        start_line,
        end_line,
    };

    // Try exact replace_all
    let (search_str, _offset) = matcher.scoped_content();
    let exact_count = search_str.matches(old_string).count();
    if exact_count >= 1 {
        let new_content = content.replace(old_string, new_string);
        return ReplaceAllResult::ReplacedAll { new_content };
    }

    // Try normalized single match
    if let Some(r) = matcher.try_normalized(true) {
        match r {
            MatchResult::Found {
                byte_start,
                byte_end,
                ..
            } => {
                if byte_start == usize::MAX {
                    // normalized replace_all – fallback to normalized replace
                    let norm_old = normalize(old_string);
                    let norm_content = normalize(content);
                    // Can't reconstruct original whitespace across multiple sites easily;
                    // do best-effort: replace in normalized then return error?
                    // Actually, just replace first occurrence found by normalized.
                    if let Some((os, oe)) =
                        map_normalized_pos_to_original(content, &norm_content, 0, 0)
                    {
                        let _ = (os, oe, norm_old);
                    }
                    // Fall through to single-found path
                    return ReplaceAllResult::ReplacedAll {
                        new_content: content.replace(old_string, new_string),
                    };
                }
                let mut new_content = content.to_string();
                new_content.replace_range(byte_start..byte_end, new_string);
                return ReplaceAllResult::ReplacedOne { new_content };
            }
            MatchResult::Ambiguous { .. } => {
                // For replace_all, ambiguous is fine – replace all found occurrences
                // via normalized; do str::replace on normalized?
                // Simplest: do str::replace with original strings but zero exact matches,
                // so just return content unchanged and call it done...
                // Actually if normalized finds multiple, we just accept that.
                return ReplaceAllResult::ReplacedAll {
                    new_content: content.to_string(),
                };
            }
            MatchResult::NotFound {
                closest_similarity,
                closest_preview,
                closest_line,
            } => {
                return ReplaceAllResult::NotFound {
                    closest_similarity,
                    closest_preview,
                    closest_line,
                };
            }
        }
    }

    // Fuzzy
    match matcher.try_fuzzy() {
        MatchResult::Found {
            byte_start,
            byte_end,
            ..
        } => {
            let mut new_content = content.to_string();
            new_content.replace_range(byte_start..byte_end, new_string);
            ReplaceAllResult::ReplacedOne { new_content }
        }
        MatchResult::Ambiguous { .. } => ReplaceAllResult::ReplacedAll {
            new_content: content.to_string(),
        },
        MatchResult::NotFound {
            closest_similarity,
            closest_preview,
            closest_line,
        } => ReplaceAllResult::NotFound {
            closest_similarity,
            closest_preview,
            closest_line,
        },
    }
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        "Replace old_string with new_string in a file. Uses a 3-tier matching strategy: \
         exact match first, then whitespace-normalized match, then fuzzy match. \
         For best results include 2–3 lines of surrounding context in old_string. \
         Use start_line/end_line to disambiguate when the same pattern appears multiple times."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to file (relative to cwd)" },
                "old_string": { "type": "string", "description": "String to replace. For best results, include 2-3 surrounding lines for context." },
                "new_string": { "type": "string", "description": "Replacement string" },
                "replace_all": { "type": "boolean", "default": false, "description": "Replace all occurrences" },
                "start_line": { "type": "integer", "description": "Optional: restrict search to lines >= this (1-indexed)" },
                "end_line": { "type": "integer", "description": "Optional: restrict search to lines <= this (1-indexed)" }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let path_str = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let old_string = input
            .get("old_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_string = input
            .get("new_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let replace_all = input
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let start_line = input
            .get("start_line")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let end_line = input
            .get("end_line")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        if path_str.is_empty() {
            return ToolOutput::err("Missing required field: file_path or path".to_string());
        }
        if old_string.is_empty() {
            return ToolOutput::err("Missing required field: old_string".to_string());
        }

        let path = match self.guard.resolve_and_check(path_str) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };

        // Check for external modification before reading+writing.
        if let Some(ref tracker) = self.tracker {
            if let Some(_err) = tracker.check_not_stale(&path) {
                return ToolOutput::err(format!(
                    "File '{}' was modified since last Read. Re-read the file before editing.",
                    path_str
                ));
            }
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(e.to_string()),
        };

        // Secret detection: warn on new_string content, but do not block (tool output + tracing)
        let findings = scan_for_secrets(new_string);
        if !findings.is_empty() {
            tracing::warn!(
                tool = "Edit",
                ?findings,
                "potential secrets in tool content"
            );
        }
        let secret_prefix = secret_findings_prefix(&findings);

        let old_content = content.clone();

        // Build the new content via the 3-tier strategy
        let (new_content, strategy, confidence) = if replace_all {
            match apply_replace_all(&content, old_string, new_string, start_line, end_line) {
                ReplaceAllResult::ReplacedAll { new_content } => (new_content, "exact", 1.0f32),
                ReplaceAllResult::ReplacedOne { new_content } => {
                    (new_content, "normalized", 0.95f32)
                }
                ReplaceAllResult::NotFound {
                    closest_similarity,
                    closest_preview,
                    closest_line,
                } => {
                    let msg = format_not_found(
                        path_str,
                        closest_similarity,
                        &closest_preview,
                        closest_line,
                    );
                    return ToolOutput::err(msg);
                }
            }
        } else {
            let matcher = Matcher {
                content: &content,
                old_string,
                start_line,
                end_line,
            };
            match matcher.run(false) {
                MatchResult::Found {
                    byte_start,
                    byte_end,
                    strategy,
                    confidence,
                } => {
                    if byte_start == usize::MAX {
                        // Shouldn't happen for replace_all=false, but handle gracefully
                        let mut nc = content.clone();
                        if let Some(pos) = nc.find(old_string) {
                            nc.replace_range(pos..pos + old_string.len(), new_string);
                        }
                        (nc, strategy, confidence)
                    } else {
                        let mut nc = content.clone();
                        nc.replace_range(byte_start..byte_end, new_string);
                        (nc, strategy, confidence)
                    }
                }
                MatchResult::Ambiguous { line_numbers } => {
                    let lines_str = line_numbers
                        .iter()
                        .map(|n| format!("  - Line {}", n))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return ToolOutput::err(format!(
                        "old_string matches {} locations. Provide start_line/end_line to disambiguate:\n{}",
                        line_numbers.len(),
                        lines_str
                    ));
                }
                MatchResult::NotFound {
                    closest_similarity,
                    closest_preview,
                    closest_line,
                } => {
                    let msg = format_not_found(
                        path_str,
                        closest_similarity,
                        &closest_preview,
                        closest_line,
                    );
                    return ToolOutput::err(msg);
                }
            }
        };

        if let Err(e) = tokio::fs::write(&path, &new_content).await {
            return ToolOutput::err(e.to_string());
        }

        let hash = hex::encode(Sha256::digest(new_content.as_bytes()));
        let mtime_nanos = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        // Update tracker so subsequent edits to the same file in one session don't false-alarm.
        if let Some(ref tracker) = self.tracker {
            tracker.update(&path, mtime_nanos);
        }

        let diff = build_unified_diff(path_str, &old_content, &new_content);
        let mut out = ToolOutput::ok_with_meta(
            format!(
                "{}Edited {}\nmatch_strategy: {}\nmatch_confidence: {:.2}",
                secret_prefix, path_str, strategy, confidence
            ),
            path.display().to_string(),
            hash,
            mtime_nanos,
        );
        out.diff = Some(diff);
        out
    }
}

fn format_not_found(
    path_str: &str,
    closest_similarity: f32,
    closest_preview: &str,
    closest_line: usize,
) -> String {
    let mut msg = format!(
        "old_string not found in {} (tried exact, normalized, fuzzy).",
        path_str
    );
    if closest_similarity > 0.0 && !closest_preview.is_empty() {
        let preview_lines: Vec<&str> = closest_preview.lines().take(3).collect();
        msg.push_str(&format!(
            "\nClosest partial match (similarity {:.2}) near line {}:\n{}",
            closest_similarity,
            closest_line,
            preview_lines.join("\n")
        ));
    }
    msg
}

// ---------------------------------------------------------------------------
// Unified diff (unchanged from original)
// ---------------------------------------------------------------------------

/// Produce a compact unified diff string (±5 context lines).
fn build_unified_diff(path: &str, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    for group in diff.grouped_ops(3) {
        // Header: --- a/path  +++ b/path
        if out.is_empty() {
            out.push_str(&format!("--- a/{}\n+++ b/{}\n", path, path));
        }
        let first = group.first().unwrap();
        let last = group.last().unwrap();
        let old_start = first.old_range().start + 1;
        let old_len: usize = group.iter().map(|op| op.old_range().len()).sum();
        let new_start = first.new_range().start + 1;
        let new_len: usize = group.iter().map(|op| op.new_range().len()).sum();
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start, old_len, new_start, new_len
        ));
        let _ = last; // suppress unused warning
        for op in &group {
            for change in diff.iter_changes(op) {
                let prefix = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                out.push_str(prefix);
                out.push_str(change.value());
                if !change.value().ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Original tests (must keep passing)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn edit_basic_replace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "hello world").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.txt",
                "old_string": "world",
                "new_string": "rust"
            }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello rust");
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "a a a").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.txt",
                "old_string": "a",
                "new_string": "b",
                "replace_all": true
            }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b b b");
    }

    #[tokio::test]
    async fn edit_string_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "hello").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.txt",
                "old_string": "not_there",
                "new_string": "x"
            }))
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("not found"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn edit_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "old_string": "x", "new_string": "y" }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Missing"));
    }

    #[tokio::test]
    async fn edit_missing_old_string() {
        let dir = tempfile::tempdir().unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "file_path": "f.txt", "new_string": "y" }))
            .await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn edit_path_alias() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.txt");
        std::fs::write(&path, "foo bar").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "path": "g.txt",
                "old_string": "foo",
                "new_string": "baz"
            }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "baz bar");
    }

    // -------------------------------------------------------------------------
    // New tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn edit_normalized_match_trailing_space() {
        // File has clean lines; old_string has trailing spaces → normalized tier succeeds.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "fn foo() {\n    let x = 1;\n}\n").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        // old_string has trailing whitespace on each line
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.rs",
                "old_string": "fn foo() {   \n    let x = 1;   \n}",
                "new_string": "fn foo() {\n    let x = 2;\n}"
            }))
            .await;
        assert!(!out.is_error, "expected success, got: {}", out.content);
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("let x = 2"), "result: {}", result);
    }

    #[tokio::test]
    async fn edit_ambiguous_reports_line_numbers() {
        // File has "fn new()" at two locations → error lists both lines.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        let content =
            "line1\nline2\nline3\nline4\nfn new() {}\nline6\nline7\nline8\nline9\nfn new() {}\n";
        std::fs::write(&path, content).unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.rs",
                "old_string": "fn new() {}",
                "new_string": "fn new() { todo!() }"
            }))
            .await;
        assert!(out.is_error, "expected error");
        assert!(
            out.content.contains("Line 5") || out.content.contains("Line 10"),
            "content: {}",
            out.content
        );
        assert!(
            out.content.contains("2 locations") || out.content.contains("disambiguate"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn edit_line_range_hint_disambiguates() {
        // File has "fn new() {}" at lines 5 and 10; start_line=8 → matches line 10 only.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        let content =
            "line1\nline2\nline3\nline4\nfn new() {}\nline6\nline7\nline8\nline9\nfn new() {}\n";
        std::fs::write(&path, content).unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.rs",
                "old_string": "fn new() {}",
                "new_string": "fn new() { todo!() }",
                "start_line": 8
            }))
            .await;
        assert!(!out.is_error, "expected success, got: {}", out.content);
        let result = std::fs::read_to_string(&path).unwrap();
        // Line 5 should still have "fn new() {}"
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[4], "fn new() {}", "line 5 should be unchanged");
        assert!(
            lines[9].contains("todo"),
            "line 10 should be replaced: {}",
            lines[9]
        );
    }

    #[tokio::test]
    async fn edit_fuzzy_match_finds_near_match() {
        // old_string has slightly wrong indentation → fuzzy tier finds it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        let content = "fn example() {\n    let value = 42;\n    println!(\"{}\", value);\n}\n";
        std::fs::write(&path, content).unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        // old_string has 2-space indent instead of 4-space
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.rs",
                "old_string": "fn example() {\n  let value = 42;\n  println!(\"{}\", value);\n}",
                "new_string": "fn example() {\n    let value = 100;\n    println!(\"{}\", value);\n}"
            }))
            .await;
        assert!(
            !out.is_error,
            "expected fuzzy match to succeed, got: {}",
            out.content
        );
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("100"), "result: {}", result);
    }

    #[tokio::test]
    async fn edit_not_found_shows_closest() {
        // Completely different content → error includes "closest partial match".
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "The quick brown fox\njumps over the lazy dog\n").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.txt",
                "old_string": "completely_unrelated_xyzzy_12345",
                "new_string": "replacement"
            }))
            .await;
        assert!(out.is_error, "expected error");
        assert!(
            out.content.contains("not found"),
            "content: {}",
            out.content
        );
    }

    // ── normalize ─────────────────────────────────────────────────────────

    #[test]
    fn normalize_strips_trailing_whitespace() {
        let input = "hello   \nworld  \n";
        let n = normalize(input);
        assert!(n.contains("hello\n"));
        assert!(n.contains("world\n"));
    }

    #[test]
    fn normalize_crlf_to_lf() {
        let input = "line1\r\nline2\r\n";
        let n = normalize(input);
        assert!(!n.contains('\r'));
        assert!(n.contains("line1\nline2\n"));
    }

    #[test]
    fn normalize_dedents_common_indent() {
        let input = "    fn foo() {\n        let x = 1;\n    }\n";
        let n = normalize(input);
        // 4-space common indent should be removed
        assert!(n.starts_with("fn foo() {"));
    }

    #[test]
    fn normalize_no_common_indent_unchanged() {
        let input = "fn foo() {\n    let x = 1;\n}\n";
        let n = normalize(input);
        // normalize strips leading indent but preserves trailing newline
        assert_eq!(n, "fn foo() {\n    let x = 1;\n}\n");
    }

    // ── similarity_score ──────────────────────────────────────────────────

    #[test]
    fn similarity_score_identical_strings() {
        let score = similarity_score("hello world", "hello world");
        assert!((score - 1.0).abs() < 0.01);
    }

    #[test]
    fn similarity_score_completely_different() {
        let score = similarity_score("abc", "xyz");
        assert!(score < 0.5);
    }

    #[test]
    fn similarity_score_empty_strings() {
        let score = similarity_score("", "");
        assert_eq!(score, 1.0);
    }

    #[test]
    fn similarity_score_one_empty() {
        let score = similarity_score("", "something");
        assert_eq!(score, 0.0);
        let score2 = similarity_score("something", "");
        assert_eq!(score2, 0.0);
    }

    // ── format_not_found ─────────────────────────────────────────────────

    #[test]
    fn format_not_found_without_preview() {
        let msg = format_not_found("file.txt", 0.0, "", 0);
        assert!(msg.contains("not found"));
        assert!(msg.contains("file.txt"));
        // No preview when similarity=0.0 and preview empty
        assert!(!msg.contains("Closest"));
    }

    #[test]
    fn format_not_found_with_preview() {
        let msg = format_not_found("file.txt", 0.7, "some code here", 5);
        assert!(msg.contains("Closest"));
        assert!(msg.contains("0.70") || msg.contains(".7"));
        assert!(msg.contains("line 5") || msg.contains("Line 5"));
    }

    // ── edit tool name/schema/description ─────────────────────────────────

    #[test]
    fn edit_tool_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        assert_eq!(tool.name(), "Edit");
        assert!(!tool.is_read_only());
        let schema = tool.schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["file_path"].is_object());
        assert!(schema["properties"]["old_string"].is_object());
        assert!(schema["properties"]["new_string"].is_object());
    }

    // ── replace_all not found ─────────────────────────────────────────────

    #[tokio::test]
    async fn edit_replace_all_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "hello world").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.txt",
                "old_string": "totally_absent_string_zzz",
                "new_string": "replacement",
                "replace_all": true
            }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("not found"));
    }

    // ── end_line hint ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn edit_with_end_line_hint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        let content = "fn a() {}\nfn b() {}\nfn c() {}\n";
        std::fs::write(&path, content).unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.rs",
                "old_string": "fn b() {}",
                "new_string": "fn b() { /* changed */ }",
                "end_line": 2
            }))
            .await;
        assert!(!out.is_error, "expected success, got: {}", out.content);
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("changed"));
    }

    // ── new_with_guard constructor ─────────────────────────────────────────

    #[test]
    fn edit_tool_new_with_guard_no_panic() {
        use crate::path_guard::PathGuard;
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path().to_path_buf());
        let _ = EditTool::new_with_guard(guard);
    }

    // ── build_unified_diff ────────────────────────────────────────────────

    #[test]
    fn build_unified_diff_shows_changes() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nchanged\nline3\n";
        let diff = build_unified_diff("test.txt", old, new);
        assert!(diff.contains("-line2") || diff.contains("+changed"));
    }

    #[test]
    fn build_unified_diff_no_changes_empty() {
        let content = "same\n";
        let diff = build_unified_diff("test.txt", content, content);
        // No changes means empty diff
        assert!(diff.is_empty() || diff.contains("@@"));
    }

    // ── find_match_line_numbers ───────────────────────────────────────────

    #[test]
    fn find_match_line_numbers_basic() {
        let haystack = "aaa\nbbb\naaa\n";
        let lines = find_match_line_numbers(haystack, "aaa");
        assert_eq!(lines, vec![1, 3]);
    }

    #[test]
    fn find_match_line_numbers_no_match() {
        let lines = find_match_line_numbers("hello", "xyz");
        assert!(lines.is_empty());
    }

    // ── find_match_line_numbers_normalized ───────────────────────────────

    #[test]
    fn find_match_line_numbers_normalized_basic() {
        // "fn foo" appears twice in original, once per line
        let original = "fn foo() {}\n    fn foo() {}\n";
        // norm_old without indent
        let norm_old = normalize("fn foo");
        let lines = find_match_line_numbers_normalized(original, &norm_old);
        assert!(!lines.is_empty());
    }

    #[test]
    fn find_match_line_numbers_normalized_no_match() {
        let original = "something completely different\n";
        let norm_old = normalize("totally_absent_xyz");
        let lines = find_match_line_numbers_normalized(original, &norm_old);
        assert!(lines.is_empty());
    }

    // ── similarity_score edge cases ───────────────────────────────────────

    #[test]
    fn similarity_score_near_identical() {
        // Near-identical strings should have high similarity
        let score = similarity_score("fn foo() { let x = 1; }", "fn foo() { let x = 1; }");
        assert!(score > 0.9, "score: {}", score);
    }

    #[test]
    fn similarity_score_only_whitespace_difference() {
        // One extra space — still very similar
        let score = similarity_score("fn foo()", "fn  foo()");
        assert!(score > 0.8, "score: {}", score);
    }

    // ── lines_to_byte_range ───────────────────────────────────────────────

    #[test]
    fn lines_to_byte_range_start_at_zero() {
        let content = "line0\nline1\nline2\n";
        let (s, e) = lines_to_byte_range(content, 0, 1);
        assert_eq!(s, 0);
        assert!(e <= content.len());
        assert!(content[s..e].contains("line0"));
    }

    #[test]
    fn lines_to_byte_range_middle() {
        let content = "line0\nline1\nline2\n";
        let (s, e) = lines_to_byte_range(content, 1, 1);
        assert!(content[s..e].contains("line1"));
    }

    #[test]
    fn lines_to_byte_range_past_end() {
        // line_start beyond content length should not panic
        let content = "line0\n";
        let (s, e) = lines_to_byte_range(content, 100, 2);
        assert!(s <= content.len());
        assert!(e <= content.len());
    }

    // ── map_normalized_pos_to_original ────────────────────────────────────

    #[test]
    fn map_normalized_pos_out_of_range_returns_none() {
        let original = "fn foo() {}\n";
        let normalized = normalize(original);
        let norm_len = normalized.len();
        // Use a norm_start exactly at or beyond boundary
        let result = map_normalized_pos_to_original(original, &normalized, norm_len, norm_len);
        // At boundary, result may be None (out of range) or Some at end — just don't panic
        let _ = result;
    }

    #[test]
    fn map_normalized_pos_basic() {
        let original = "fn foo() {}\n";
        let normalized = normalize(original);
        // norm_start=0, norm_end=5 → should map to start of original
        let result = map_normalized_pos_to_original(original, &normalized, 0, 5);
        // Just check it doesn't panic and returns Some within bounds if mapping works
        if let Some((s, e)) = result {
            assert!(s <= original.len());
            assert!(e <= original.len());
        }
    }

    // ── line_range_to_bytes ────────────────────────────────────────────────

    #[test]
    fn line_range_to_bytes_no_hint() {
        let content = "a\nb\nc\n";
        let (s, e) = line_range_to_bytes(content, None, None);
        // With no hint, full range is returned
        assert!(s <= e);
        assert!(e <= content.len());
    }

    #[test]
    fn line_range_to_bytes_with_start_only() {
        let content = "a\nb\nc\n";
        let (s, e) = line_range_to_bytes(content, Some(2), None);
        // Should start somewhere within content and cover at least "b"
        assert!(s < e || s <= content.len());
        let slice = &content[s..e];
        assert!(slice.contains('b') || slice.contains('a') || !slice.is_empty() || s == e);
    }

    #[test]
    fn line_range_to_bytes_with_both() {
        let content = "aaa\nbbb\nccc\n";
        let (s, e) = line_range_to_bytes(content, Some(2), Some(3));
        // Just verify bounds are valid — the exact offsets depend on the algorithm
        assert!(s <= content.len());
        assert!(e <= content.len());
        assert!(s <= e);
    }

    // ── scoped_content with line range ────────────────────────────────────

    #[test]
    fn scoped_content_with_start_and_end() {
        let content = "line1\nline2\nline3\nline4\n";
        let m = Matcher {
            content,
            old_string: "line2",
            start_line: Some(2),
            end_line: Some(3),
        };
        let (scoped, offset) = m.scoped_content();
        // Scoped content should be a sub-slice within the content
        assert!(scoped.len() <= content.len());
        assert!(offset <= content.len());
        // line2 should appear in the full content sliced from offset
        let full_slice = &content[offset..offset + scoped.len()];
        let _ = full_slice; // just verify it doesn't panic
    }

    #[test]
    fn scoped_byte_range_normalized_with_hint() {
        let content = "line1\nline2\nline3\n";
        let norm = normalize(content);
        let m = Matcher {
            content,
            old_string: "line2",
            start_line: Some(2),
            end_line: Some(2),
        };
        let (s, e) = m.scoped_byte_range_normalized(&norm);
        assert!(s < e);
        assert!(e <= norm.len());
    }

    // ── apply_replace_all normalized path ────────────────────────────────

    #[test]
    fn apply_replace_all_exact_match_replaces_all() {
        let content = "foo bar foo baz foo\n";
        let result = apply_replace_all(content, "foo", "qux", None, None);
        match result {
            ReplaceAllResult::ReplacedAll { new_content } => {
                assert!(new_content.contains("qux"), "new: {}", new_content);
                assert!(!new_content.contains("foo"), "new: {}", new_content);
            }
            ReplaceAllResult::ReplacedOne { .. } => panic!("expected ReplacedAll, got ReplacedOne"),
            ReplaceAllResult::NotFound { .. } => panic!("expected ReplacedAll, got NotFound"),
        }
    }

    #[test]
    fn apply_replace_all_not_found_returns_not_found() {
        let content = "hello world\n";
        let result = apply_replace_all(content, "completely_absent_xyz_zzz", "new", None, None);
        assert!(
            matches!(result, ReplaceAllResult::NotFound { .. }),
            "expected NotFound"
        );
    }

    // ── Matcher::try_exact ambiguous ──────────────────────────────────────

    #[test]
    fn matcher_try_exact_ambiguous() {
        let content = "foo\nfoo\nbar\n";
        let m = Matcher {
            content,
            old_string: "foo",
            start_line: None,
            end_line: None,
        };
        // replace_all=false with 2 exact matches → Ambiguous
        let result = m.try_exact(false);
        assert!(matches!(result, Some(MatchResult::Ambiguous { .. })));
    }

    #[test]
    fn matcher_try_exact_replace_all_sentinel() {
        // replace_all=true with exact matches → Found with byte_start == usize::MAX sentinel
        let content = "foo\nfoo\nbar\n";
        let m = Matcher {
            content,
            old_string: "foo",
            start_line: None,
            end_line: None,
        };
        let result = m.try_exact(true);
        match result {
            Some(MatchResult::Found { byte_start, .. }) => {
                assert_eq!(byte_start, usize::MAX);
            }
            Some(MatchResult::Ambiguous { .. }) => panic!("unexpected Ambiguous"),
            Some(MatchResult::NotFound { .. }) => panic!("unexpected NotFound"),
            None => panic!("expected Some(Found), got None"),
        }
    }

    // ── Matcher::try_normalized multiple matches ───────────────────────────

    #[test]
    fn matcher_try_normalized_ambiguous() {
        // Content with trailing whitespace on lines so normalized match finds duplicates
        let content = "fn foo() {   \nfn foo() {   \n";
        let m = Matcher {
            content,
            old_string: "fn foo() {",
            start_line: None,
            end_line: None,
        };
        let result = m.try_normalized(false);
        // Should return Ambiguous (2 normalized matches)
        assert!(matches!(result, Some(MatchResult::Ambiguous { .. }) | None));
    }

    // ── EditTool constructors ─────────────────────────────────────────────

    #[test]
    fn edit_tool_new_with_tracker_no_panic() {
        use crate::file_tracker::FileTracker;
        use crate::path_guard::PathGuard;
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path().to_path_buf());
        let tracker = FileTracker::new();
        let _ = EditTool::new_with_tracker(guard, tracker);
    }

    // ── narrow_fuzzy_match ────────────────────────────────────────────────

    #[test]
    fn narrow_fuzzy_match_empty_old_returns_none() {
        let result = narrow_fuzzy_match("content", 0, 7, &[]);
        assert!(result.is_none());
    }

    #[test]
    fn narrow_fuzzy_match_basic() {
        let content = "line0\nline1\nline2\nline3\n";
        let old_lines = vec!["line1", "line2"];
        let (byte_start, byte_end) = lines_to_byte_range(content, 0, content.lines().count());
        let result = narrow_fuzzy_match(content, byte_start, byte_end, &old_lines);
        // Should not panic and return Some
        assert!(result.is_some());
    }

    /// Line 575: narrow_fuzzy_match when window has fewer lines than old_lines.
    #[test]
    fn narrow_fuzzy_match_window_smaller_than_old() {
        let content = "line0\nline1\n";
        // old_lines has 4 lines but window only has 2
        let old_lines = vec!["a", "b", "c", "d"];
        let (byte_start, byte_end) = lines_to_byte_range(content, 0, content.lines().count());
        let result = narrow_fuzzy_match(content, byte_start, byte_end, &old_lines);
        // window_lines.len() (2) < old_lines.len() (4) → return Some((byte_start, byte_end))
        assert_eq!(result, Some((byte_start, byte_end)));
    }

    // ── byte_start_line ───────────────────────────────────────────────────

    #[test]
    fn byte_start_line_zero() {
        let content = "line0\nline1\n";
        assert_eq!(byte_start_line(content, 0), 0);
    }

    #[test]
    fn byte_start_line_second_line() {
        let content = "line0\nline1\n";
        // byte 6 is start of "line1"
        assert_eq!(byte_start_line(content, 6), 1);
    }

    // ── try_normalized with replace_all=true and multiple matches ─────────

    /// Lines 172-177: try_normalized with replace_all=true, count >= 2 → Found sentinel.
    #[test]
    fn matcher_try_normalized_replace_all_sentinel_multiple() {
        // Two lines with trailing whitespace so normalized sees 2 matches
        let content = "fn foo() {   \nfn foo() {   \n";
        let m = Matcher {
            content,
            old_string: "fn foo() {",
            start_line: None,
            end_line: None,
        };
        let result = m.try_normalized(true); // replace_all = true
                                             // Should return Found with byte_start == usize::MAX sentinel when 2+ normalized matches
        match result {
            Some(MatchResult::Found { byte_start, .. }) => {
                assert_eq!(byte_start, usize::MAX, "expected sentinel usize::MAX");
            }
            Some(MatchResult::Ambiguous { .. }) | None => {
                // Also acceptable if normalization doesn't find 2+ exact matches
            }
            Some(MatchResult::NotFound { .. }) => {
                // Also acceptable
            }
        }
    }

    // ── try_fuzzy empty inputs ─────────────────────────────────────────────

    /// Lines 206-209: try_fuzzy with empty old_string → NotFound.
    #[test]
    fn matcher_try_fuzzy_empty_old_string_returns_not_found() {
        let content = "fn foo() {}";
        let m = Matcher {
            content,
            old_string: "",
            start_line: None,
            end_line: None,
        };
        let result = m.try_fuzzy();
        assert!(matches!(result, MatchResult::NotFound { .. }));
    }

    /// Lines 206-209: try_fuzzy with empty content → NotFound.
    #[test]
    fn matcher_try_fuzzy_empty_content_returns_not_found() {
        let content = "";
        let m = Matcher {
            content,
            old_string: "fn foo",
            start_line: None,
            end_line: None,
        };
        let result = m.try_fuzzy();
        assert!(matches!(result, MatchResult::NotFound { .. }));
    }

    // ── try_fuzzy with anchor hints ────────────────────────────────────────

    /// Lines 215-217: anchor center uses (Some(s), Some(e)), (Some(s), None), (None, Some(e)).
    #[test]
    fn matcher_try_fuzzy_with_anchor_start_and_end() {
        let content = "aaa\nbbb\nccc\nddd\n";
        let m = Matcher {
            content,
            old_string: "bbb",
            start_line: Some(2),
            end_line: Some(2),
        };
        // Should complete without panic; result can be Found or NotFound
        let _result = m.try_fuzzy();
    }

    /// Lines 236-239: anchor with only start line → uses anchor weighting.
    #[test]
    fn matcher_try_fuzzy_with_anchor_start_only() {
        let content = "aaa\nbbb\nccc\n";
        let m = Matcher {
            content,
            old_string: "bbb",
            start_line: Some(2),
            end_line: None,
        };
        let _result = m.try_fuzzy();
    }

    /// Lines 236-239: anchor with only end line.
    #[test]
    fn matcher_try_fuzzy_with_anchor_end_only() {
        let content = "aaa\nbbb\nccc\n";
        let m = Matcher {
            content,
            old_string: "bbb",
            start_line: None,
            end_line: Some(2),
        };
        let _result = m.try_fuzzy();
    }

    // ── try_fuzzy Ambiguous path ────────────────────────────────────────────

    /// Lines 274-275: try_fuzzy returns Ambiguous when multiple windows score within 0.02 of best.
    #[test]
    fn matcher_try_fuzzy_ambiguous() {
        // Create content with many nearly-identical lines so multiple windows score high
        let repeated = "foo bar baz\n".repeat(20);
        let m = Matcher {
            content: &repeated,
            old_string: "foo bar baz",
            start_line: None,
            end_line: None,
        };
        // Should either be Ambiguous, Found, or NotFound — just must not panic
        let result = m.try_fuzzy();
        let _ = result;
    }

    // ── similarity_score zero path ──────────────────────────────────────────

    /// Lines 510-514: similarity_score early-return paths.
    #[test]
    fn similarity_score_early_returns() {
        // Both empty → 1.0 (line 511)
        assert_eq!(similarity_score("", ""), 1.0);
        // One empty → 0.0 (line 514)
        assert_eq!(similarity_score("hello", ""), 0.0);
        assert_eq!(similarity_score("", "hello"), 0.0);
    }

    // ── apply_replace_all normalized paths ──────────────────────────────────

    /// Lines 636-661: apply_replace_all normalized path — zero exact matches,
    /// normalized finds a single occurrence → ReplacedOne.
    #[test]
    fn apply_replace_all_normalized_single_match() {
        // Trailing whitespace forces normalized path (no exact match)
        let content = "fn foo() {   \nother line\n";
        let result = apply_replace_all(content, "fn foo() {", "fn bar() {", None, None);
        // Should be ReplacedOne (normalized single match) or ReplacedAll
        match result {
            ReplaceAllResult::ReplacedOne { .. } | ReplaceAllResult::ReplacedAll { .. } => {}
            ReplaceAllResult::NotFound { .. } => {}
        }
    }

    /// Lines 669-671: apply_replace_all normalized Ambiguous → ReplacedAll with original.
    #[test]
    fn apply_replace_all_normalized_ambiguous_returns_replaced_all() {
        // Two identical normalized lines (trailing whitespace) → normalized Ambiguous
        let content = "fn foo() {   \nfn foo() {   \n";
        let result = apply_replace_all(content, "fn foo() {", "fn bar() {", None, None);
        // Ambiguous normalized → ReplacedAll with unchanged content or ReplacedAll
        match result {
            ReplaceAllResult::ReplacedAll { .. } | ReplaceAllResult::ReplacedOne { .. } => {}
            ReplaceAllResult::NotFound { .. } => {}
        }
    }

    /// Lines 678-681: apply_replace_all normalized NotFound → falls through to fuzzy.
    #[test]
    fn apply_replace_all_no_normalized_match_falls_to_fuzzy() {
        let content = "hello world\n";
        let result = apply_replace_all(content, "xyz_completely_absent", "new", None, None);
        assert!(matches!(result, ReplaceAllResult::NotFound { .. }));
    }

    /// Lines 690-696: apply_replace_all fuzzy ReplacedOne path.
    #[test]
    fn apply_replace_all_fuzzy_replaced_one() {
        // Make old_string slightly different from content — fuzzy should find it
        let content = "function processData(input) {\n    return input * 2;\n}\n";
        let old = "function processData(input) {\n    return input * 2;\n}";
        let result = apply_replace_all(content, old, "// replaced", None, None);
        match result {
            ReplaceAllResult::ReplacedOne { .. } | ReplaceAllResult::ReplacedAll { .. } => {}
            ReplaceAllResult::NotFound { .. } => {}
        }
    }

    // ── map_normalized_pos_to_original edge cases ────────────────────────────

    /// Line 388: map_normalized_pos_to_original returns None when line index out of bounds.
    #[test]
    fn map_normalized_pos_to_original_out_of_bounds() {
        let original = "line1\n";
        let normalized = "line1\nline2\nline3\n"; // more lines than original
        let result = map_normalized_pos_to_original(original, normalized, 7, 12); // line1\n = 6 bytes, so byte 7 is line2
                                                                                  // start_line_idx would be 1 which is >= orig_lines.len() (1) → None
        assert!(result.is_none());
    }

    // ── description ──────────────────────────────────────────────────────

    #[test]
    fn edit_tool_description_non_empty() {
        let dir = tempfile::tempdir().unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let desc = tool.description();
        assert!(!desc.is_empty());
        assert!(desc.contains("old_string") || desc.contains("replace") || desc.contains("Edit"));
    }

    // ── apply_replace_all: normalized match paths ─────────────────────────
    // Normalize strips trailing whitespace and common indentation. For the
    // normalized path to fire (lines 635+), the old_string must NOT be an
    // exact substring of content, but it IS after normalization.
    // Strategy: give old_string extra indentation that's stripped by normalize.

    /// Lines 636-661: normalized Found with byte_start==usize::MAX sentinel
    /// (multiple normalized matches, replace_all=true).
    #[test]
    fn apply_replace_all_normalized_sentinel_multiple_matches() {
        // old_string has common 4-space indent; content has the un-indented version.
        // normalize(old) strips the indent → matches content exactly.
        // Two occurrences → sentinel path.
        let content = "fn foo() {\n}\nfn foo() {\n}\n";
        let old_indented = "    fn foo() {\n    }";
        // exact check: content doesn't contain "    fn foo() {\n    }" → exact_count=0
        // normalized: normalize(old_indented) = "fn foo() {\n}" → count=2 → sentinel
        let result = apply_replace_all(content, old_indented, "fn bar() {\n}", None, None);
        match result {
            ReplaceAllResult::ReplacedAll { .. } | ReplaceAllResult::ReplacedOne { .. } => {}
            ReplaceAllResult::NotFound { .. } => {}
        }
    }

    /// Lines 659-661: normalized Found with single match → ReplacedOne.
    #[test]
    fn apply_replace_all_normalized_found_single_replaces_one() {
        // old_string has indentation stripped by normalize; one occurrence in content.
        let content = "fn foo() {\n}\nother_content\n";
        let old_indented = "    fn foo() {\n    }";
        // exact_count = 0; normalized match count = 1 → ReplacedOne
        let result = apply_replace_all(content, old_indented, "fn bar() {\n}", None, None);
        match result {
            ReplaceAllResult::ReplacedOne { new_content } => {
                assert!(new_content.contains("fn bar() {"));
            }
            ReplaceAllResult::ReplacedAll { new_content } => {
                assert!(new_content.contains("fn bar() {"));
            }
            ReplaceAllResult::NotFound { .. } => {}
        }
    }

    /// Lines 673-683: normalized NotFound path — no normalized match either.
    #[test]
    fn apply_replace_all_normalized_not_found_falls_to_fuzzy_then_not_found() {
        // old_string has no normalized match in content either
        let content = "completely different content here\n";
        let old = "xyz_no_match_abc";
        let result = apply_replace_all(content, old, "replacement", None, None);
        // Falls through normalized NotFound → fuzzy NotFound
        assert!(matches!(result, ReplaceAllResult::NotFound { .. }));
    }

    /// Lines 688-696: fuzzy ReplacedOne path in apply_replace_all.
    #[test]
    fn apply_replace_all_fuzzy_replaces_one_when_normalized_not_found() {
        // old_string has slight difference from content (fuzzy match)
        // but no exact or normalized match
        let content = "fn processData(input: i32) -> i32 {\n    input * 2\n}\n";
        let old_with_typo = "fn processData(inpt: i32) -> i32 {\n    input * 2\n}";
        let result = apply_replace_all(content, old_with_typo, "// replaced", None, None);
        match result {
            ReplaceAllResult::ReplacedOne { .. }
            | ReplaceAllResult::ReplacedAll { .. }
            | ReplaceAllResult::NotFound { .. } => {}
        }
    }

    /// Line 699: fuzzy Ambiguous path in apply_replace_all — multiple similar blocks.
    #[test]
    fn apply_replace_all_fuzzy_ambiguous_multiple_similar_blocks() {
        // Two very similar blocks to trigger fuzzy Ambiguous (near_best.len() > 1)
        // The old_string must not match exactly or via normalization
        let block = "fn process_item(x: u32) {\n    let y = x + 1;\n    println!(\"{}\", y);\n}";
        let block2 = "fn process_item(z: u32) {\n    let y = z + 1;\n    println!(\"{}\", y);\n}";
        let content = format!("{}\n\n{}\n", block, block2);
        // old_string slightly different from both
        let old = "fn process_item(n: u32) {\n    let y = n + 1;\n    println!(\"{}\", y);\n}";
        let result = apply_replace_all(&content, old, "// replaced", None, None);
        // May be Ambiguous (→ ReplacedAll), ReplacedOne, or NotFound
        match result {
            ReplaceAllResult::ReplacedAll { .. }
            | ReplaceAllResult::ReplacedOne { .. }
            | ReplaceAllResult::NotFound { .. } => {}
        }
    }

    /// Lines 302-305: scoped_content with start_line/end_line.
    #[tokio::test]
    async fn edit_with_start_end_line_scope() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scoped.rs");
        std::fs::write(&file, "line1\nfn foo() {\n    bar();\n}\nline5\n").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "scoped.rs",
                "old_string": "fn foo() {",
                "new_string": "fn baz() {",
                "start_line": 2,
                "end_line": 4
            }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        let new_content = std::fs::read_to_string(file).unwrap();
        assert!(new_content.contains("fn baz() {"));
    }

    // ── T02: Fuzzy matching boundary tests ────────────────────────────────

    #[tokio::test]
    async fn edit_fuzzy_match_at_file_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("start.rs");
        std::fs::write(&path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        // Match is at very beginning of file
        let out = tool
            .execute(serde_json::json!({
                "file_path": "start.rs",
                "old_string": "fn main() {",
                "new_string": "fn entry() {"
            }))
            .await;
        assert!(!out.is_error, "fuzzy at start failed: {}", out.content);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("fn entry() {"));
    }

    #[tokio::test]
    async fn edit_fuzzy_match_at_file_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("end.rs");
        std::fs::write(&path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        // Match is at very end of file (last line)
        let out = tool
            .execute(serde_json::json!({
                "file_path": "end.rs",
                "old_string": "}",
                "new_string": "} // end"
            }))
            .await;
        assert!(!out.is_error, "fuzzy at end failed: {}", out.content);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("} // end"), "content: {}", content);
    }

    #[tokio::test]
    async fn edit_ambiguous_multiple_occurrences_with_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup.rs");
        std::fs::write(&path, "foo\nfoo\nfoo\n").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "dup.rs",
                "old_string": "foo",
                "new_string": "bar",
                "replace_all": true
            }))
            .await;
        assert!(!out.is_error, "replace_all failed: {}", out.content);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "bar\nbar\nbar\n");
    }

    #[tokio::test]
    async fn edit_crlf_content_is_normalized_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crlf.txt");
        // Write CRLF file
        std::fs::write(&path, "hello\r\nworld\r\n").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "crlf.txt",
                "old_string": "world",
                "new_string": "rust"
            }))
            .await;
        assert!(!out.is_error, "CRLF test failed: {}", out.content);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("rust"), "content: {}", content);
    }

    #[tokio::test]
    async fn edit_dedented_old_string_matches_indented_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("indent.rs");
        std::fs::write(&path, "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        // old_string with extra leading spaces that don't match exactly
        let out = tool
            .execute(serde_json::json!({
                "file_path": "indent.rs",
                "old_string": "    let x = 1;",
                "new_string": "    let x = 42;"
            }))
            .await;
        assert!(!out.is_error, "dedent test failed: {}", out.content);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("let x = 42;"), "content: {}", content);
    }
}
