//! TUI layout constants — **single source of truth** for all UI sizing thresholds.
//!
//! Change a value here and the entire UI responds. No more hunting through
//! 5+ files to adjust one cap.

#![allow(dead_code)]

// ── Terminal size breakpoints ────────────────────────────────────────────────

/// Below this width: narrow layout (stacked, minimal header).
pub const NARROW_WIDTH: u16 = 60;

/// Below this width: hide status panel, hint line.
pub const STATUS_MIN_WIDTH: u16 = 40;

// ── Status rail (wide layout) ────────────────────────────────────────────────

/// Minimum terminal width to show rail in Auto mode.
pub const STATUS_RAIL_MIN_TERM_WIDTH: u16 = 118;

/// Minimum terminal width to show rail when forced On.
pub const STATUS_RAIL_MIN_TERM_WIDTH_ON: u16 = 108;

/// Minimum width of the rail itself.
pub const STATUS_RAIL_WIDTH_MIN: u16 = 28;

/// Maximum width of the rail itself.
pub const STATUS_RAIL_WIDTH_MAX: u16 = 48;

// ── Plan / Task panel ────────────────────────────────────────────────────────

/// Max steps displayed in the plan panel before "+N more" and scroll.
pub const PLAN_MAX_VISIBLE_STEPS: usize = 8;

/// Minimum terminal width for plan panel to appear.
pub const PLAN_PANEL_MIN_WIDTH: u16 = 52;

/// Minimum terminal height for plan panel in Auto mode.
pub const PLAN_PANEL_MIN_TERM_H_AUTO: u16 = 28;

/// Minimum terminal height for plan panel in On mode.
pub const PLAN_PANEL_MIN_TERM_H_ON: u16 = 18;

/// When harness mode is active, Auto threshold is reduced by this amount
/// (floored at 20 to prevent absurdly low thresholds).
pub const PLAN_PANEL_HARNESS_AUTO_REDUCTION: u16 = 4;
pub const PLAN_PANEL_MIN_TERM_H_FLOOR: u16 = 20;

// ── Tool / Status log ────────────────────────────────────────────────────────

/// Max tool lines shown in the stacked layout status strip.
pub const TOOLS_CAP_STACKED: Option<usize> = Some(2);

/// Max tool lines shown in the status rail.
pub const TOOLS_CAP_RAIL: Option<usize> = Some(5);

// ── Input bar ────────────────────────────────────────────────────────────────

/// Minimum input bar height (title + textarea + padding).
pub const INPUT_MIN_HEIGHT: u16 = 3;

/// Maximum input bar height.
pub const INPUT_MAX_HEIGHT: u16 = 8;

/// Title row count for the input block.
pub const INPUT_TITLE_ROWS: u16 = 1;

// ── Chat area ────────────────────────────────────────────────────────────────

/// Minimum chat area height in stacked layout.
pub const CHAT_MIN_HEIGHT: u16 = 10;

// ── Status strip (stacked layout) ────────────────────────────────────────────

/// Status spinner strip height.
pub const STATUS_STRIP_HEIGHT: u16 = 2;

/// Hint line height.
pub const HINT_HEIGHT: u16 = 1;

// ── Pickers ──────────────────────────────────────────────────────────────────

/// Max visible rows in slash command completion popup.
pub const SLASH_COMPLETION_VISIBLE: usize = 12;

/// Max visible rows in model picker.
pub const MODEL_PICKER_VISIBLE: usize = 14;

/// Max visible rows in session picker.
pub const SESSION_PICKER_VISIBLE: usize = 12;

/// Max visible rows in profile picker.
pub const PROFILE_PICKER_VISIBLE: usize = 12;

/// Max width for the cost column in pickers.
pub const PICKER_COST_WIDTH: usize = 8;

// ── Agent stall thresholds ───────────────────────────────────────────────────

/// Seconds before showing "agent may be stalled" warning.
pub const AGENT_STALL_WARN_SECS: u64 = 300;

/// Seconds before force-stopping a stalled agent.
pub const AGENT_STALL_MAX_SECS: u64 = 900;
