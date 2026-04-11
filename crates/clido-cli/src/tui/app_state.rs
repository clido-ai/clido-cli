use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clido_core::PermissionMode;
use ratatui::style::Color;
use ratatui::text::Line;

use crate::git_context::GitContext;
use crate::image_input::ImageAttachment;
use crate::overlay::OverlayStack;
use crate::repl::expand_at_file_refs;
use crate::text_input::TextInput;

use super::commands::{execute_slash, is_known_slash_cmd, parse_per_turn_model};
use super::copy::info as copy_info;
use super::render::build_plan_from_assistant_text;
use super::state::*;

/// Text selection for in-app copy.
/// Tracks anchor (start) and focus (end) positions in (row, col) format.
#[derive(Debug, Clone, Default)]
pub(super) struct Selection {
    pub anchor: (usize, usize), // (row, col) - start of selection
    pub focus: (usize, usize),  // (row, col) - end of selection
    pub active: bool,           // whether selection is currently active
}

#[allow(dead_code)]
impl Selection {
    /// Clear the selection.
    pub fn clear(&mut self) {
        self.active = false;
        self.anchor = (0, 0);
        self.focus = (0, 0);
    }

    /// Set anchor point and start selection.
    pub fn start(&mut self, row: usize, col: usize) {
        self.anchor = (row, col);
        self.focus = (row, col);
        self.active = true;
    }

    /// Update focus point (during drag).
    pub fn update(&mut self, row: usize, col: usize) {
        self.focus = (row, col);
    }

    /// Get ordered selection bounds (start_row, start_col, end_row, end_col).
    pub fn bounds(&self) -> (usize, usize, usize, usize) {
        let (ar, ac) = self.anchor;
        let (fr, fc) = self.focus;

        if ar < fr || (ar == fr && ac < fc) {
            (ar, ac, fr, fc)
        } else {
            (fr, fc, ar, ac)
        }
    }

    /// Check if a cell is within the selection.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        if !self.active {
            return false;
        }
        let (sr, sc, er, ec) = self.bounds();
        row >= sr && row <= er && (row > sr || col >= sc) && (row < er || col <= ec)
    }
}

// ── Selection column helpers ──────────────────────────────────────────────────

/// Display-cell column (terminal position) → character index in `line`.
/// Each wide character advances the display position by its Unicode width.
fn display_col_to_char_idx(line: &str, target_col: usize) -> usize {
    let mut display_col = 0usize;
    for (i, ch) in line.char_indices() {
        if display_col >= target_col {
            return i;
        }
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        display_col += w;
    }
    line.len()
}

/// Total display width of a line in terminal cells.
fn line_display_width(line: &str) -> usize {
    line.chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// High-level agent activity (complements `busy` for IDE-like status awareness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AppRunState {
    #[default]
    Idle,
    /// Waiting on the model (batch or stream aggregate).
    Generating,
    /// Executing one or more tools.
    RunningTools,
}

/// Per-step display status for the active workflow panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkflowStepStatus {
    Pending,
    Active,
    Done,
    Failed,
    /// Step ran but hit on_error:continue — completed with error, workflow proceeds.
    Skipped,
}

/// One step entry in the active workflow panel.
#[derive(Debug, Clone)]
pub(super) struct WorkflowStepEntry {
    pub(super) step_id: String,
    pub(super) name: String,
    pub(super) status: WorkflowStepStatus,
}

/// State for a workflow being orchestrated through the main agent session.
pub(super) struct ActiveWorkflow {
    // ── Display (right rail) ──────────────────────────────────────────────────
    pub(super) name: String,
    pub(super) steps: Vec<WorkflowStepEntry>,
    // ── Orchestration ─────────────────────────────────────────────────────────
    pub(super) def: clido_workflows::WorkflowDef,
    pub(super) context: clido_workflows::WorkflowContext,
    /// Index of the next step to execute (0-based).
    pub(super) current_idx: usize,
    pub(super) run_id: String,
    /// Cumulative session cost at workflow start — used to compute total workflow cost.
    /// Uses `session_total_cost_usd` (includes parallel step costs).
    pub(super) start_cost: f64,
    pub(super) start_time: std::time::Instant,
    /// Timestamp when the current sequential step was sent.
    pub(super) step_start_time: Option<std::time::Instant>,
    /// Model to restore once the current step's profile override expires.
    pub(super) step_prev_model: Option<String>,
    /// Abort handle for an in-progress parallel batch task.
    pub(super) parallel_abort: Option<tokio::task::AbortHandle>,
    /// Number of times the current step has been retried (reset to 0 on success/skip/advance).
    pub(super) retry_attempts: usize,
    /// Profile override from `/workflow run --profile=<name>` — applied to steps
    /// that don't have their own `profile:` field.
    pub(super) profile_override: Option<String>,
}

