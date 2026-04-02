use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clido_core::PermissionMode;
use ratatui::style::Color;
use ratatui::text::Line;

use crate::image_input::ImageAttachment;
use crate::overlay::OverlayStack;
use crate::repl::expand_at_file_refs;
use crate::text_input::TextInput;

use super::commands::{execute_slash, is_known_slash_cmd, parse_per_turn_model};
use super::render::build_plan_from_assistant_text;
use super::state::*;

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

    /// Layout metrics computed during render; consumed by input/event handlers.
    pub(super) layout: super::state::LayoutInfo,
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
    pub(super) wants_profile_switch: Option<String>,
    /// When true, the TUI exits and the profile-creation wizard runs, then TUI restarts.
    pub(super) wants_profile_create: bool,
    /// When Some(name), the TUI exits and the profile-edit wizard runs, then TUI restarts.
    pub(super) wants_profile_edit: Option<String>,
    /// When Some(id), restart TUI and resume this session immediately.
    pub(super) restart_resume_session: Option<String>,
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

    /// Whether we're in plan dry-run mode (show editor but never execute).
    pub(super) plan_dry_run: bool,

    /// Current plan step being executed, extracted from agent text (e.g. "Step 3: Write contract").
    pub(super) current_step: Option<String>,
    /// The step number most recently seen while the agent was executing a plan.
    /// Used after agent finishes to show which steps remain.
    pub(super) last_executed_step_num: Option<usize>,
    /// Shared todo list written by the agent via the TodoWrite tool.
    pub(super) todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
    /// Track whether we have already shown the empty-input hint this session.
    pub(super) empty_input_hint_shown: bool,
    /// Pending `/enhance` request — set by cmd_enhance, consumed by event_loop.
    pub(super) pending_enhance: Option<String>,
    /// Utility (fast) provider for background tasks like `/enhance`.
    pub(super) utility_provider: Arc<dyn clido_providers::ModelProvider>,
    /// Model name for the utility provider.
    pub(super) utility_model: String,
    /// Max budget for the session (from config), shown in header.
    pub(super) max_budget_usd: Option<f64>,

    /// Rate-limit auto-resume: when the agent hits a rate limit with a known
    /// retry_after, we set a timer. When it expires the agent is automatically
    /// sent a "continue" message so it can pick up where it left off.
    /// `None` means no auto-resume is pending.
    pub(super) rate_limit_resume_at: Option<std::time::Instant>,
    /// Whether the user has cancelled the auto-resume (Escape while waiting).
    pub(super) rate_limit_cancelled: bool,

    /// Resolved API key for the active profile — used for live model fetching.
    pub(super) api_key: String,
    /// Optional custom base URL for the active profile's provider.
    pub(super) base_url: Option<String>,

    /// True while a model-list fetch is in progress (shows spinner in model picker).
    pub(super) models_loading: bool,
    /// Render cache: maps (content_hash, render_width) to pre-built Line<'static> slices.
    /// Avoids re-parsing markdown on every 80ms render tick when messages haven't changed.
    /// Invalidated (cleared) on terminal resize since width affects line-wrapping.
    pub(super) render_cache: std::collections::HashMap<(u64, usize), Vec<Line<'static>>>,
    /// Hash of the messages Vec at the time the cache was last populated.
    /// Used to detect when messages change and stale entries should be evicted.
    pub(super) render_cache_msg_count: usize,
    /// Non-blocking toast notifications (auto-dismiss).
    pub(super) toasts: Vec<Toast>,
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
        todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
        api_key: String,
        base_url: Option<String>,
        utility_provider: Arc<dyn clido_providers::ModelProvider>,
        utility_model: String,
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
            layout: super::state::LayoutInfo::default(),

            busy: false,
            spinner_tick: 0,
            pending_perm: None,
            overlay_stack: OverlayStack::new(),
            channels,
            queued: VecDeque::new(),
            queue_nav_idx: None,
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
            wants_profile_switch: None,
            wants_profile_create: false,
            wants_profile_edit: None,
            restart_resume_session: None,
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
            current_step: None,
            last_executed_step_num: None,
            plan_dry_run,
            todo_store,
            empty_input_hint_shown: false,
            pending_enhance: None,
            utility_provider,
            utility_model,
            max_budget_usd: budget,
            rate_limit_resume_at: None,
            rate_limit_cancelled: false,
            api_key,
            base_url,
            models_loading: false,
            render_cache: std::collections::HashMap::new(),
            render_cache_msg_count: 0,
            toasts: Vec::new(),
        };
        app.messages.push(ChatLine::WelcomeSplash);
        app
    }

    pub(super) fn push(&mut self, line: ChatLine) {
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
        });
    }

    /// Remove expired toasts.
    pub(super) fn expire_toasts(&mut self) {
        let now = std::time::Instant::now();
        self.toasts.retain(|t| t.expires > now);
    }

    /// Determine which component currently owns input focus.
    /// Mirrors the priority cascade in `handle_key` but as a single query.
    pub(super) fn focus(&self) -> super::state::FocusTarget {
        use super::state::FocusTarget;
        if self.plan.text_editor.is_some() {
            FocusTarget::PlanTextEditor
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

    /// Send immediately (not busy). Moves input → chat + agent.
    /// If input starts with `@model-name prompt`, applies a per-turn model override.
    /// Send `prompt` to the agent without showing anything in the chat.
    pub(super) fn send_silent(&mut self, prompt: String) {
        let _ = self.channels.prompt_tx.send(prompt);
        self.text_input.text.clear();
        self.text_input.cursor = 0;
        self.busy = true;
        self.following = true;
        self.turn_start = Some(std::time::Instant::now());
        self.text_input.history_idx = None;
        self.text_input.history_draft.clear();
    }

    pub(super) fn send_now(&mut self, text: String) {
        // Cancel any pending rate-limit auto-resume — user is taking manual action.
        self.rate_limit_resume_at = None;
        self.rate_limit_cancelled = false;

        // If a pending image was attached via /image, publish it to the shared image_state
        // so agent_task can prepend an Image ContentBlock to this user message.
        if let Some(img) = self.pending_image.take() {
            if let Ok(mut guard) = self.image_state.lock() {
                *guard = Some((img.media_type.to_string(), img.base64_data));
            }
        }
        // Expand @file references in user input
        let text = expand_at_file_refs(&text, std::env::current_dir().ok().as_deref());
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
            if self.text_input.history.last().map(|s| s.as_str()) != Some(text.as_str()) {
                self.text_input.history.push(text);
                if self.text_input.history.len() > 1000 {
                    self.text_input.history.remove(0);
                }
            }
            self.channels.prompt_tx.send(actual_prompt)
        } else {
            self.push(ChatLine::User(text.clone()));
            if self.text_input.history.last().map(|s| s.as_str()) != Some(text.as_str()) {
                self.text_input.history.push(text.clone());
                if self.text_input.history.len() > 1000 {
                    self.text_input.history.remove(0);
                }
            }
            self.channels.prompt_tx.send(text)
        };

        if send_result.is_err() {
            // Agent task channel closed — can't send; stay idle and surface an error.
            self.push(ChatLine::Info(
                "  ✗ Agent is not running — try restarting clido.".into(),
            ));
            return;
        }

        self.text_input.text.clear();
        self.text_input.cursor = 0;
        self.busy = true;
        self.following = true;
        self.turn_start = Some(std::time::Instant::now());
        self.text_input.history_idx = None;
        self.text_input.history_draft.clear();
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
            self.push(ChatLine::Info(
                "  Type a message or command — bare '/' alone is not sent".into(),
            ));
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
                self.push(ChatLine::Info(
                    "  Type a message or command — bare '/' alone is not sent".into(),
                ));
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
                self.push(ChatLine::Info(
                    "  Type a message to start, or /help for available commands".into(),
                ));
            }
            return;
        }
        if text == "/" {
            self.push(ChatLine::Info(
                "  Type a message or command — bare '/' alone is not sent".into(),
            ));
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

        // /stop command bypasses queue and executes immediately
        if text == "/stop" {
            if self.busy {
                self.stop_only();
            } else {
                self.push(ChatLine::Info("  ✗ No active run to stop".into()));
            }
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
            self.push(ChatLine::Info(
                "  ↻ Interrupt requested — will send after current response completes".into(),
            ));
        } else {
            self.dispatch_user_input(text);
        }
    }

    /// Interrupt current run without sending a follow-up prompt.
    pub(super) fn stop_only(&mut self) {
        if self.pending_perm.is_some() || !self.busy {
            return;
        }
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.push(ChatLine::Info("  ↻ Interrupted".into()));
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
        // Keep only the last 2 visible entries.
        while self.status_log.len() > 2 {
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
        self.status_log.clear();
        self.current_step = None;
        self.cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.stats.session_turn_count += 1;

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
        if self.busy || self.pending_perm.is_some() {
            self.spinner_tick = (self.spinner_tick + 1) % super::SPINNER.len();
        }
    }

    pub(super) fn last_assistant_text(&self) -> Option<&str> {
        self.messages.iter().rev().find_map(|line| match line {
            ChatLine::Assistant(text) if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
    }
}
