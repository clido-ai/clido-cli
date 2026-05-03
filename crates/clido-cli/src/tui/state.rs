use clido_planner::{Complexity, Plan, PlanEditor};
use ratatui::style::Color;
use ratatui::text::Span;
use tokio::sync::{mpsc, oneshot};
use unicode_width::UnicodeWidthStr;

use crate::list_picker::{ListPicker, PickerItem};

use super::render::parse_plan_from_text;
use super::{AgentEvent, PermGrant};

// ── Helper functions for profile overlay ──────────────────────────────────────

/// Build saved-key catalog from all profiles, checking credentials file + env + inline key.
pub(crate) fn build_saved_keys_from_profiles(
    profiles: &std::collections::HashMap<String, clido_core::ProfileEntry>,
    config_path: &std::path::Path,
    exclude: Option<&str>,
) -> Vec<ProfileSavedKey> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (name, entry) in profiles {
        if exclude == Some(name.as_str()) {
            continue;
        }
        // Read from credentials file, env var, then inline (same order as setup/mod.rs)
        let key = crate::setup::read_credential(config_path, &entry.provider)
            .or_else(|| {
                entry
                    .api_key_env
                    .as_ref()
                    .and_then(|e| std::env::var(e).ok())
            })
            .or_else(|| entry.api_key.clone());
        let Some(k) = key else {
            continue;
        };
        if k.is_empty() || !seen.insert(k.clone()) {
            continue;
        }
        // Anonymize: show first 4 and last 4 characters.
        let display = if k.len() <= 12 {
            format!(
                "{}•••{}",
                &k[..2.min(k.len())],
                &k[k.len() - 2.min(k.len())..]
            )
        } else {
            format!("{}•••{}", &k[..4], &k[k.len() - 4..])
        };
        out.push(ProfileSavedKey {
            source_profile: name.clone(),
            provider_id: entry.provider.clone(),
            display,
        });
    }
    out
}

/// Detect which provider has an env var set (mimics first-run setup).
/// Returns (provider_id, env_var_name).
fn detect_provider_from_env() -> Option<(&'static str, &'static str)> {
    let check: [(&str, &str); 11] = [
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("google", "GEMINI_API_KEY"),
        ("groq", "GROQ_API_KEY"),
        ("xai", "XAI_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"),
        ("openrouter", "OPENROUTER_API_KEY"),
        ("mistral", "MISTRAL_API_KEY"),
        ("perplexity", "PERPLEXITY_API_KEY"),
        ("togetherai", "TOGETHER_API_KEY"),
        ("fireworks", "FIREWORKS_API_KEY"),
    ];
    for (id, var) in check {
        if std::env::var(var).is_ok_and(|v| !v.is_empty()) {
            return Some((id, var));
        }
    }
    None
}

// ── Focus management ──────────────────────────────────────────────────────────

/// Which component currently owns keyboard (and optionally mouse) input.
/// Determined dynamically from App state — no separate stack needed because the
/// existing Option fields already encode the open/close lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusTarget {
    /// Main text input field — the default when nothing else is open.
    ChatInput,
    /// Plan text editor (nano-style, full-screen).
    PlanTextEditor,
    /// Plan editor modal (task list, full-screen).
    PlanEditor,
    /// Profile overlay (overview / create / edit wizard).
    ProfileOverlay,
    /// Workflow text editor (nano-style, full-screen).
    WorkflowEditor,
    /// Unified overlay stack (error, read-only, choice).
    Overlay,
    /// Model picker popup.
    ModelPicker,
    /// Session picker popup.
    SessionPicker,
    /// Profile picker popup.
    ProfilePicker,
    /// Permission approval dialog.
    Permission,
}

impl FocusTarget {
    /// Returns true if this focus target is a modal overlay that should block
    /// normal input handling.
    pub(crate) fn is_modal(self) -> bool {
        !matches!(self, FocusTarget::ChatInput)
    }
}

impl FocusTarget {}

// ── Layout info ───────────────────────────────────────────────────────────────

/// Layout metrics computed during render and consumed by input/event handlers.
/// Avoids the need for render to mutate App state for these values.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LayoutInfo {
    /// Screen-Y bounds of the chat area (top, bottom).
    pub(crate) chat_area_y: (u16, u16),
    /// Width of the chat area in columns.
    pub(crate) chat_area_width: u16,
    /// Maximum scroll offset for the chat content (total_lines − visible_lines).
    pub(crate) max_scroll: u32,
    /// True when the layout allocated the right-hand status rail this frame (wide terminal + `/panel`).
    pub(crate) status_rail_active: bool,
    /// Max scroll for the status rail (`0` when all lines fit).
    pub(crate) status_panel_max_scroll: u16,
}

impl Default for LayoutInfo {
    fn default() -> Self {
        Self {
            chat_area_y: (0, 0),
            chat_area_width: 120,
            max_scroll: 0,
            status_rail_active: false,
            status_panel_max_scroll: 0,
        }
    }
}

// ── Session statistics ────────────────────────────────────────────────────────

/// Accumulated token/cost counters for the current TUI session.
#[derive(Default)]
pub(crate) struct SessionStats {
    /// Last completed agent invocation's token totals (for context % in header).
    pub(crate) session_input_tokens: u64,
    pub(crate) session_output_tokens: u64,
    pub(crate) session_cost_usd: f64,
    /// Running totals across all completed turns in this TUI session (including planning calls).
    pub(crate) session_total_input_tokens: u64,
    pub(crate) session_total_output_tokens: u64,
    pub(crate) session_total_cost_usd: f64,
    /// Number of completed agent turns in this TUI session.
    pub(crate) session_turn_count: u32,
}

// ── Side panel & task strip (TUI) ────────────────────────────────────────────

/// User preference for the **right status column** (session, git, agent, queue, tasks, tools).
/// `/panel on|off|auto` — independent of the stacked task strip on narrow layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StatusRailVisibility {
    /// Show the rail when the terminal is wide enough ([`crate::tui::render::status_panel::STATUS_RAIL_MIN_TERM_WIDTH`]).
    #[default]
    Auto,
    /// Prefer the rail from a slightly lower width ([`crate::tui::render::status_panel::STATUS_RAIL_MIN_TERM_WIDTH_ON`]).
    On,
    /// Never use the side rail; use the stacked bottom layout even on wide terminals.
    Off,
}

/// User preference for the **task list** strip: todos, planner snapshot, harness rows, live agent step.
/// On wide layouts this appears inside the side panel; on narrow layouts it stacks above the status line.
/// `/tasks on|off|auto` (alias: `/progress`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlanPanelVisibility {
    /// Show the panel only when the terminal is large enough and there is content.
    #[default]
    Auto,
    /// Always show when the terminal meets a minimum size (may show an empty hint).
    On,
    /// Never show the panel.
    Off,
}

// ── Plan state ────────────────────────────────────────────────────────────────

/// All plan-related fields grouped together.
#[derive(Default)]
pub(crate) struct PlanState {
    /// Last plan produced by the planner (--planner mode) or parsed from a /plan <task> response.
    pub(crate) last_plan_snapshot: Option<Plan>,
    /// Convenience list of top-level task descriptions derived from `last_plan_snapshot`.
    pub(crate) last_plan: Option<Vec<String>>,
    /// Raw text of the last plan response — used by the text editor to show unmodified formatting.
    pub(crate) last_plan_raw: Option<String>,
    /// Set to true when /plan <task> is sent; cleared after the agent responds and the plan is parsed.
    pub(crate) awaiting_plan_response: bool,
    /// Set to true when reviewer is triggered after plan completion; cleared after review response.
    pub(crate) awaiting_review_response: bool,
    /// When `Some`, the plan editor full-screen overlay is active (--plan flag mode).
    pub(crate) editor: Option<PlanEditor>,
    /// Currently selected task index in the plan editor list.
    pub(crate) selected_task: usize,
    /// When `Some`, the inline task edit form is active.
    pub(crate) task_editing: Option<TaskEditState>,
    /// Simple nano-style text editor for /plan edit.
    pub(crate) text_editor: Option<PlanTextEditor>,
}

// ── Agent channels ────────────────────────────────────────────────────────────