pub(super) struct App {
    pub(super) messages: Vec<ChatLine>,
    /// Live activity log shown in the status strip (last 2 entries).
    pub(super) status_log: std::collections::VecDeque<StatusEntry>,
    pub(super) text_input: TextInput,
    /// Current scroll offset (logical lines from top). Updated by handle_key; clamped in render.
    pub(super) scroll: u32,
    pub(super) following: bool,
    /// If set after a terminal resize, restore scroll to this ratio of max_scroll on next render.
    pub(super) pending_scroll_ratio: Option<f64>,
    /// Tool call counts for the current assistant turn (shown when the turn ends).
    pub(super) turn_tool_tally: TurnToolTally,
    /// `max_scroll` from the previous chat render — used to detect large jumps while following.
    pub(super) chat_render_prev_max_scroll: u32,
    /// After a large auto-follow jump, drop one spurious scroll-up (wheel echo).
    pub(super) suppress_next_chat_scroll_up: bool,

    /// Layout metrics computed during render; consumed by input/event handlers.
    pub(super) layout: super::state::LayoutInfo,
    /// Vertical scroll within the status rail (wide-terminal layout).
    pub(super) status_panel_scroll: u16,
    /// Cached `git` snapshot for the status rail (refreshed on workdir change and at startup).
    pub(super) tui_git_snapshot: Option<GitContext>,
    pub(super) agent_run_state: AppRunState,
    pub(super) busy: bool,
    pub(super) spinner_tick: usize,
    pub(super) pending_perm: Option<PendingPerm>,
    /// Unified overlay stack (errors, read-only, choices, etc.)
    pub(super) overlay_stack: OverlayStack,
    pub(super) channels: AgentChannels,
    /// Inputs queued while agent was busy — drained FIFO when agent finishes.
    pub(super) queued: VecDeque<String>,
    /// Index for cycling through queued items with Up arrow (None = not in queue nav mode).
    pub(super) queue_nav_idx: Option<usize>,
    /// When editing a queued item, this stores the original text to restore if cancelled.
    pub(super) editing_queued_item: Option<String>,
    /// Session picker popup state (Some = popup visible).
    pub(super) session_picker: Option<SessionPickerState>,
    /// Model picker popup state (Some = popup visible).
    pub(super) model_picker: Option<ModelPickerState>,
    /// Profile picker popup state (Some = popup visible).
    pub(super) profile_picker: Option<ProfilePickerState>,
    /// In-TUI profile overview/editor overlay (Some = visible).
    pub(super) profile_overlay: Option<ProfileOverlayState>,
    /// All known models (built at startup from pricing table + profiles).
    pub(super) known_models: Vec<ModelEntry>,
    /// User model preferences: favorites, recency, role assignments.
    pub(super) model_prefs: clido_core::ModelPrefs,
    /// Signal to cancel the current agent run (force send).
    pub(super) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Selected option in the permission popup (0=once, 1=session, 2=workdir, 3=deny, 4=deny+feedback).
    pub(super) perm_selected: usize,
    /// When user picks "Deny with feedback", this holds the feedback text being typed.
    pub(super) perm_feedback_input: Option<String>,
    /// Tracks whether the user has granted AllowAll for this session (for UI display).
    pub(super) permission_mode_override: Option<PermissionMode>,
    /// Selected index in the slash-command popup (None = no popup).
    pub(super) selected_cmd: Option<usize>,
    pub(super) quit: bool,
    /// When true, the TUI exits and setup wizard re-runs to reconfigure.
    pub(super) wants_reinit: bool,
    /// When Some(name), the TUI exits and the active profile is switched then TUI restarts.
    /// When true, the TUI exits and the profile-creation wizard runs, then TUI restarts.
    pub(super) wants_profile_create: bool,
    /// When Some(name), the TUI exits and the profile-edit wizard runs, then TUI restarts.
    pub(super) wants_profile_edit: Option<String>,
    /// When Some(id), restart TUI and resume this session immediately.
    /// Text selection state for in-app copy/paste.
    pub(super) selection: Selection,
    pub(super) provider: String,
    pub(super) model: String,
    /// Active profile name, shown in the header.
    pub(super) current_profile: String,
    /// Session ID of the current agent session (set after SessionStarted event).
    pub(super) current_session_id: Option<String>,
    /// Generated title for the current session.
    pub(super) session_title: Option<String>,
    /// Project root used for listing sessions.
    pub(super) workspace_root: std::path::PathBuf,
    /// Whether we're in selection mode (vim-style copy mode).
    pub(super) selection_mode: bool,