/// Message to the background agent task (replaces a bare `String` prompt).
#[derive(Debug, Clone)]
pub(crate) enum AgentUserInput {
    /// Normal user or `/plan` prompt → `run` / `run_next_turn`.
    Prompt(String),
    /// Call `AgentLoop::run_continue` — no new user line; used after rate-limit recovery.
    ContinueTurn,
}

/// mpsc senders used to communicate with the background agent task.
pub(crate) struct AgentChannels {
    pub(crate) prompt_tx: mpsc::UnboundedSender<AgentUserInput>,
    /// Channel to request session resume in agent_task.
    pub(crate) resume_tx: mpsc::UnboundedSender<String>,
    /// Channel to switch the session model in agent_task.
    pub(crate) model_switch_tx: mpsc::UnboundedSender<String>,
    /// Channel to update tool workspace in agent_task.
    pub(crate) workdir_tx: mpsc::UnboundedSender<std::path::PathBuf>,
    /// Channel to trigger immediate context compaction in agent_task.
    pub(crate) compact_now_tx: mpsc::UnboundedSender<()>,
    /// Channel to send AgentEvents from background tasks (e.g. model fetch) to the TUI loop.
    /// Bounded for backpressure — slow UI applies pressure instead of unbounded memory growth.
    pub(crate) fetch_tx: mpsc::Sender<AgentEvent>,
    /// Channel to force abort the agent task immediately (for /stop command).
    pub(crate) kill_tx: mpsc::UnboundedSender<()>,
    /// Channel to update allowed external paths for this session.
    pub(crate) allowed_paths_tx: mpsc::UnboundedSender<Vec<std::path::PathBuf>>,
    /// Channel to inject a note/hint into the running conversation.
    pub(crate) note_tx: mpsc::UnboundedSender<String>,
    /// Channel to grant permission for external path access (user clicked "Allow").
    pub(crate) path_permission_tx: mpsc::UnboundedSender<std::path::PathBuf>,
    /// Channel to request switching to a different profile seamlessly.
    pub(crate) profile_switch_tx: mpsc::UnboundedSender<String>,
}

// ── Plan editor state ─────────────────────────────────────────────────────────

/// Simple nano-style full-screen plan text editor.
pub(crate) struct PlanTextEditor {
    pub(crate) lines: Vec<String>,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
    pub(crate) scroll: usize,
}

impl PlanTextEditor {
    pub(crate) fn from_raw(text: &str) -> Self {
        let lines = text.lines().map(|l| l.to_string()).collect();
        Self {
            lines,
            cursor_row: 0,
            cursor_col: 0,
            scroll: 0,
        }
    }

    pub(crate) fn to_tasks(&self) -> Vec<String> {
        parse_plan_from_text(&self.lines.join("\n"))
    }

    pub(crate) fn clamp_col(&mut self) {
        let max = self
            .lines
            .get(self.cursor_row)
            .map(|l| l.chars().count())
            .unwrap_or(0);
        if self.cursor_col > max {
            self.cursor_col = max;
        }
    }
}

/// Which field is focused in the inline task edit form.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TaskEditField {
    Description,
    Notes,
    Complexity,
}

/// State for the inline task edit form.
pub(crate) struct TaskEditState {
    pub(crate) task_id: String,
    pub(crate) description: String,
    pub(crate) notes: String,
    pub(crate) complexity: Complexity,
    pub(crate) focused_field: TaskEditField,
}

impl TaskEditState {
    pub(crate) fn new(
        task_id: &str,
        description: &str,
        notes: &str,
        complexity: Complexity,
    ) -> Self {
        Self {
            task_id: task_id.to_string(),
            description: description.to_string(),
            notes: notes.to_string(),
            complexity,
            focused_field: TaskEditField::Description,
        }
    }
}

// ── PickerItem implementations ────────────────────────────────────────────────

impl PickerItem for clido_storage::SessionSummary {
    fn filter_text(&self) -> String {
        match self.title.as_deref() {
            Some(t) if !t.is_empty() => format!("{} {}", self.session_id, t),
            _ => self.session_id.clone(),
        }
    }
    fn filter_text_secondary(&self) -> Option<String> {
        Some(self.preview.clone())
    }
}

impl PickerItem for (String, clido_core::ProfileEntry) {
    fn filter_text(&self) -> String {
        self.0.clone()
    }
    fn filter_text_secondary(&self) -> Option<String> {
        Some(format!("{} {}", self.1.provider, self.1.model))
    }
}

impl PickerItem for (String, String) {
    fn filter_text(&self) -> String {
        self.0.clone()
    }
    fn filter_text_secondary(&self) -> Option<String> {
        Some(self.1.clone())
    }
}

// ── File picker popup state ───────────────────────────────────────────────────

/// Where the selected path should be delivered when the file picker confirms.
#[derive(Clone)]
pub(crate) enum FilePickerTarget {
    /// Fill a specific field in the workflow input form.
    WorkflowFormField(usize),
    /// Insert the path at the cursor in the main chat text input.
    MainTextInput,
}

#[derive(Clone)]
pub(crate) struct FileEntry {
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) path: std::path::PathBuf,
}

pub(crate) struct FilePickerState {
    pub(crate) current_dir: std::path::PathBuf,
    pub(crate) entries: Vec<FileEntry>,
    pub(crate) selected: usize,
    pub(crate) scroll_offset: usize,
    pub(crate) filter: String,
    pub(crate) target: FilePickerTarget,
}

impl FilePickerState {
    pub(crate) fn new(start_dir: std::path::PathBuf, target: FilePickerTarget) -> Self {
        let mut s = Self {
            current_dir: start_dir,
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            filter: String::new(),
            target,
        };
        s.reload();
        s
    }

    pub(crate) fn reload(&mut self) {
        self.entries.clear();
        if let Ok(rd) = std::fs::read_dir(&self.current_dir) {
            let mut dirs: Vec<FileEntry> = Vec::new();
            let mut files: Vec<FileEntry> = Vec::new();
            for entry in rd.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue; // skip hidden by default
                }
                let is_dir = path.is_dir();
                let fe = FileEntry { name, is_dir, path };
                if is_dir {
                    dirs.push(fe);
                } else {
                    files.push(fe);
                }
            }
            dirs.sort_by(|a, b| a.name.cmp(&b.name));
            files.sort_by(|a, b| a.name.cmp(&b.name));
            self.entries = dirs;
            self.entries.extend(files);
        }
        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub(crate) fn filtered(&self) -> Vec<&FileEntry> {
        let f = self.filter.to_lowercase();
        if f.is_empty() {
            return self.entries.iter().collect();
        }
        self.entries
            .iter()
            .filter(|e| e.name.to_lowercase().contains(&f))
            .collect()
    }

    pub(crate) fn enter_dir(&mut self, path: std::path::PathBuf) {
        self.current_dir = path;
        self.filter.clear();
        self.reload();
    }

    pub(crate) fn go_up(&mut self) {
        if let Some(parent) = self.current_dir.parent().map(|p| p.to_path_buf()) {
            self.current_dir = parent;
            self.filter.clear();
            self.reload();
        }
    }

    pub(crate) fn clamp(&mut self) {
        let n = self.filtered().len();
        if n == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
        } else {
            self.selected = self.selected.min(n - 1);
        }
    }
}

// ── Workflow input form state ─────────────────────────────────────────────────

/// A single editable field in the workflow input form.
pub(crate) struct InputFormField {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) required: bool,
    /// Default value as a display string (empty if none).
    pub(crate) default_val: String,
    /// Current value being entered by the user.
    pub(crate) value: String,
    /// Whether this field should offer the file picker.
    pub(crate) is_path: bool,
}

impl InputFormField {
    pub(crate) fn effective_value(&self) -> &str {
        if self.value.is_empty() {
            &self.default_val
        } else {
            &self.value
        }
    }
}

pub(crate) struct WorkflowInputFormState {
    pub(crate) workflow_path: std::path::PathBuf,
    pub(crate) fields: Vec<InputFormField>,
    pub(crate) current_field: usize,
    pub(crate) profile_override: Option<String>,
    /// Cursor position within the current field's text input.
    pub(crate) cursor: usize,
}