    /// Last completed agent invocation's token totals (for context % in header).
    pub(super) stats: SessionStats,
    /// Max context window in tokens for the current model (0 = unknown).
    pub(super) context_max_tokens: u64,
    /// Channel to trigger immediate context compaction in agent_task.
    pub(super) plan: PlanState,
    /// When true, fire desktop notification + terminal bell after each agent turn
    /// (subject to the MIN_ELAPSED_SECS gate in `notify.rs`).
    pub(super) notify_enabled: bool,
    /// Shared flag that gates SpawnReviewerTool execution.  Toggle with `/reviewer on|off`.
    pub(super) reviewer_enabled: Arc<AtomicBool>,
    /// Set to true during crash recovery so the ResumedSession event preserves
    /// the current TUI messages instead of clearing and replaying them.
    pub(super) recovering: bool,
    /// Reviewer is always available (sub-agents always registered).
    pub(super) reviewer_configured: bool,
    /// Timestamp of when the current agent turn was submitted; used to compute elapsed time.
    pub(super) turn_start: Option<std::time::Instant>,
    /// Previous model to revert to after a per-turn `@model` override completes.
    pub(super) per_turn_prev_model: Option<String>,
    /// Image loaded via `/image <path>` — attached to the next user message then cleared.
    pub(super) pending_image: Option<ImageAttachment>,
    /// Shared state: image to attach to the next prompt.  Written by the TUI on send,
    /// drained by agent_task before calling run/run_next_turn.
    pub(super) image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
    /// Pending path permission request waiting for user response (y/n/a).
    pub(super) pending_path_permission: Option<std::path::PathBuf>,

    /// Whether we're in plan dry-run mode (show editor but never execute).
    pub(super) plan_dry_run: bool,

    /// Current plan step being executed, extracted from agent text (e.g. "Step 3: Write contract").
    pub(super) current_step: Option<String>,
    /// The step number most recently seen while the agent was executing a plan.
    /// Used after agent finishes to show which steps remain.
    pub(super) last_executed_step_num: Option<usize>,
    /// `--harness` only (session flag). Combined with each workspace's `[agent] harness` after workdir switch.
    pub(super) harness_from_cli: bool,
    /// Effective harness mode for this workspace: `harness_from_cli` OR loaded `[agent] harness`.
    pub(super) harness_mode: bool,
    /// Shared todo list written by the agent via the TodoWrite tool.
    pub(super) todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
    /// Task list strip: todos, planner snapshot, harness, live step (`/tasks on|off|auto`, alias `/progress`).
    pub(super) plan_panel_visibility: PlanPanelVisibility,
    /// Right-hand status column: session, context, agent, queue, tasks, tools (`/panel on|off|auto`).
    pub(super) status_rail_visibility: StatusRailVisibility,
    /// Track whether we have already shown the empty-input hint this session.
    pub(super) empty_input_hint_shown: bool,
    /// Pending `/enhance` request — set by cmd_enhance, consumed by event_loop.
    pub(super) pending_enhance: Option<String>,
    /// True while an `/enhance` LLM call is in flight.
    pub(super) enhancing: bool,
    /// Utility (fast) provider for background tasks like `/enhance`.
    pub(super) utility_provider: Arc<dyn clido_providers::ModelProvider>,
    /// Model name for the utility provider.
    pub(super) utility_model: String,
    /// Max budget for the session (from config), shown in header.
    pub(super) max_budget_usd: Option<f64>,

    /// Workflow text editor overlay — open when editing a workflow YAML.
    pub(super) workflow_editor: Option<super::state::PlanTextEditor>,
    /// File path of the workflow being edited (None = new/unsaved).
    pub(super) workflow_editor_path: Option<std::path::PathBuf>,
    /// State for a workflow currently running in the background.
    pub(super) active_workflow: Option<ActiveWorkflow>,

    /// Rate-limit auto-resume: when the agent hits a rate limit with a known
    /// retry_after, we set a timer. When it expires the agent is automatically
    /// sent a "continue" message so it can pick up where it left off.
    /// `None` means no auto-resume is pending.
    pub(super) rate_limit_resume_at: Option<std::time::Instant>,
    /// Whether the user has cancelled the auto-resume (Escape while waiting).
    pub(super) rate_limit_cancelled: bool,
    /// Background ping mode: when rate limit has unknown reset time, we ping
    /// every 15 minutes to check if API is available again.
    pub(super) rate_limit_pinging: bool,
    /// Next scheduled ping time for background rate limit recovery.
    pub(super) rate_limit_next_ping: Option<std::time::Instant>,
    /// Count of background pings sent (for user feedback).
    pub(super) rate_limit_ping_count: u32,

    /// Resolved API key for the active profile — used for live model fetching.
    pub(super) api_key: String,
    /// Optional custom base URL for the active profile's provider.
    pub(super) base_url: Option<String>,
    /// Externally allowed paths for this session (outside workspace_root).
    /// Tools can access these paths in addition to the workspace.
    pub(super) allowed_external_paths: Vec<std::path::PathBuf>,