impl WorkflowInputFormState {
    #[allow(dead_code)]
    pub(crate) fn current_mut(&mut self) -> &mut InputFormField {
        &mut self.fields[self.current_field]
    }

    pub(crate) fn collect_inputs(&self) -> Vec<(String, String)> {
        self.fields
            .iter()
            .filter_map(|f| {
                let v = f.effective_value();
                if v.is_empty() {
                    None
                } else {
                    Some((f.name.clone(), v.to_string()))
                }
            })
            .collect()
    }
}

// ── Workflow picker popup state ───────────────────────────────────────────────

/// One entry in the workflow picker.
#[derive(Clone)]
pub(crate) struct WorkflowEntry {
    pub(crate) name: String,
    pub(crate) desc: String,
    pub(crate) steps: usize,
    pub(crate) is_local: bool,
}

/// What to do when the user selects a workflow from the picker.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkflowPickerAction {
    Run,
    Show,
    Edit,
    AgentEdit,
}

pub(crate) struct WorkflowPickerState {
    pub(crate) workflows: Vec<WorkflowEntry>,
    pub(crate) selected: usize,
    pub(crate) scroll_offset: usize,
    pub(crate) filter: String,
    pub(crate) action: WorkflowPickerAction,
}

impl WorkflowPickerState {
    pub(crate) fn filtered(&self) -> Vec<&WorkflowEntry> {
        let f = self.filter.trim().to_lowercase();
        if f.is_empty() {
            return self.workflows.iter().collect();
        }
        self.workflows
            .iter()
            .filter(|w| w.name.to_lowercase().contains(&f) || w.desc.to_lowercase().contains(&f))
            .collect()
    }

    pub(crate) fn clamp(&mut self) {
        let n = self.filtered().len();
        if n == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
        } else {
            self.selected = self.selected.min(n - 1);
        }
    }
}

// ── Session picker popup state ────────────────────────────────────────────────

pub(crate) struct SessionPickerState {
    pub(crate) picker: ListPicker<clido_storage::SessionSummary>,
    /// Multi-selected session IDs for bulk delete.
    pub(crate) selected: std::collections::HashSet<String>,
}

// ── Profile picker popup state ─────────────────────────────────────────────────

pub(crate) struct ProfilePickerState {
    pub(crate) picker: ListPicker<(String, clido_core::ProfileEntry)>,
    /// Currently active profile name (shown with ▶ marker).
    pub(crate) active: String,
}

// ── Model picker popup state ──────────────────────────────────────────────────

/// One row in the model picker.
#[derive(Clone)]
pub(crate) struct ModelEntry {
    pub(crate) id: String,
    pub(crate) provider: String,
    /// Cost per million input tokens (USD).
    pub(crate) input_mtok: f64,
    /// Cost per million output tokens (USD).
    pub(crate) output_mtok: f64,
    /// Context window in thousands of tokens.
    pub(crate) context_k: Option<u32>,
    /// Role name assigned to this model (e.g. "fast", "reasoning").
    pub(crate) role: Option<String>,
    /// Whether the user has starred this model.
    pub(crate) is_favorite: bool,
}

pub(crate) struct ModelPickerState {
    /// Full sorted model list (favorites → recent → rest).
    pub(crate) models: Vec<ModelEntry>,
    /// Live filter string.
    pub(crate) filter: String,
    /// Selected index within the *filtered* view.
    pub(crate) selected: usize,
    pub(crate) scroll_offset: usize,
}

impl ModelPickerState {
    pub(crate) fn filtered(&self) -> Vec<&ModelEntry> {
        let f = self.filter.trim().to_lowercase();
        if f.is_empty() {
            return self.models.iter().collect();
        }
        self.models
            .iter()
            .filter(|m| {
                m.id.to_lowercase().contains(&f)
                    || m.provider.to_lowercase().contains(&f)
                    || m.role
                        .as_deref()
                        .map(|r| r.to_lowercase().contains(&f))
                        .unwrap_or(false)
            })
            .collect()
    }

    pub(crate) fn clamp(&mut self) {
        let n = self.filtered().len();
        if n == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
        } else {
            self.selected = self.selected.min(n - 1);
            self.scroll_offset = self.scroll_offset.min(self.selected);
        }
    }

    /// Replace the full model list (e.g. after a live API fetch) and re-clamp selection.
    pub(crate) fn refresh_models(&mut self, ids: Vec<String>) {
        self.models = ids
            .into_iter()
            .map(|id| ModelEntry {
                id,
                provider: String::new(),
                input_mtok: 0.0,
                output_mtok: 0.0,
                context_k: None,
                role: None,
                is_favorite: false,
            })
            .collect();
        self.clamp();
    }
}

/// Known providers with their display name and whether they require an API key.
pub(crate) const KNOWN_PROVIDERS: &[(&str, &str, bool)] = &[
    ("anthropic", "Anthropic  (Claude)", true),
    ("openai", "OpenAI  (GPT / o-series)", true),
    ("gemini", "Google  (Gemini)", true),
    ("xai", "xAI  (Grok)", true),
    ("deepseek", "DeepSeek", true),
    ("groq", "Groq", true),
    ("cerebras", "Cerebras", true),
    ("togetherai", "Together AI", true),
    ("fireworks", "Fireworks AI", true),
    ("perplexity", "Perplexity", true),
    ("mistral", "Mistral", true),
    ("kimi", "Moonshot AI  (Kimi)", true),
    ("kimi-code", "Kimi Code  (coding)", true),
    ("alibabacloud", "Alibaba Cloud  (Qwen)", true),
    ("alibabacloud-code", "Alibaba Cloud  (coding plan)", true),
    ("minimax", "MiniMax", true),
    ("openrouter", "OpenRouter", true),
    ("ollama", "Ollama  (local)", false),
    ("local", "Local  (OpenAI-compatible)", false),
];

pub(crate) struct ProviderPickerState {
    /// Index of the highlighted provider in KNOWN_PROVIDERS.
    pub(crate) selected: usize,
    /// Live filter string typed by user.
    pub(crate) filter: String,
    pub(crate) scroll_offset: usize,
}

impl ProviderPickerState {
    pub(crate) fn new() -> Self {
        Self {
            selected: 0,
            filter: String::new(),
            scroll_offset: 0,
        }
    }

    pub(crate) fn filtered(&self) -> Vec<usize> {
        let f = self.filter.trim().to_lowercase();
        KNOWN_PROVIDERS
            .iter()
            .enumerate()
            .filter(|(_, (id, name, _))| {
                f.is_empty() || id.contains(&f) || name.to_lowercase().contains(&f)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub(crate) fn clamp(&mut self) {
        let n = self.filtered().len();
        if n == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
        } else {
            self.selected = self.selected.min(n.saturating_sub(1));
            self.scroll_offset = self.scroll_offset.min(self.selected);
        }
    }

    /// The selected provider id string, or empty if none.
    pub(crate) fn selected_id(&self) -> Option<&'static str> {
        let indices = self.filtered();
        indices.get(self.selected).map(|&i| KNOWN_PROVIDERS[i].0)
    }

    /// Whether the currently selected provider requires an API key.
    pub(crate) fn selected_requires_key(&self) -> bool {
        let indices = self.filtered();
        indices
            .get(self.selected)
            .map(|&i| KNOWN_PROVIDERS[i].2)
            .unwrap_or(true)
    }
}

// ── Profile overview overlay ──────────────────────────────────────────────────

/// Which field of a profile is being inline-edited.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProfileEditField {
    None,
    Provider,
    ApiKey,
    Model,
    BaseUrl,
    FastProvider,
    FastApiKey,
    FastModel,
}

/// Screen mode for the profile overlay.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProfileOverlayMode {
    /// Showing all fields; user navigates with arrows and picks one to edit.
    Overview,
    /// User is editing a specific field inline.
    EditField(ProfileEditField),
    /// Provider picker — shown when editing a provider field (or wizard provider step).
    PickingProvider { for_field: ProfileEditField },
    /// Model picker — shown when editing a model field (or wizard model step).
    PickingModel { for_field: ProfileEditField },
    /// User is creating a new profile, step-by-step.
    Creating { step: ProfileCreateStep },
    /// Saved API key picker during profile creation.
    PickingSavedKey {
        selected: usize,
        /// Extra row after saved keys: enter a new API key manually.
        show_type_new_row: bool,
    },
}

/// Steps for the in-TUI new profile wizard.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProfileCreateStep {
    Name,
    Provider,
    /// Base URL step — only shown for providers where `needs_base_url == true` (e.g. alibabacloud, local).
    BaseUrl,
    ApiKey,
    Model,
}

/// New-profile name step: pick auto-generated vs custom before typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ProfileCreateNameChoice {
    /// Continue without a name; it will be derived from the provider.
    #[default]
    AutoGenerate,
    /// User types a profile name in the text field.
    TypeCustomName,
}

/// A saved API key offered for reuse in the profile overlay.
#[derive(Debug, Clone)]
pub(crate) struct ProfileSavedKey {
    pub(crate) source_profile: String,
    pub(crate) provider_id: String,
    /// Anonymized display (first 4 •••• last 4).
    pub(crate) display: String,
}

/// In-TUI profile overview/editor overlay — never exits the TUI.
pub(crate) struct ProfileOverlayState {
    /// Profile name being viewed/edited (empty = creating new).
    pub(crate) name: String,
    /// Live-editable fields — main agent.
    pub(crate) provider: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) base_url: String,
    /// Fast/utility provider (optional; empty = not configured).
    pub(crate) fast_provider: String,
    pub(crate) fast_api_key: String,
    pub(crate) fast_model: String,
    /// Current cursor row in overview mode (0-based index into PROFILE_FIELDS).
    pub(crate) cursor: usize,
    /// Current overlay mode.
    pub(crate) mode: ProfileOverlayMode,
    /// Staging buffer for inline text editing.
    pub(crate) input: String,
    /// Cursor position inside `input`.
    pub(crate) input_cursor: usize,
    /// Status message shown at the bottom.
    pub(crate) status: Option<String>,
    /// Config file path for persistence.
    pub(crate) config_path: std::path::PathBuf,
    /// Whether this is a new-profile creation flow.
    pub(crate) is_new: bool,
    /// State for the provider picker (used in PickingProvider mode).
    pub(crate) provider_picker: ProviderPickerState,
    /// State for the model picker inside the profile overlay (used in PickingModel mode).
    pub(crate) profile_model_picker: Option<ModelPickerState>,
    /// Saved keys available for reuse when creating this profile (built from credentials file + env vars of other profiles).
    pub(crate) saved_keys: Vec<ProfileSavedKey>,
    /// During `/profile new` wizard — name step highlights auto vs custom (arrow keys).
    pub(crate) profile_create_name_choice: ProfileCreateNameChoice,
}

/// Display labels and keys for the editable profile fields (in order).
/// The special key "__section__" marks a non-editable section header divider.
pub(crate) const PROFILE_FIELDS: &[(&str, &str)] = &[
    ("provider", "Provider"),
    ("api_key", "API Key"),
    ("model", "Model"),
    ("base_url", "Custom Endpoint (optional)"),
    ("__section__", "── Fast/Utility Provider (optional) ──"),
    ("fast_provider", "Fast Provider"),
    ("fast_api_key", "Fast API Key"),
    ("fast_model", "Fast Model"),
];

impl ProfileOverlayState {
    /// Open an existing profile for editing.
    pub(crate) fn for_edit(
        name: String,
        entry: &clido_core::ProfileEntry,
        config_path: std::path::PathBuf,
        all_profiles: &std::collections::HashMap<String, clido_core::ProfileEntry>,
    ) -> Self {
        let env_var = entry
            .api_key_env
            .as_deref()
            .unwrap_or_else(|| crate::provider::default_api_key_env(&entry.provider));
        let api_key = crate::setup::read_credential(&config_path, &entry.provider)
            .or_else(|| std::env::var(env_var).ok())
            .or_else(|| entry.api_key.clone())
            .unwrap_or_default();
        let fast_provider = entry
            .fast
            .as_ref()
            .map(|f| f.provider.clone())
            .unwrap_or_default();
        let fast_api_key = entry
            .fast
            .as_ref()
            .and_then(|f| {
                let env = f
                    .api_key_env
                    .as_deref()
                    .unwrap_or_else(|| crate::provider::default_api_key_env(&f.provider));
                crate::setup::read_credential(&config_path, &f.provider)
                    .or_else(|| std::env::var(env).ok())
                    .or_else(|| f.api_key.clone())
            })
            .unwrap_or_default();
        let fast_model = entry
            .fast
            .as_ref()
            .map(|f| f.model.clone())
            .unwrap_or_default();

        let saved_keys = build_saved_keys_from_profiles(all_profiles, &config_path, Some(&name));

        Self {
            name,
            provider: entry.provider.clone(),
            api_key,
            model: entry.model.clone(),
            base_url: entry.base_url.clone().unwrap_or_default(),
            fast_provider,
            fast_api_key,
            fast_model,
            cursor: 0,
            mode: ProfileOverlayMode::Overview,
            input: String::new(),
            input_cursor: 0,
            status: None,
            config_path,
            is_new: false,
            provider_picker: ProviderPickerState::new(),
            profile_model_picker: None,
            saved_keys,
            profile_create_name_choice: ProfileCreateNameChoice::default(),
        }
    }

    /// Open a blank state for creating a new profile.
    pub(crate) fn for_create(
        config_path: std::path::PathBuf,
        all_profiles: &std::collections::HashMap<String, clido_core::ProfileEntry>,
    ) -> Self {
        // Check if a provider is pre-detected via env vars (same as first-run)
        let detected = detect_provider_from_env();

        let api_key = detected
            .as_ref()
            .and_then(|(_, env_var)| std::env::var(env_var).ok())
            .unwrap_or_default();

        let provider = detected
            .as_ref()
            .map(|(id, _)| id.to_string())
            .unwrap_or_default();

        let saved_keys = build_saved_keys_from_profiles(all_profiles, &config_path, None);

        Self {
            name: String::new(),
            provider: provider.clone(),
            api_key,
            model: String::new(),
            base_url: String::new(),
            fast_provider: String::new(),
            fast_api_key: String::new(),
            fast_model: String::new(),
            cursor: 0,
            mode: if provider.is_empty() {
                ProfileOverlayMode::Creating {
                    step: ProfileCreateStep::Name,
                }
            } else {
                // Provider detected via env, skip to model selection
                ProfileOverlayMode::Creating {
                    step: ProfileCreateStep::Model,
                }
            },
            input: String::new(),
            input_cursor: 0,
            status: None,
            config_path,
            is_new: true,
            provider_picker: ProviderPickerState::new(),
            profile_model_picker: None,
            saved_keys,
            profile_create_name_choice: ProfileCreateNameChoice::default(),
        }
    }

    /// Value of the field at `cursor`.
    pub(crate) fn field_value(&self, field: &ProfileEditField) -> String {
        match field {
            ProfileEditField::Provider => self.provider.clone(),
            ProfileEditField::ApiKey => self.api_key.clone(),
            ProfileEditField::Model => self.model.clone(),
            ProfileEditField::BaseUrl => self.base_url.clone(),
            ProfileEditField::FastProvider => self.fast_provider.clone(),
            ProfileEditField::FastApiKey => self.fast_api_key.clone(),
            ProfileEditField::FastModel => self.fast_model.clone(),
            ProfileEditField::None => String::new(),
        }
    }

    /// The `ProfileEditField` corresponding to cursor row.
    /// cursor 0-3 = main agent fields; 4-6 = fast provider fields.
    pub(crate) fn cursor_field(&self) -> ProfileEditField {
        match self.cursor {
            0 => ProfileEditField::Provider,
            1 => ProfileEditField::ApiKey,
            2 => ProfileEditField::Model,
            3 => ProfileEditField::BaseUrl,
            4 => ProfileEditField::FastProvider,
            5 => ProfileEditField::FastApiKey,
            6 => ProfileEditField::FastModel,
            _ => ProfileEditField::None,
        }
    }