    /// True while a model-list fetch is in progress (shows spinner in model picker).
    pub(super) models_loading: bool,
    /// Render cache: maps (content_hash, render_width) to pre-built Line<'static> slices.
    /// Avoids re-parsing markdown on every 80ms render tick when messages haven't changed.
    /// Invalidated (cleared) on terminal resize since width affects line-wrapping.
    pub(super) render_cache: std::collections::HashMap<(u64, usize), Vec<Line<'static>>>,
    /// Hash of the messages Vec at the time the cache was last populated.
    /// Used to detect when messages change and stale entries should be evicted.
    pub(super) render_cache_msg_count: usize,
    /// Plain-text snapshot of the last rendered lines, used by `get_selected_text()`
    /// so selection coordinates (which are in rendered-line space) can be resolved
    /// without re-running the markdown→Line conversion.
    pub(super) rendered_line_texts: Vec<String>,
    /// Non-blocking toast notifications (auto-dismiss).
    pub(super) toasts: Vec<Toast>,
    /// Last time we showed a "agent seems stuck" warning to avoid spamming.
    pub(super) last_stall_warning: Option<std::time::Instant>,
    /// Set when the agent fails to deliver an `AgentEvent` to the TUI (channel closed).
    pub(super) ui_emit_unhealthy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The most recent user prompt — shown as a banner below the header while the
    /// agent is busy, so the user always knows what task is in flight.
    pub(super) active_prompt: Option<String>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        channels: AgentChannels,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        provider: String,
        model: String,
        workspace_root: std::path::PathBuf,
        notify_enabled: bool,
        image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
        plan_dry_run: bool,
        known_models: Vec<ModelEntry>,
        model_prefs: clido_core::ModelPrefs,
        current_profile: String,
        reviewer_enabled: Arc<AtomicBool>,
        harness_from_cli: bool,
        harness_mode: bool,
        todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
        api_key: String,
        base_url: Option<String>,
        utility_provider: Arc<dyn clido_providers::ModelProvider>,
        utility_model: String,
        ui_emit_unhealthy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let budget = clido_core::load_config(&workspace_root)
            .ok()
            .and_then(|c| c.agent.max_budget_usd);
        let mut app = Self {
            messages: Vec::new(),
            status_log: std::collections::VecDeque::new(),
            text_input: TextInput::new(),
            scroll: 0,
            following: true,
            pending_scroll_ratio: None,
            turn_tool_tally: TurnToolTally::default(),
            chat_render_prev_max_scroll: 0,
            suppress_next_chat_scroll_up: false,
            layout: super::state::LayoutInfo::default(),
            status_panel_scroll: 0,
            tui_git_snapshot: GitContext::discover(&workspace_root),
            agent_run_state: AppRunState::default(),

            busy: false,
            spinner_tick: 0,
            pending_perm: None,
            overlay_stack: OverlayStack::new(),
            channels,
            queued: VecDeque::new(),
            queue_nav_idx: None,
            editing_queued_item: None,
            session_picker: None,
            model_picker: None,
            profile_picker: None,
            profile_overlay: None,
            known_models,
            model_prefs,
            cancel,
            perm_selected: 0,
            perm_feedback_input: None,
            permission_mode_override: None,
            selected_cmd: None,
            quit: false,
            wants_reinit: false,
            wants_profile_create: false,
            wants_profile_edit: None,
            selection: Selection::default(),
            provider,
            model,
            current_profile,
            current_session_id: None,
            session_title: None,
            workspace_root,

            stats: SessionStats::default(),
            context_max_tokens: 0,
            plan: PlanState::default(),
            notify_enabled,
            reviewer_enabled,
            recovering: false,
            reviewer_configured: true,
            turn_start: None,
            per_turn_prev_model: None,
            pending_image: None,
            image_state,
            pending_path_permission: None,
            current_step: None,
            last_executed_step_num: None,
            plan_dry_run,
            harness_from_cli,
            harness_mode,
            todo_store,
            plan_panel_visibility: if harness_mode {
                PlanPanelVisibility::On
            } else {
                PlanPanelVisibility::default()
            },
            status_rail_visibility: StatusRailVisibility::default(),
            empty_input_hint_shown: false,
            pending_enhance: None,
            enhancing: false,
            utility_provider,
            utility_model,
            max_budget_usd: budget,
            workflow_editor: None,
            workflow_editor_path: None,
            active_workflow: None,
            rate_limit_resume_at: None,
            rate_limit_cancelled: false,
            rate_limit_pinging: false,
            rate_limit_next_ping: None,
            rate_limit_ping_count: 0,
            api_key,
            base_url,
            allowed_external_paths: Vec::new(),
            models_loading: false,
            render_cache: std::collections::HashMap::new(),
            render_cache_msg_count: 0,
            rendered_line_texts: Vec::new(),
            toasts: Vec::new(),
            last_stall_warning: None,
            selection_mode: false,
            ui_emit_unhealthy,
            active_prompt: None,
        };
        app.messages.push(ChatLine::WelcomeSplash);
        app
    }

    pub(super) fn push(&mut self, line: ChatLine) {
        const MAX_THINKING_COALESCE: usize = 24_000;
        if let (ChatLine::Thinking(new_t), Some(ChatLine::Thinking(prev))) =
            (&line, self.messages.last_mut())
        {
            if !new_t.is_empty() {
                if !prev.is_empty() {
                    prev.push('\n');
                }
                prev.push_str(new_t);
                if prev.len() > MAX_THINKING_COALESCE {
                    prev.truncate(MAX_THINKING_COALESCE);
                    prev.push_str("\n… [truncated]");
                }
            }
            return;
        }
        self.messages.push(line);
        // scroll position is computed at render time when following=true
    }

    /// Show a non-blocking toast that auto-dismisses after `duration`.
    pub(super) fn push_toast(
        &mut self,
        message: impl Into<String>,
        style: Color,
        duration: std::time::Duration,
    ) {
        self.toasts.push(Toast {
            message: message.into(),
            style,
            expires: std::time::Instant::now() + duration,
            position: None,
        });
    }

    /// Show a toast anchored near screen coordinates (x, y).
    pub(super) fn push_toast_at(
        &mut self,
        message: impl Into<String>,
        style: Color,
        duration: std::time::Duration,
        pos: (u16, u16),
    ) {
        self.toasts.push(Toast {
            message: message.into(),
            style,
            expires: std::time::Instant::now() + duration,
            position: Some(pos),
        });
    }

    /// Remove expired toasts.
    pub(super) fn expire_toasts(&mut self) {
        let now = std::time::Instant::now();
        self.toasts.retain(|t| t.expires > now);
    }

    /// Refresh git branch/status for the status rail after `workspace_root` changes.
    pub(super) fn refresh_git_snapshot(&mut self) {
        self.tui_git_snapshot = GitContext::discover(&self.workspace_root);
    }

    /// Determine which component currently owns input focus.
    /// Mirrors the priority cascade in `handle_key` but as a single query.
    pub(super) fn focus(&self) -> super::state::FocusTarget {
        use super::state::FocusTarget;
        if self.plan.text_editor.is_some() {
            FocusTarget::PlanTextEditor
        } else if self.workflow_editor.is_some() {
            FocusTarget::WorkflowEditor
        } else if self.plan.editor.is_some() {
            FocusTarget::PlanEditor
        } else if self.profile_overlay.is_some() {
            FocusTarget::ProfileOverlay
        } else if !self.overlay_stack.is_empty() {
            FocusTarget::Overlay
        } else if self.model_picker.is_some() {
            FocusTarget::ModelPicker
        } else if self.session_picker.is_some() {
            FocusTarget::SessionPicker
        } else if self.profile_picker.is_some() {
            FocusTarget::ProfilePicker
        } else if self.pending_perm.is_some() {
            FocusTarget::Permission
        } else {
            FocusTarget::ChatInput
        }
    }

    /// Clear scheduled rate-limit auto-resume and background ping mode.
    pub(super) fn disarm_rate_limit_recovery(&mut self) {
        self.rate_limit_resume_at = None;
        self.rate_limit_cancelled = false;
        self.rate_limit_pinging = false;
        self.rate_limit_next_ping = None;
        self.rate_limit_ping_count = 0;
    }

    /// Send immediately (not busy). Moves input → chat + agent.
    /// If input starts with `@model-name prompt`, applies a per-turn model override.
    /// Sends `prompt` to the agent without showing anything in the chat.
    pub(super) fn send_silent(&mut self, prompt: String) {
        let _ = self.channels.prompt_tx.send(AgentUserInput::Prompt(prompt));
        self.turn_tool_tally.clear();
        self.text_input.text.clear();
        self.text_input.cursor = 0;
        self.busy = true;
        self.agent_run_state = AppRunState::Generating;
        self.following = true;
        self.turn_start = Some(std::time::Instant::now());
        self.text_input.history_idx = None;
        self.text_input.history_draft.clear();
    }

    /// Resume the model turn with `run_continue` (no extra user message or session user line).
    /// Shows a short info line so the transcript matches what the agent is doing.
    pub(super) fn send_agent_continue_turn(&mut self) {
        self.push(ChatLine::Info(
            "  ▶ Resuming from saved context (rate-limit recovery — no new user message)".into(),
        ));
        let _ = self.channels.prompt_tx.send(AgentUserInput::ContinueTurn);
        self.turn_tool_tally.clear();
        self.text_input.text.clear();
        self.text_input.cursor = 0;
        self.busy = true;
        self.agent_run_state = AppRunState::Generating;
        self.following = true;
        self.turn_start = Some(std::time::Instant::now());
        self.text_input.history_idx = None;
        self.text_input.history_draft.clear();
    }

    pub(super) fn send_now(&mut self, text: String) {
        self.disarm_rate_limit_recovery();

        // If a pending image was attached via /image, publish it to the shared image_state
        // so agent_task can prepend an Image ContentBlock to this user message.
        if let Some(img) = self.pending_image.take() {
            if let Ok(mut guard) = self.image_state.lock() {
                *guard = Some((img.media_type.to_string(), img.base64_data));
            }
        }
        // Expand @file references in user input
        let text = expand_at_file_refs(&text, std::env::current_dir().ok().as_deref());
        // Remember if text matches current input BEFORE any moves (to decide whether to clear after send)
        let text_matches_input = self.text_input.text.trim() == text.trim();
        // Capture banner text before text is moved into the send call.
        let banner_text = if let Some((_, actual)) = parse_per_turn_model(&text) {
            actual
        } else {
            text.clone()
        };
        // Check for per-turn @model-name prefix.
        let send_result = if let Some((per_turn_model, actual_prompt)) = parse_per_turn_model(&text)
        {
            self.per_turn_prev_model = Some(self.model.clone());
            self.model = per_turn_model.clone();
            let _ = self.channels.model_switch_tx.send(per_turn_model.clone());
            self.push(ChatLine::Info(format!(
                "  ↻ Using {} for this turn only",
                per_turn_model
            )));
            self.push(ChatLine::User(actual_prompt.clone()));
            self.text_input.push_history(&text);
            self.channels
                .prompt_tx
                .send(AgentUserInput::Prompt(actual_prompt))
        } else {
            self.push(ChatLine::User(text.clone()));
            self.text_input.push_history(&text);
            self.channels.prompt_tx.send(AgentUserInput::Prompt(text))
        };

        if send_result.is_err() {
            // Agent task channel closed — can't send; stay idle and surface an error.
            self.push(ChatLine::Info(copy_info::AGENT_NOT_RUNNING.into()));
            return;
        }

        // Show acknowledgment that the agent understood and is working
        self.push(ChatLine::Info(
            "  🤔 Understood — analyzing and working on it…".into(),
        ));

        self.turn_tool_tally.clear();

        // Only clear input field if this text matches what's currently in the input
        // (user just submitted it). Don't clear if we're draining a queued item
        // while user is typing something new.
        if text_matches_input {
            self.text_input.text.clear();
            self.text_input.cursor = 0;
        }
        self.busy = true;
        self.agent_run_state = AppRunState::Generating;
        self.following = true;
        self.turn_start = Some(std::time::Instant::now());
        self.text_input.history_idx = None;
        self.text_input.history_draft.clear();
        self.active_prompt = Some(banner_text);
    }

    /// Execute a slash command or send chat to the agent (single user line).
    pub(super) fn dispatch_user_input(&mut self, text: String) {
        self.text_input.text.clear();
        self.text_input.cursor = 0;
        self.text_input.history_idx = None;
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            // Silently ignore — no feedback needed; user pressed Enter on blank input.
            return;
        }
        if trimmed == "/" {
            self.push(ChatLine::Info(copy_info::BARE_SLASH.into()));
            return;
        }
        if is_known_slash_cmd(&trimmed) {
            execute_slash(self, &trimmed);
        } else {
            self.send_now(trimmed);
        }
    }

    /// After the agent is idle, drain the FIFO queue: slash commands run in order until one
    /// submits a new turn (`busy`) or `/quit` is seen.
    pub(super) fn drain_input_queue(&mut self) {
        while let Some(next) = self.queued.pop_front() {
            let trimmed = next.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed == "/" {
                self.push(ChatLine::Info(copy_info::BARE_SLASH.into()));
                continue;
            }
            if is_known_slash_cmd(&trimmed) {
                execute_slash(self, &trimmed);
                if self.quit {
                    return;
                }
                if self.busy {
                    return;
                }
            } else {
                self.send_now(trimmed);
                return;
            }
        }
    }

    /// Normal Enter: send if idle, queue if busy.
    /// If editing a queued item (via arrow-up navigation), remove it from queue.
    pub(super) fn submit(&mut self) {
        if self.pending_perm.is_some() {
            return;
        }
        let text = self.text_input.text.trim().to_string();
        if text.is_empty() {
            if !self.empty_input_hint_shown && !self.busy {
                self.empty_input_hint_shown = true;
                self.push(ChatLine::Info(copy_info::EMPTY_HINT.into()));
            }
            return;
        }
        if text == "/" {
            self.push(ChatLine::Info(copy_info::BARE_SLASH.into()));
            self.text_input.text.clear();
            self.text_input.cursor = 0;
            return;
        }

        // Check if we're editing a queued item - remove it from queue
        if let Some(idx) = self.queue_nav_idx.take() {
            if idx < self.queued.len() {
                // If the text changed, we need to find and remove the original
                // If text is same as original, remove it; if different, user edited it
                self.queued.remove(idx);
            }
        }

        // /stop and /note commands bypass queue and execute immediately
        if text == "/stop" {
            if self.busy {
                self.stop_only();
            } else {
                self.push(ChatLine::Info(copy_info::NO_ACTIVE_STOP.into()));
            }
            self.text_input.text.clear();
            self.text_input.cursor = 0;
            return;
        }
        if text.starts_with("/note") {
            self.dispatch_user_input(text);
            self.text_input.text.clear();
            self.text_input.cursor = 0;
            return;
        }

        if self.busy {
            // Enqueue for after the current run finishes (FIFO).
            self.queued.push_back(text);
            self.text_input.text.clear();
            self.text_input.cursor = 0;
        } else {
            self.dispatch_user_input(text);
        }
    }

    /// Ctrl+Enter: cancel current run and send input immediately.
    pub(super) fn force_send(&mut self) {
        if self.pending_perm.is_some() {
            return;
        }
        let text = self.text_input.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.busy {
            // Cancel the running agent turn, then queue this as next prompt.
            self.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            // Prioritize this prompt ahead of already queued inputs.
            self.queued.push_front(text);
            self.text_input.text.clear();
            self.text_input.cursor = 0;
            self.push(ChatLine::Info(copy_info::INTERRUPT_QUEUE.into()));
        } else {
            self.dispatch_user_input(text);
        }
    }

    /// Interrupt current run without sending a follow-up prompt.
    pub(super) fn stop_only(&mut self) {
        if self.pending_perm.is_some() || !self.busy {
            return;
        }
        // First try to kill the agent task immediately via channel
        let _ = self.channels.kill_tx.send(());
        // Also set cancel flag as fallback
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.push(ChatLine::Info(copy_info::STOPPING.into()));
    }

    pub(super) fn push_status(&mut self, tool_use_id: String, name: String, detail: String) {
        self.status_log.push_back(StatusEntry {
            tool_use_id,
            name,
            detail,
            done: false,
            is_error: false,
            start: std::time::Instant::now(),
            elapsed_ms: None,
        });
        // Ring buffer for tool activity (status strip shows a tail; rail can show more).
        const STATUS_LOG_CAP: usize = 24;
        while self.status_log.len() > STATUS_LOG_CAP {
            self.status_log.pop_front();
        }
    }

    pub(super) fn finish_status(&mut self, tool_use_id: &str, is_error: bool) {
        for entry in self.status_log.iter_mut().rev() {
            if entry.tool_use_id == tool_use_id && !entry.done {
                entry.done = true;
                entry.is_error = is_error;
                entry.elapsed_ms = Some(entry.start.elapsed().as_millis() as u64);
                break;
            }
        }
    }

    /// Called when agent finishes a turn. Drains queue if any.
    pub(super) fn on_agent_done(&mut self) {
        self.busy = false;
        self.agent_run_state = AppRunState::Idle;
        self.status_log.clear();
        self.current_step = None;
        self.active_prompt = None;
        self.cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.stats.session_turn_count += 1;

        if let Some(line) = self.turn_tool_tally.take_summary_line() {
            self.push(ChatLine::Info(line));
        }

        // Show elapsed time and per-turn cost for the completed turn.
        if let Some(start) = self.turn_start {
            let elapsed = start.elapsed();
            let elapsed_str = if elapsed.as_secs() >= 60 {
                format!("{}m {}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
            } else if elapsed.as_secs() >= 1 {
                format!("{:.1}s", elapsed.as_secs_f64())
            } else {
                format!("{}ms", elapsed.as_millis())
            };
            // Include per-turn cost if available.
            let cost_usd = self.stats.session_cost_usd;
            let cost_str = if cost_usd > 0.0 {
                format!("  ${:.4}", cost_usd)
            } else {
                String::new()
            };
            self.push(ChatLine::Info(format!(
                "  done in {}{}",
                elapsed_str, cost_str
            )));
        }

        // If a plan was running and not all steps were completed, show remaining steps.
        if let Some(last_num) = self.last_executed_step_num {
            if let Some(plans) = self.plan.last_plan.clone() {
                let total = plans.len();
                if last_num < total {
                    self.push(ChatLine::Info(format!(
                        "  Plan: {}/{} steps completed. Remaining:",
                        last_num, total
                    )));
                    for (i, step) in plans[last_num..].iter().enumerate() {
                        let n = last_num + i + 1;
                        self.push(ChatLine::Info(format!("    {}. {}", n, step)));
                    }
                }
            }
        }
        self.last_executed_step_num = None;
        if self.plan.awaiting_plan_response {
            self.plan.awaiting_plan_response = false;
            if let Some(text) = self.last_assistant_text().map(|s| s.to_string()) {
                self.plan.last_plan_raw = Some(text.clone());
                if let Some(plan) = build_plan_from_assistant_text(&text) {
                    if let Err(e) = clido_planner::save_plan(&self.workspace_root, &plan) {
                        self.push(ChatLine::Info(format!("  ⚠ Could not save plan: {}", e)));
                    }
                    self.plan.last_plan = Some(
                        plan.tasks
                            .iter()
                            .map(|t| t.description.clone())
                            .collect::<Vec<_>>(),
                    );
                    self.plan.last_plan_snapshot = Some(plan);
                }
            }
        }
        self.drain_input_queue();
    }

    pub(super) fn tick_spinner(&mut self) {
        if self.busy || self.pending_perm.is_some() || self.enhancing {
            self.spinner_tick = (self.spinner_tick + 1) % super::SPINNER.len();
        }
    }

    pub(super) fn last_assistant_text(&self) -> Option<&str> {
        self.messages.iter().rev().find_map(|line| match line {
            ChatLine::Assistant(text) if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
    }

    // ── Selection Methods ────────────────────────────────────────────────────
    #[allow(dead_code)]
    /// Start selection at the given screen coordinates.
    pub(super) fn start_selection(&mut self, row: u16, col: u16) {
        self.selection.start(row as usize, col as usize);
    }

    /// Update selection focus while dragging.
    #[allow(dead_code)]
    #[allow(dead_code)]
    pub(super) fn update_selection(&mut self, row: u16, col: u16) {
        self.selection.update(row as usize, col as usize);
    }

    /// End selection (mouse release).
    #[allow(dead_code)]
    pub(super) fn end_selection(&mut self) {
        self.selection_mode = false;
    }

    /// Clear selection entirely.
    #[allow(dead_code)]
    pub(super) fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// Get the normalized selection bounds (start_row, start_col, end_row, end_col).
    #[allow(dead_code)]
    pub(super) fn get_selection_bounds(&self) -> Option<(u16, u16, u16, u16)> {
        if !self.selection.active {
            return None;
        }
        let (sr, sc, er, ec) = self.selection.bounds();
        Some((sr as u16, sc as u16, er as u16, ec as u16))
    }

    /// Copy selected text to clipboard.
    /// Returns true if something was copied.
    #[allow(dead_code)]
    pub(super) fn copy_selection(&mut self) -> bool {
        let text = self.get_selected_text();
        if text.is_empty() {
            return false;
        }

        // Try to copy to clipboard
        if let Err(e) = self.copy_to_clipboard(&text) {
            self.push(ChatLine::Info(format!("  ✗ Failed to copy: {}", e)));
            return false;
        }

        self.push_toast(
            format!("✓ Copied {} chars", text.len()),
            super::TUI_STATE_OK,
            std::time::Duration::from_secs(2),
        );
        true
    }

    /// Get selected text from the rendered line snapshot.
    ///
    /// Selection coordinates live in rendered-line space (the same indices
    /// used by `apply_selection_highlight`), so we read directly from
    /// `rendered_line_texts` which is populated each render tick.
    /// Get selected text from the rendered line snapshot.
    ///
    /// Selection coordinates are in **display-cell space** (terminal column
    /// positions).  This method converts display columns → character indices
    /// per-line so multi-byte and wide characters are handled correctly.
    pub(super) fn get_selected_text(&self) -> String {
        if !self.selection.active {
            return String::new();
        }

        let lines = &self.rendered_line_texts[..];
        let total_lines = lines.len();
        if total_lines == 0 {
            return String::new();
        }

        let (sr, sc, er, ec) = self.selection.bounds();
        // Clamp to actual line count.
        let sr: usize = sr.min(total_lines.saturating_sub(1));
        let er: usize = er.min(total_lines.saturating_sub(1));
        if sr >= total_lines {
            return String::new();
        }

        let mut result = String::new();

        for row in sr..=er {
            let line: &str = match lines.get(row) {
                Some(l) => l.as_str(),
                None => continue,
            };
            // Convert display-cell column → character index.
            let start_col = display_col_to_char_idx(line, sc);
            let end_col = display_col_to_char_idx(
                line,
                if row == er {
                    ec + 1
                } else {
                    line_display_width(line)
                },
            );

            let chars: Vec<char> = line.chars().collect();
            let start = start_col.min(chars.len());
            let end = end_col.min(chars.len());

            if start < end {
                result.extend(&chars[start..end]);
            }

            if row < er {
                result.push('\n');
            }
        }

        result
    }

    /// Copy text to clipboard using OSC 52 escape sequence (works over SSH)
    /// or fallback to native clipboard (pbcopy on macOS).
    pub(super) fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        // Try OSC 52 first (works over SSH, in tmux, etc.)
        if self.copy_osc52(text).is_ok() {
            return Ok(());
        }

        // Fallback to native clipboard
        #[cfg(target_os = "macos")]
        {
            use std::io::Write;
            use std::process::{Command, Stdio};

            let mut child = Command::new("pbcopy")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("Failed to spawn pbcopy: {}", e))?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(text.as_bytes())
                    .map_err(|e| format!("Failed to write to pbcopy: {}", e))?;
            }

            // Don't wait for completion - pbcopy exits immediately
            let _ = child.wait();
            Ok(())
        }

        #[cfg(target_os = "linux")]
        {
            use std::io::Write;
            use std::process::{Command, Stdio};

            // Try wl-copy (Wayland), then xclip (X11)
            let cmd = if Command::new("wl-copy").arg("--version").output().is_ok() {
                "wl-copy"
            } else {
                "xclip"
            };

            let mut child = Command::new(cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("Failed to spawn {}: {}", cmd, e))?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(text.as_bytes())
                    .map_err(|e| format!("Failed to write to clipboard: {}", e))?;
            }

            let _ = child.wait();
            Ok(())
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Err("Clipboard not supported on this platform".to_string())
        }
    }

    /// Copy to clipboard using OSC 52 terminal escape sequence.
    /// This works over SSH and in most modern terminals.
    #[allow(deprecated)]
    fn copy_osc52(&self, text: &str) -> Result<(), String> {
        use std::io::Write;

        // Base64 encode the text
        let encoded = base64::encode(text);

        // OSC 52 sequence: ESC ] 52 ; c ; <base64> BEL
        // The 'c' parameter means system clipboard
        let osc52 = format!("\x1b]52;c;{}\x07", encoded);

        // Write to stdout (the terminal)
        std::io::stdout()
            .write_all(osc52.as_bytes())
            .map_err(|e| format!("Failed to write OSC 52: {}", e))?;

        // Flush to ensure it's sent
        std::io::stdout()
            .flush()
            .map_err(|e| format!("Failed to flush: {}", e))?;

        Ok(())
    }
}