    /// Total number of editable cursor positions.
    pub(crate) fn field_count() -> usize {
        7
    }

    /// Start editing the field at `cursor`.
    pub(crate) fn begin_edit(&mut self, known_models: &[ModelEntry]) {
        let field = self.cursor_field();
        match field {
            ProfileEditField::Provider | ProfileEditField::FastProvider => {
                let current = self.field_value(&field);
                self.provider_picker = ProviderPickerState::new();
                let indices = self.provider_picker.filtered();
                if let Some(pos) = indices
                    .iter()
                    .position(|&i| KNOWN_PROVIDERS[i].0 == current.as_str())
                {
                    self.provider_picker.selected = pos;
                }
                self.mode = ProfileOverlayMode::PickingProvider { for_field: field };
            }
            ProfileEditField::Model | ProfileEditField::FastModel => {
                let target_provider = match field {
                    ProfileEditField::Model => &self.provider,
                    ProfileEditField::FastModel => &self.fast_provider,
                    _ => unreachable!(),
                };
                let filtered: Vec<ModelEntry> = if target_provider.is_empty() {
                    known_models.to_vec()
                } else {
                    known_models
                        .iter()
                        .filter(|m| m.provider.eq_ignore_ascii_case(target_provider))
                        .cloned()
                        .collect()
                };
                let mut picker = ModelPickerState {
                    models: filtered,
                    filter: String::new(),
                    selected: 0,
                    scroll_offset: 0,
                };
                picker.clamp();
                self.profile_model_picker = Some(picker);
                self.mode = ProfileOverlayMode::PickingModel { for_field: field };
            }
            _ => {
                self.input = self.field_value(&field);
                self.input_cursor = self.input.chars().count();
                self.mode = ProfileOverlayMode::EditField(field);
            }
        }
    }

    /// Commit the current input to the field being edited and return to overview.
    pub(crate) fn commit_edit(&mut self) {
        if let ProfileOverlayMode::EditField(ref field) = self.mode.clone() {
            match field {
                ProfileEditField::Provider => self.provider = self.input.trim().to_string(),
                ProfileEditField::ApiKey => self.api_key = self.input.trim().to_string(),
                ProfileEditField::Model => self.model = self.input.trim().to_string(),
                ProfileEditField::BaseUrl => self.base_url = self.input.trim().to_string(),
                ProfileEditField::FastProvider => {
                    self.fast_provider = self.input.trim().to_string()
                }
                ProfileEditField::FastApiKey => self.fast_api_key = self.input.trim().to_string(),
                ProfileEditField::FastModel => self.fast_model = self.input.trim().to_string(),
                ProfileEditField::None => {}
            }
        }
        self.mode = ProfileOverlayMode::Overview;
        self.input.clear();
        self.input_cursor = 0;
    }

    /// Abandon in-progress edit and return to overview.
    pub(crate) fn cancel_edit(&mut self) {
        self.mode = ProfileOverlayMode::Overview;
        self.input.clear();
        self.input_cursor = 0;
    }

    pub(crate) fn commit_provider_pick(&mut self) {
        if let ProfileOverlayMode::PickingProvider { ref for_field } = self.mode.clone() {
            if let Some(id) = self.provider_picker.selected_id() {
                match for_field {
                    ProfileEditField::Provider => self.provider = id.to_string(),
                    ProfileEditField::FastProvider => self.fast_provider = id.to_string(),
                    _ => {}
                }

                // Check if this provider typically needs an API key
                let needs_key = matches!(
                    id,
                    "openai" | "anthropic" | "openrouter" | "mistral" | "alibabacloud"
                );

                // Get current key for this provider scope
                let has_key = match for_field {
                    ProfileEditField::Provider => !self.api_key.is_empty(),
                    ProfileEditField::FastProvider => !self.fast_api_key.is_empty(),
                    _ => true,
                };

                if needs_key && !has_key {
                    // Prompt for API key before returning to overview
                    self.provider_picker = ProviderPickerState::new();
                    let target_field = match for_field {
                        ProfileEditField::Provider => ProfileEditField::ApiKey,
                        ProfileEditField::FastProvider => ProfileEditField::FastApiKey,
                        _ => ProfileEditField::ApiKey,
                    };
                    self.input.clear();
                    self.input_cursor = 0;
                    self.mode = ProfileOverlayMode::EditField(target_field);
                    return;
                }
            }
        }
        self.provider_picker = ProviderPickerState::new();
        self.mode = ProfileOverlayMode::Overview;
    }

    pub(crate) fn commit_model_pick(&mut self) {
        if let ProfileOverlayMode::PickingModel { ref for_field } = self.mode.clone() {
            if let Some(picker) = &self.profile_model_picker {
                let filtered = picker.filtered();
                if let Some(m) = filtered.get(picker.selected) {
                    let id = m.id.clone();
                    match for_field {
                        ProfileEditField::Model => self.model = id,
                        ProfileEditField::FastModel => self.fast_model = id,
                        _ => {}
                    }
                }
            }
        }
        self.profile_model_picker = None;
        self.mode = ProfileOverlayMode::Overview;
    }

    /// Persist the current state to the config file and report result.
    /// API keys for remote providers are saved to the credentials file
    /// (matching first-run setup behavior) and omitted from config.toml.
    pub(crate) fn save(&mut self) {
        use clido_providers::PROVIDER_REGISTRY;

        let is_local = PROVIDER_REGISTRY
            .iter()
            .find(|d| d.id == self.provider)
            .map(|d| d.is_local)
            .unwrap_or(self.provider == "local");

        let is_fast_local = if self.fast_provider.is_empty() {
            true
        } else {
            PROVIDER_REGISTRY
                .iter()
                .find(|d| d.id == self.fast_provider)
                .map(|d| d.is_local)
                .unwrap_or(self.fast_provider == "local")
        };

        // Save remote API keys to the credentials file.
        if !is_local && !self.api_key.is_empty() {
            if let Err(e) =
                crate::setup::upsert_credential(&self.config_path, &self.provider, &self.api_key)
            {
                self.status = Some(format!("  ✗ Credentials save failed: {e}"));
                return;
            }
        }
        if !is_fast_local && !self.fast_api_key.is_empty() {
            if let Err(e) = crate::setup::upsert_credential(
                &self.config_path,
                &self.fast_provider,
                &self.fast_api_key,
            ) {
                self.status = Some(format!("  ✗ Credentials save failed: {e}"));
                return;
            }
        }

        let base_url = if self.base_url.is_empty() {
            None
        } else {
            Some(self.base_url.clone())
        };
        // API keys for all providers are stored in the credentials file (remote)
        // or not needed (local). Never put them in config.toml.
        let fast = if !self.fast_provider.is_empty() && !self.fast_model.is_empty() {
            Some(clido_core::FastProviderConfig {
                provider: self.fast_provider.clone(),
                model: self.fast_model.clone(),
                api_key: None,
                api_key_env: None,
                base_url: None,
                user_agent: None,
            })
        } else {
            None
        };
        let entry = clido_core::ProfileEntry {
            provider: self.provider.clone(),
            model: self.model.clone(),
            api_key: None,
            api_key_env: None,
            base_url,
            user_agent: None,
            fast,
        };
        match clido_core::upsert_profile_in_config(&self.config_path, &self.name, &entry) {
            Ok(()) => {
                self.status = Some(format!("  ✓ Profile '{}' saved", self.name));
            }
            Err(e) => {
                self.status = Some(format!("  ✗ Save failed: {e}"));
            }
        }
    }

    /// Masked API key for display (shows last 4 chars).
    pub(crate) fn masked_api_key(&self) -> String {
        crate::setup::anonymize_key(&self.api_key)
    }

    /// Masked fast API key for display (shows last 4 chars).
    pub(crate) fn masked_fast_api_key(&self) -> String {
        crate::setup::anonymize_key(&self.fast_api_key)
    }
}

/// Update `[profile.<profile>].model` in the config file. Preserves all other keys.
pub(crate) fn save_default_model_to_config(
    path: &std::path::Path,
    model: &str,
    profile: &str,
) -> Result<(), String> {
    if model.trim().is_empty() {
        return Ok(());
    }
    let existing = if path.exists() {
        std::fs::read_to_string(path).map_err(|e| format!("read config: {e}"))?
    } else {
        String::new()
    };
    let mut doc: toml::Value = if existing.is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&existing).map_err(|e| format!("parse config TOML: {e}"))?
    };
    if let toml::Value::Table(ref mut root) = doc {
        let profile_table = root
            .entry("profile".to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let toml::Value::Table(ref mut profiles) = profile_table {
            let entry = profiles
                .entry(profile.to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if let toml::Value::Table(ref mut prof) = entry {
                prof.insert("model".to_string(), toml::Value::String(model.to_string()));
            }
        }
    }
    let new_text = toml::to_string_pretty(&doc).map_err(|e| format!("serialize config: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    std::fs::write(path, new_text).map_err(|e| format!("write config: {e}"))?;
    Ok(())
}

// ── Per-turn tool tally (transcript summary when a turn ends) ────────────────

/// Counts tool starts in the current assistant turn for a one-line summary.
#[derive(Debug, Clone, Default)]
pub(crate) struct TurnToolTally {
    read: u32,
    write_edit: u32,
    search: u32,
    shell: u32,
    glob: u32,
    other: u32,
}

impl TurnToolTally {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn record(&mut self, name: &str) {
        match name {
            "Read" => self.read += 1,
            "Write" | "Edit" => self.write_edit += 1,
            "SemanticSearch" | "Grep" => self.search += 1,
            "Bash" => self.shell += 1,
            "Glob" => self.glob += 1,
            _ => self.other += 1,
        }
    }

    fn is_empty(&self) -> bool {
        self.read == 0
            && self.write_edit == 0
            && self.search == 0
            && self.shell == 0
            && self.glob == 0
            && self.other == 0
    }

    /// Format a single info line and reset counts.
    pub(crate) fn take_summary_line(&mut self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut push = |n: u32, one: &str, many: &str| {
            if n == 0 {
                return;
            }
            parts.push(if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {many}")
            });
        };
        push(self.read, "read", "reads");
        push(self.write_edit, "edit", "edits");
        push(self.search, "search", "searches");
        push(self.shell, "bash", "bash");
        push(self.glob, "glob", "globs");
        push(self.other, "tool", "tools");
        self.clear();
        Some(format!("  Tools: {}", parts.join(" · ")))
    }
}

// ── Status strip ─────────────────────────────────────────────────────────────

pub(crate) struct StatusEntry {
    pub(crate) tool_use_id: String,
    pub(crate) name: String,
    pub(crate) detail: String,
    pub(crate) done: bool,
    pub(crate) is_error: bool,
    pub(crate) start: std::time::Instant,
    pub(crate) elapsed_ms: Option<u64>,
}

// ── Chat lines ────────────────────────────────────────────────────────────────

pub(crate) enum ChatLine {
    User(String),
    Assistant(String),
    /// Intermediate text emitted by the model while still calling tools (dim, no label).
    Thinking(String),
    ToolCall {
        tool_use_id: String,
        name: String,
        detail: String,
        done: bool,
        is_error: bool,
    },
    Diff(String),

    Info(String),
    /// Section heading in /help output (rendered brighter than Info).
    Section(String),
    /// Welcome brand line with highlighted semicolon (compact, used for resumed sessions).
    WelcomeBrand,
    /// Full startup splash: centered panel widget (rendered directly, not as chat lines).
    WelcomeSplash,
    /// Slash command with optional text argument - displayed with command highlighted
    SlashCommand {
        cmd: String,
        text: Option<String>,
    },
}

// ── Unified Content Lines ─────────────────────────────────────────────────────

/// A single line of rendered content in the chat area.
/// This is the unified representation used for display, scrolling, and selection.
#[derive(Debug, Clone)]
pub(crate) struct ContentLine {
    /// The text content with styles
    pub spans: Vec<Span<'static>>,
    /// Where this line came from (for debugging and context)
    pub source: LineSource,
    /// Whether this line can be selected/copied
    pub selectable: bool,
    /// Original message index this line belongs to
    pub msg_idx: usize,
}

/// Source of a content line - tracks which type of message generated it
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LineSource {
    User,
    Assistant,
    Thinking,
    ToolCall,
    #[allow(dead_code)]
    ToolOutput,
    Diff,
    Info,
    Section,
    #[allow(dead_code)]
    WorkflowStep,
    #[allow(dead_code)]
    WorkflowOutput,
}

impl ContentLine {
    /// Create a new content line
    pub fn new(
        spans: Vec<Span<'static>>,
        source: LineSource,
        selectable: bool,
        msg_idx: usize,
    ) -> Self {
        Self {
            spans,
            source,
            selectable,
            msg_idx,
        }
    }

    /// Get plain text content (for selection/copy)
    #[allow(dead_code)]
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Get display width (respecting Unicode)
    #[allow(dead_code)]
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.content.width()).sum()
    }
}

// ── Wrapped Content Lines ────────────────────────────────────────────────────

/// A wrapped line represents one visible screen line after text wrapping.
/// This is what the user sees and interacts with (scroll, select, copy).
#[derive(Debug, Clone)]
pub(crate) struct WrappedLine {
    /// The text content with styles for this wrapped segment
    pub spans: Vec<Span<'static>>,
    /// Source of this line
    #[allow(dead_code)]
    pub source: LineSource,
    /// Whether this line can be selected
    pub selectable: bool,
    /// Original message index
    #[allow(dead_code)]
    pub msg_idx: usize,
    /// Which content line this wrapped segment came from
    #[allow(dead_code)]
    pub content_line_idx: usize,
    /// Character offset within the original content line
    #[allow(dead_code)]
    pub char_offset: usize,
}

impl WrappedLine {
    /// Create a new wrapped line
    pub fn new(
        spans: Vec<Span<'static>>,
        source: LineSource,
        selectable: bool,
        msg_idx: usize,
        content_line_idx: usize,
        char_offset: usize,
    ) -> Self {
        Self {
            spans,
            source,
            selectable,
            msg_idx,
            content_line_idx,
            char_offset,
        }
    }

    /// Get plain text content
    #[allow(dead_code)]
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Get display width
    #[allow(dead_code)]
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.content.width()).sum()
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

pub(crate) struct PendingPerm {
    pub(crate) tool_name: String,
    pub(crate) preview: String,
    pub(crate) reply: oneshot::Sender<PermGrant>,
}

// ── Toast notifications ────────────────────────────────────────────────────────

/// Non-blocking notification that auto-dismisses after a timeout.
pub(crate) struct Toast {
    pub(crate) message: String,
    pub(crate) style: Color,
    pub(crate) expires: std::time::Instant,
    /// Optional screen position (x, y) to anchor the toast near.
    pub(crate) position: Option<(u16, u16)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ProfileOverlayState ──────────────────────────────────────────────────

    fn make_profile_state() -> ProfileOverlayState {
        ProfileOverlayState {
            name: "test-profile".to_string(),
            provider: "anthropic".to_string(),
            api_key: "sk-test".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            base_url: String::new(),
            fast_provider: String::new(),
            fast_api_key: String::new(),
            fast_model: String::new(),
            cursor: 0,
            mode: ProfileOverlayMode::Overview,
            input: String::new(),
            input_cursor: 0,
            status: None,
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            is_new: false,
            provider_picker: ProviderPickerState::new(),
            profile_model_picker: None,
            saved_keys: vec![],
            profile_create_name_choice: ProfileCreateNameChoice::AutoGenerate,
        }
    }

    #[test]
    fn cancel_edit_from_edit_field_returns_to_overview() {
        let mut st = make_profile_state();
        st.mode = ProfileOverlayMode::EditField(ProfileEditField::Provider);
        st.input = "some-value".to_string();
        st.input_cursor = 10;
        st.cancel_edit();
        assert!(
            matches!(st.mode, ProfileOverlayMode::Overview),
            "expected Overview, got {:?}",
            st.mode
        );
        assert!(st.input.is_empty(), "cancel_edit should clear input");
        assert_eq!(st.input_cursor, 0);
    }

    #[test]
    fn cancel_edit_from_picking_provider_returns_to_overview() {
        let mut st = make_profile_state();
        st.mode = ProfileOverlayMode::PickingProvider {
            for_field: ProfileEditField::Provider,
        };
        st.cancel_edit();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
    }

    #[test]
    fn cancel_edit_from_picking_model_returns_to_overview() {
        let mut st = make_profile_state();
        st.mode = ProfileOverlayMode::PickingModel {
            for_field: ProfileEditField::Model,
        };
        st.cancel_edit();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
    }

    #[test]
    fn commit_edit_api_key_updates_field_and_returns_to_overview() {
        let mut st = make_profile_state();
        // cursor 1 = ApiKey
        st.cursor = 1;
        st.mode = ProfileOverlayMode::EditField(ProfileEditField::ApiKey);
        st.input = "  new-api-key  ".to_string();
        st.input_cursor = st.input.chars().count();
        st.commit_edit();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
        assert_eq!(st.api_key, "new-api-key", "commit_edit should trim input");
        assert!(
            st.input.is_empty(),
            "commit_edit should clear staging buffer"
        );
    }

    #[test]
    fn commit_edit_model_field_updates_model() {
        let mut st = make_profile_state();
        st.cursor = 2;
        st.mode = ProfileOverlayMode::EditField(ProfileEditField::Model);
        st.input = "claude-opus-4".to_string();
        st.input_cursor = st.input.chars().count();
        st.commit_edit();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
        assert_eq!(st.model, "claude-opus-4");
    }

    #[test]
    fn commit_edit_base_url_field_updates_base_url() {
        let mut st = make_profile_state();
        st.cursor = 3;
        st.mode = ProfileOverlayMode::EditField(ProfileEditField::BaseUrl);
        st.input = "https://my-endpoint.example.com".to_string();
        st.input_cursor = st.input.chars().count();
        st.commit_edit();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
        assert_eq!(st.base_url, "https://my-endpoint.example.com");
    }

    #[test]
    fn commit_edit_fast_api_key_updates_fast_api_key() {
        let mut st = make_profile_state();
        st.mode = ProfileOverlayMode::EditField(ProfileEditField::FastApiKey);
        st.input = "fast-key-xyz".to_string();
        st.input_cursor = st.input.chars().count();
        st.commit_edit();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
        assert_eq!(st.fast_api_key, "fast-key-xyz");
    }

    #[test]
    fn commit_edit_noop_when_not_in_edit_mode() {
        let mut st = make_profile_state();
        // mode is Overview — commit_edit should not crash and should stay Overview
        st.commit_edit();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
    }

    #[test]
    fn begin_edit_on_api_key_cursor_enters_edit_field_mode() {
        let mut st = make_profile_state();
        st.cursor = 1; // ApiKey
        st.begin_edit(&[]);
        assert!(
            matches!(
                st.mode,
                ProfileOverlayMode::EditField(ProfileEditField::ApiKey)
            ),
            "expected EditField(ApiKey), got {:?}",
            st.mode
        );
        // staging buffer should be pre-filled with current value
        assert_eq!(st.input, "sk-test");
    }

    #[test]
    fn begin_edit_on_provider_cursor_enters_picking_provider_mode() {
        let mut st = make_profile_state();
        st.cursor = 0; // Provider
        st.begin_edit(&[]);
        assert!(
            matches!(
                st.mode,
                ProfileOverlayMode::PickingProvider {
                    for_field: ProfileEditField::Provider
                }
            ),
            "expected PickingProvider, got {:?}",
            st.mode
        );
    }

    #[test]
    fn begin_edit_on_model_cursor_enters_picking_model_mode() {
        let mut st = make_profile_state();
        st.cursor = 2; // Model
        st.begin_edit(&[]);
        assert!(
            matches!(
                st.mode,
                ProfileOverlayMode::PickingModel {
                    for_field: ProfileEditField::Model
                }
            ),
            "expected PickingModel, got {:?}",
            st.mode
        );
    }

    #[test]
    fn commit_provider_pick_with_no_key_required_returns_to_overview() {
        let mut st = make_profile_state();
        // Set up picking provider for FastProvider (ollama doesn't need a key)
        st.mode = ProfileOverlayMode::PickingProvider {
            for_field: ProfileEditField::FastProvider,
        };
        // Select "ollama" which doesn't require a key
        let ollama_pos = KNOWN_PROVIDERS
            .iter()
            .position(|(id, _, _)| *id == "ollama");
        if let Some(pos) = ollama_pos {
            // Find the filtered index for ollama
            let filtered = st.provider_picker.filtered();
            if let Some(fi) = filtered.iter().position(|&i| i == pos) {
                st.provider_picker.selected = fi;
            }
        }
        st.commit_provider_pick();
        assert!(
            matches!(st.mode, ProfileOverlayMode::Overview),
            "expected Overview after picking provider that doesn't need key, got {:?}",
            st.mode
        );
        assert_eq!(st.fast_provider, "ollama");
    }

    #[test]
    fn commit_provider_pick_with_key_required_and_no_key_enters_edit_field() {
        let mut st = make_profile_state();
        // Clear out the api_key so the logic triggers prompt-for-key
        st.api_key = String::new();
        st.mode = ProfileOverlayMode::PickingProvider {
            for_field: ProfileEditField::Provider,
        };
        // Select "openai" which requires a key
        let openai_pos = KNOWN_PROVIDERS
            .iter()
            .position(|(id, _, _)| *id == "openai");
        if let Some(pos) = openai_pos {
            let filtered = st.provider_picker.filtered();
            if let Some(fi) = filtered.iter().position(|&i| i == pos) {
                st.provider_picker.selected = fi;
            }
        }
        st.commit_provider_pick();
        // Should enter EditField(ApiKey) to prompt user for the key
        assert!(
            matches!(
                st.mode,
                ProfileOverlayMode::EditField(ProfileEditField::ApiKey)
            ),
            "expected EditField(ApiKey) when key is required and missing, got {:?}",
            st.mode
        );
    }

    #[test]
    fn commit_provider_pick_with_existing_key_returns_to_overview() {
        let mut st = make_profile_state();
        // api_key is already set ("sk-test")
        st.mode = ProfileOverlayMode::PickingProvider {
            for_field: ProfileEditField::Provider,
        };
        // Select "anthropic"
        let pos = KNOWN_PROVIDERS
            .iter()
            .position(|(id, _, _)| *id == "anthropic");
        if let Some(pos) = pos {
            let filtered = st.provider_picker.filtered();
            if let Some(fi) = filtered.iter().position(|&i| i == pos) {
                st.provider_picker.selected = fi;
            }
        }
        st.commit_provider_pick();
        assert!(
            matches!(st.mode, ProfileOverlayMode::Overview),
            "expected Overview when key already present, got {:?}",
            st.mode
        );
        assert_eq!(st.provider, "anthropic");
    }

    #[test]
    fn commit_model_pick_with_selection_updates_model_and_returns_to_overview() {
        let mut st = make_profile_state();
        st.mode = ProfileOverlayMode::PickingModel {
            for_field: ProfileEditField::Model,
        };
        st.profile_model_picker = Some(ModelPickerState {
            models: vec![ModelEntry {
                id: "claude-opus-4".to_string(),
                provider: "anthropic".to_string(),
                input_mtok: 0.0,
                output_mtok: 0.0,
                context_k: None,
                role: None,
                is_favorite: false,
            }],
            filter: String::new(),
            selected: 0,
            scroll_offset: 0,
        });
        st.commit_model_pick();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
        assert_eq!(st.model, "claude-opus-4");
        assert!(st.profile_model_picker.is_none());
    }

    #[test]
    fn commit_model_pick_for_fast_model_updates_fast_model() {
        let mut st = make_profile_state();
        st.mode = ProfileOverlayMode::PickingModel {
            for_field: ProfileEditField::FastModel,
        };
        st.profile_model_picker = Some(ModelPickerState {
            models: vec![ModelEntry {
                id: "claude-haiku-3-5".to_string(),
                provider: "anthropic".to_string(),
                input_mtok: 0.0,
                output_mtok: 0.0,
                context_k: None,
                role: None,
                is_favorite: false,
            }],
            filter: String::new(),
            selected: 0,
            scroll_offset: 0,
        });
        st.commit_model_pick();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
        assert_eq!(st.fast_model, "claude-haiku-3-5");
    }

    #[test]
    fn commit_model_pick_with_no_picker_returns_to_overview() {
        let mut st = make_profile_state();
        st.mode = ProfileOverlayMode::PickingModel {
            for_field: ProfileEditField::Model,
        };
        st.profile_model_picker = None;
        let original_model = st.model.clone();
        st.commit_model_pick();
        assert!(matches!(st.mode, ProfileOverlayMode::Overview));
        assert_eq!(
            st.model, original_model,
            "model should not change when picker is None"
        );
    }

    #[test]
    fn for_create_starts_in_creating_name_step_when_no_env_detected() {
        // Unset any provider env vars that might be present in test environment
        // We can't guarantee env is clean, so just verify shape when provider is empty.
        // Call for_create with an empty profiles map.
        let path = std::path::PathBuf::from("/tmp/test-config.toml");
        let profiles = std::collections::HashMap::new();
        let st = ProfileOverlayState::for_create(path, &profiles);
        assert!(st.is_new);
        // Mode is either Creating{Name} (no env) or Creating{Model} (env detected)
        // Both are valid; just assert it's a Creating variant.
        assert!(
            matches!(st.mode, ProfileOverlayMode::Creating { .. }),
            "expected Creating variant, got {:?}",
            st.mode
        );
    }

    #[test]
    fn cursor_field_mapping_covers_all_positions() {
        let st = make_profile_state();
        assert_eq!(
            ProfileOverlayState {
                cursor: 0,
                ..make_profile_state()
            }
            .cursor_field(),
            ProfileEditField::Provider
        );
        assert_eq!(
            ProfileOverlayState {
                cursor: 1,
                ..make_profile_state()
            }
            .cursor_field(),
            ProfileEditField::ApiKey
        );
        assert_eq!(
            ProfileOverlayState {
                cursor: 2,
                ..make_profile_state()
            }
            .cursor_field(),
            ProfileEditField::Model
        );
        assert_eq!(
            ProfileOverlayState {
                cursor: 3,
                ..make_profile_state()
            }
            .cursor_field(),
            ProfileEditField::BaseUrl
        );
        assert_eq!(
            ProfileOverlayState {
                cursor: 4,
                ..make_profile_state()
            }
            .cursor_field(),
            ProfileEditField::FastProvider
        );
        assert_eq!(
            ProfileOverlayState {
                cursor: 5,
                ..make_profile_state()
            }
            .cursor_field(),
            ProfileEditField::FastApiKey
        );
        assert_eq!(
            ProfileOverlayState {
                cursor: 6,
                ..make_profile_state()
            }
            .cursor_field(),
            ProfileEditField::FastModel
        );
        // Out-of-range → None
        assert_eq!(
            ProfileOverlayState {
                cursor: 99,
                ..make_profile_state()
            }
            .cursor_field(),
            ProfileEditField::None
        );
        let _ = st; // suppress unused warning
    }

    #[test]
    fn field_count_is_seven() {
        assert_eq!(ProfileOverlayState::field_count(), 7);
    }

    #[test]
    fn field_value_returns_correct_value_for_each_field() {
        let st = ProfileOverlayState {
            provider: "openai".to_string(),
            api_key: "key-abc".to_string(),
            model: "gpt-4o".to_string(),
            base_url: "https://api.example.com".to_string(),
            fast_provider: "groq".to_string(),
            fast_api_key: "fast-key".to_string(),
            fast_model: "llama-3".to_string(),
            ..make_profile_state()
        };
        assert_eq!(st.field_value(&ProfileEditField::Provider), "openai");
        assert_eq!(st.field_value(&ProfileEditField::ApiKey), "key-abc");
        assert_eq!(st.field_value(&ProfileEditField::Model), "gpt-4o");
        assert_eq!(
            st.field_value(&ProfileEditField::BaseUrl),
            "https://api.example.com"
        );
        assert_eq!(st.field_value(&ProfileEditField::FastProvider), "groq");
        assert_eq!(st.field_value(&ProfileEditField::FastApiKey), "fast-key");
        assert_eq!(st.field_value(&ProfileEditField::FastModel), "llama-3");
        assert_eq!(st.field_value(&ProfileEditField::None), "");
    }

    // ── SessionStats ─────────────────────────────────────────────────────────

    #[test]
    fn session_stats_default_all_zeros() {
        let stats = SessionStats::default();
        assert_eq!(stats.session_input_tokens, 0);
        assert_eq!(stats.session_output_tokens, 0);
        assert_eq!(stats.session_cost_usd, 0.0);
        assert_eq!(stats.session_total_input_tokens, 0);
        assert_eq!(stats.session_total_output_tokens, 0);
        assert_eq!(stats.session_total_cost_usd, 0.0);
        assert_eq!(stats.session_turn_count, 0);
    }

    #[test]
    fn turn_tool_tally_summary_joins_categories_and_clears() {
        let mut t = TurnToolTally::default();
        t.record("Read");
        t.record("Read");
        t.record("SemanticSearch");
        t.record("Bash");
        let line = t.take_summary_line().expect("summary");
        assert!(line.contains("2 reads"));
        assert!(line.contains("1 search"));
        assert!(line.contains("1 bash"));
        assert!(t.take_summary_line().is_none());
    }

    // ── PlanState ────────────────────────────────────────────────────────────

    #[test]
    fn plan_state_default_all_none_false_zero() {
        let plan = PlanState::default();
        assert!(plan.last_plan_snapshot.is_none());
        assert!(plan.last_plan.is_none());
        assert!(plan.last_plan_raw.is_none());
        assert!(!plan.awaiting_plan_response);
        assert!(plan.editor.is_none());
        assert_eq!(plan.selected_task, 0);
        assert!(plan.task_editing.is_none());
        assert!(plan.text_editor.is_none());
    }

    // ── PlanTextEditor ───────────────────────────────────────────────────────

    #[test]
    fn plan_text_editor_from_raw_splits_lines() {
        let editor = PlanTextEditor::from_raw("line one\nline two\nline three");
        assert_eq!(editor.lines.len(), 3);
        assert_eq!(editor.lines[0], "line one");
        assert_eq!(editor.lines[2], "line three");
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 0);
        assert_eq!(editor.scroll, 0);
    }

    #[test]
    fn plan_text_editor_clamp_col_limits_cursor() {
        let mut editor = PlanTextEditor::from_raw("short\nlonger line");
        editor.cursor_row = 0;
        editor.cursor_col = 100;
        editor.clamp_col();
        assert_eq!(editor.cursor_col, 5); // "short" is 5 chars
    }

    // ── TaskEditState ────────────────────────────────────────────────────────

    #[test]
    fn task_edit_state_defaults_to_description_field() {
        let state = TaskEditState::new("t1", "Fix bug", "some notes", Complexity::Medium);
        assert_eq!(state.task_id, "t1");
        assert_eq!(state.description, "Fix bug");
        assert_eq!(state.notes, "some notes");
        assert_eq!(state.complexity, Complexity::Medium);
        assert_eq!(state.focused_field, TaskEditField::Description);
    }

    // ── FocusTarget ─────────────────────────────────────────────────────────

    #[test]
    fn focus_target_chat_input_is_not_modal() {
        assert!(!FocusTarget::ChatInput.is_modal());
    }

    #[test]
    fn focus_target_all_others_are_modal() {
        assert!(FocusTarget::PlanTextEditor.is_modal());
        assert!(FocusTarget::PlanEditor.is_modal());
        assert!(FocusTarget::ProfileOverlay.is_modal());
        assert!(FocusTarget::WorkflowEditor.is_modal());
        assert!(FocusTarget::Overlay.is_modal());
        assert!(FocusTarget::ModelPicker.is_modal());
        assert!(FocusTarget::SessionPicker.is_modal());
        assert!(FocusTarget::ProfilePicker.is_modal());
        assert!(FocusTarget::Permission.is_modal());
    }
}
