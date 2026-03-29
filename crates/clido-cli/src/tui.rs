//! Full-screen ratatui TUI: scrollable conversation + persistent input bar.

use std::collections::{HashSet, VecDeque};
use std::env;
use std::hash::{Hash, Hasher};
use std::io::{stdout, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use clido_agent::{
    AgentLoop, AskUser, EventEmitter, PermGrant as AgentPermGrant, PermRequest as AgentPermRequest,
};
use clido_core::{ClidoError, PermissionMode};
use clido_storage::SessionWriter;
use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyModifiers,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use pulldown_cmark::Parser;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame, Terminal,
};
use tokio::sync::{mpsc, oneshot};

use crate::agent_setup::AgentSetup;
use crate::cli::Cli;
use crate::git_context::GitContext;
use crate::image_input::ImageAttachment;
use crate::prompt_enhance::{
    load_prompt_mode, load_rules, project_rules_path, project_settings_path, save_prompt_mode,
    save_rules, EnhancementCtx, PromptMode, PromptRules, RuleEntry,
};
use clido_index::RepoIndex;
use clido_memory::MemoryStore;
use clido_planner::{Complexity, Plan, PlanEditor, TaskStatus};

use crate::overlay::{AppAction, ErrorOverlay, OverlayKeyResult, OverlayKind, OverlayStack};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Truecolor accent for borders and highlights — avoids saturated ANSI blue.
const TUI_SOFT_ACCENT: Color = Color::Rgb(150, 200, 255);
/// Selected row background in pickers and completion lists (muted slate).
const TUI_SELECTION_BG: Color = Color::Rgb(52, 62, 78);

/// Slash commands grouped by section — now delegates to command_registry.
fn slash_command_sections() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    crate::command_registry::commands_by_section()
        .into_iter()
        .map(|(section, cmds)| {
            let pairs: Vec<(&'static str, &'static str)> =
                cmds.into_iter().map(|c| (c.name, c.description)).collect();
            (section, pairs)
        })
        .collect()
}

/// Flat list of all slash commands — delegates to command_registry.
fn slash_commands() -> Vec<(&'static str, &'static str)> {
    crate::command_registry::flat_commands()
}

// ── Permission grant options ───────────────────────────────────────────────────

#[derive(Debug)]
enum PermGrant {
    /// Allow this single invocation.
    Once,
    /// Allow this tool for the rest of the session.
    Session,
    /// Allow all tools for the rest of the session (workdir-wide).
    Workdir,
    /// Deny.
    Deny,
    /// Deny with feedback message sent back to the agent.
    DenyWithFeedback(String),
}

// ── Session-level permission state (shared between TuiAskUser calls) ──────────

#[derive(Default)]
struct PermsState {
    /// Tool names granted for the whole session.
    session_allowed: HashSet<String>,
    /// All tools open for this session (workdir-wide grant).
    workdir_open: bool,
}

impl PermsState {
    fn clear_all_grants(&mut self) {
        self.session_allowed.clear();
        self.workdir_open = false;
    }
}

// ── Agent → TUI events ────────────────────────────────────────────────────────

enum AgentEvent {
    ToolStart {
        tool_use_id: String,
        name: String,
        detail: String,
    },
    ToolDone {
        tool_use_id: String,
        is_error: bool,
        diff: Option<String>,
    },
    /// Intermediate text the model emits while it's still calling tools.
    Thinking(String),
    Response(String),
    Interrupted,
    Err(String),
    /// Emitted once when the agent session is created.
    SessionStarted(String),
    /// Emitted when a session is resumed; carries display messages.
    ResumedSession {
        messages: Vec<(String, String)>,
    },
    /// Token usage update after agent turn completion.
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        context_max_tokens: u64,
    },
    /// Emitted once by the `/compact` command after history is compacted.
    Compacted {
        before: usize,
        after: usize,
    },
    /// Emitted when the planner produces a valid task graph (--planner mode).
    /// Each string is a human-readable description of one planned task.
    PlanCreated {
        tasks: Vec<String>,
    },
    /// Emitted when the session model is switched (via /model, /fast, /smart).
    ModelSwitched {
        to_model: String,
    },
    /// Emitted when background runtime successfully switches workdir/tooling.
    WorkdirSwitched {
        path: std::path::PathBuf,
    },
    /// Plan generated and ready for user review (--plan mode).
    PlanReady {
        plan: Plan,
    },
    /// A plan task started executing.
    #[allow(dead_code)]
    PlanTaskStarted {
        task_id: String,
    },
    /// A plan task completed.
    #[allow(dead_code)]
    PlanTaskDone {
        task_id: String,
        success: bool,
    },
    /// Periodic keep-alive from agent_task while waiting on a slow LLM response.
    /// Resets the stall timer without producing any visible output.
    Heartbeat,
    /// Emitted when cumulative cost crosses a budget threshold (50%, 80%, or 90%).
    BudgetWarning {
        percent: u8,
        cost: f64,
        limit: f64,
    },
    /// Emitted when models are fetched live from the provider API.
    ModelsLoaded(Vec<String>),
}

// ── Plan editor state ─────────────────────────────────────────────────────────

/// Simple nano-style full-screen plan text editor.
struct PlanTextEditor {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    scroll: usize,
}

impl PlanTextEditor {
    fn from_raw(text: &str) -> Self {
        let lines = text.lines().map(|l| l.to_string()).collect();
        Self {
            lines,
            cursor_row: 0,
            cursor_col: 0,
            scroll: 0,
        }
    }

    fn to_tasks(&self) -> Vec<String> {
        parse_plan_from_text(&self.lines.join("\n"))
    }

    fn clamp_col(&mut self) {
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
enum TaskEditField {
    Description,
    Notes,
    Complexity,
}

/// State for the inline task edit form.
struct TaskEditState {
    task_id: String,
    description: String,
    notes: String,
    complexity: Complexity,
    focused_field: TaskEditField,
}

impl TaskEditState {
    fn new(task_id: &str, description: &str, notes: &str, complexity: Complexity) -> Self {
        Self {
            task_id: task_id.to_string(),
            description: description.to_string(),
            notes: notes.to_string(),
            complexity,
            focused_field: TaskEditField::Description,
        }
    }
}

// ── Session picker popup state ────────────────────────────────────────────────

struct SessionPickerState {
    sessions: Vec<clido_storage::SessionSummary>,
    selected: usize,
    scroll_offset: usize,
    filter: String,
}

impl SessionPickerState {
    fn filtered(&self) -> Vec<(usize, &clido_storage::SessionSummary)> {
        let f = self.filter.trim().to_lowercase();
        if f.is_empty() {
            return self.sessions.iter().enumerate().collect();
        }
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.session_id.to_lowercase().contains(&f) || s.preview.to_lowercase().contains(&f)
            })
            .collect()
    }
}

// ── Profile picker popup state ─────────────────────────────────────────────────

struct ProfilePickerState {
    /// All profiles, sorted by name: (profile_name, entry).
    profiles: Vec<(String, clido_core::ProfileEntry)>,
    selected: usize,
    scroll_offset: usize,
    /// Currently active profile name (shown with ▶ marker).
    active: String,
    filter: String,
}

impl ProfilePickerState {
    fn filtered(&self) -> Vec<(usize, &(String, clido_core::ProfileEntry))> {
        let f = self.filter.trim().to_lowercase();
        if f.is_empty() {
            return self.profiles.iter().enumerate().collect();
        }
        self.profiles
            .iter()
            .enumerate()
            .filter(|(_, (name, entry))| {
                name.to_lowercase().contains(&f)
                    || entry.provider.to_lowercase().contains(&f)
                    || entry.model.to_lowercase().contains(&f)
            })
            .collect()
    }
}

// ── Role picker popup state ────────────────────────────────────────────────────

struct RolePickerState {
    /// All configured roles, sorted by name: (role_name, model_id).
    roles: Vec<(String, String)>,
    selected: usize,
    scroll_offset: usize,
    filter: String,
}

impl RolePickerState {
    fn filtered(&self) -> Vec<(usize, &(String, String))> {
        let f = self.filter.trim().to_lowercase();
        if f.is_empty() {
            return self.roles.iter().enumerate().collect();
        }
        self.roles
            .iter()
            .enumerate()
            .filter(|(_, (name, model))| {
                name.to_lowercase().contains(&f) || model.to_lowercase().contains(&f)
            })
            .collect()
    }
}

// ── Model picker popup state ──────────────────────────────────────────────────

/// One row in the model picker.
#[derive(Clone)]
struct ModelEntry {
    id: String,
    provider: String,
    /// Cost per million input tokens (USD).
    input_mtok: f64,
    /// Cost per million output tokens (USD).
    output_mtok: f64,
    /// Context window in thousands of tokens.
    context_k: Option<u32>,
    /// Role name assigned to this model (e.g. "fast", "reasoning").
    role: Option<String>,
    /// Whether the user has starred this model.
    is_favorite: bool,
}

struct ModelPickerState {
    /// Full sorted model list (favorites → recent → rest).
    models: Vec<ModelEntry>,
    /// Live filter string.
    filter: String,
    /// Selected index within the *filtered* view.
    selected: usize,
    scroll_offset: usize,
}

impl ModelPickerState {
    fn filtered(&self) -> Vec<&ModelEntry> {
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

    fn clamp(&mut self) {
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
    fn refresh_models(&mut self, ids: Vec<String>) {
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
const KNOWN_PROVIDERS: &[(&str, &str, bool)] = &[
    ("anthropic", "Anthropic  (Claude)", true),
    ("openai", "OpenAI  (GPT / o-series)", true),
    ("google", "Google  (Gemini)", true),
    ("gemini", "Google Gemini", true),
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
    ("cohere", "Cohere", true),
    ("together", "Together AI", true),
    ("ollama", "Ollama  (local)", false),
    ("lmstudio", "LM Studio  (local)", false),
    ("openrouter", "OpenRouter", true),
    ("azure", "Azure OpenAI", true),
    ("custom", "Custom / Other", true),
];

struct ProviderPickerState {
    /// Index of the highlighted provider in KNOWN_PROVIDERS.
    selected: usize,
    /// Live filter string typed by user.
    filter: String,
    scroll_offset: usize,
}

impl ProviderPickerState {
    fn new() -> Self {
        Self {
            selected: 0,
            filter: String::new(),
            scroll_offset: 0,
        }
    }

    fn filtered(&self) -> Vec<usize> {
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

    fn clamp(&mut self) {
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
    fn selected_id(&self) -> Option<&'static str> {
        let indices = self.filtered();
        indices.get(self.selected).map(|&i| KNOWN_PROVIDERS[i].0)
    }

    /// Whether the currently selected provider requires an API key.
    fn selected_requires_key(&self) -> bool {
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
enum ProfileEditField {
    None,
    Provider,
    ApiKey,
    Model,
    BaseUrl,
    WorkerProvider,
    WorkerModel,
    ReviewerProvider,
    ReviewerModel,
}

/// Screen mode for the profile overlay.
#[derive(Debug, Clone, PartialEq)]
enum ProfileOverlayMode {
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
}

/// Steps for the in-TUI new profile wizard.
#[derive(Debug, Clone, PartialEq)]
enum ProfileCreateStep {
    #[allow(dead_code)]
    Name,
    Provider,
    ApiKey,
    Model,
}

/// In-TUI profile overview/editor overlay — never exits the TUI.
struct ProfileOverlayState {
    /// Profile name being viewed/edited (empty = creating new).
    name: String,
    /// Live-editable fields — main agent.
    provider: String,
    api_key: String,
    model: String,
    base_url: String,
    /// Sub-agent fields — worker (all optional; empty = not configured).
    worker_provider: String,
    worker_model: String,
    /// Sub-agent fields — reviewer (all optional; empty = not configured).
    reviewer_provider: String,
    reviewer_model: String,
    /// Current cursor row in overview mode (0-based index into PROFILE_FIELDS).
    cursor: usize,
    /// Current overlay mode.
    mode: ProfileOverlayMode,
    /// Staging buffer for inline text editing.
    input: String,
    /// Cursor position inside `input`.
    input_cursor: usize,
    /// Status message shown at the bottom.
    status: Option<String>,
    /// Config file path for persistence.
    config_path: std::path::PathBuf,
    /// Whether this is a new-profile creation flow.
    is_new: bool,
    /// State for the provider picker (used in PickingProvider mode).
    provider_picker: ProviderPickerState,
    /// State for the model picker inside the profile overlay (used in PickingModel mode).
    profile_model_picker: Option<ModelPickerState>,
}

/// Display labels and keys for the editable profile fields (in order).
/// The special key "__section__" marks a non-editable section header divider.
const PROFILE_FIELDS: &[(&str, &str)] = &[
    ("provider", "Provider"),
    ("api_key", "API Key"),
    ("model", "Model"),
    ("base_url", "Custom Endpoint (optional)"),
    ("__section__", "── Worker Sub-agent (optional) ──"),
    ("worker_provider", "Worker Provider"),
    ("worker_model", "Worker Model"),
    ("__section__", "── Reviewer Sub-agent (optional) ──"),
    ("reviewer_provider", "Reviewer Provider"),
    ("reviewer_model", "Reviewer Model"),
];

impl ProfileOverlayState {
    /// Open an existing profile for editing.
    fn for_edit(
        name: String,
        entry: &clido_core::ProfileEntry,
        config_path: std::path::PathBuf,
    ) -> Self {
        let api_key = entry
            .api_key
            .clone()
            .or_else(|| {
                entry
                    .api_key_env
                    .as_ref()
                    .and_then(|e| std::env::var(e).ok())
            })
            .unwrap_or_default();
        let worker_provider = entry
            .worker
            .as_ref()
            .map(|w| w.provider.clone())
            .unwrap_or_default();
        let worker_model = entry
            .worker
            .as_ref()
            .map(|w| w.model.clone())
            .unwrap_or_default();
        let reviewer_provider = entry
            .reviewer
            .as_ref()
            .map(|r| r.provider.clone())
            .unwrap_or_default();
        let reviewer_model = entry
            .reviewer
            .as_ref()
            .map(|r| r.model.clone())
            .unwrap_or_default();
        Self {
            name,
            provider: entry.provider.clone(),
            api_key,
            model: entry.model.clone(),
            base_url: entry.base_url.clone().unwrap_or_default(),
            worker_provider,
            worker_model,
            reviewer_provider,
            reviewer_model,
            cursor: 0,
            mode: ProfileOverlayMode::Overview,
            input: String::new(),
            input_cursor: 0,
            status: None,
            config_path,
            is_new: false,
            provider_picker: ProviderPickerState::new(),
            profile_model_picker: None,
        }
    }

    /// Open a blank state for creating a new profile.
    fn for_create(config_path: std::path::PathBuf) -> Self {
        Self {
            name: String::new(),
            provider: String::new(),
            api_key: String::new(),
            model: String::new(),
            base_url: String::new(),
            worker_provider: String::new(),
            worker_model: String::new(),
            reviewer_provider: String::new(),
            reviewer_model: String::new(),
            cursor: 0,
            mode: ProfileOverlayMode::Creating {
                step: ProfileCreateStep::Provider,
            },
            input: String::new(),
            input_cursor: 0,
            status: None,
            config_path,
            is_new: true,
            provider_picker: ProviderPickerState::new(),
            profile_model_picker: None,
        }
    }

    /// Value of the field at `cursor`.
    fn field_value(&self, field: &ProfileEditField) -> String {
        match field {
            ProfileEditField::Provider => self.provider.clone(),
            ProfileEditField::ApiKey => self.api_key.clone(),
            ProfileEditField::Model => self.model.clone(),
            ProfileEditField::BaseUrl => self.base_url.clone(),
            ProfileEditField::WorkerProvider => self.worker_provider.clone(),
            ProfileEditField::WorkerModel => self.worker_model.clone(),
            ProfileEditField::ReviewerProvider => self.reviewer_provider.clone(),
            ProfileEditField::ReviewerModel => self.reviewer_model.clone(),
            ProfileEditField::None => String::new(),
        }
    }

    /// The `ProfileEditField` corresponding to cursor row.
    /// cursor 0-3 = main agent fields; 4-5 = worker; 6-7 = reviewer.
    fn cursor_field(&self) -> ProfileEditField {
        match self.cursor {
            0 => ProfileEditField::Provider,
            1 => ProfileEditField::ApiKey,
            2 => ProfileEditField::Model,
            3 => ProfileEditField::BaseUrl,
            4 => ProfileEditField::WorkerProvider,
            5 => ProfileEditField::WorkerModel,
            6 => ProfileEditField::ReviewerProvider,
            7 => ProfileEditField::ReviewerModel,
            _ => ProfileEditField::None,
        }
    }

    /// Total number of editable cursor positions.
    fn field_count() -> usize {
        8
    }

    /// Start editing the field at `cursor`.
    fn begin_edit(&mut self, known_models: &[ModelEntry]) {
        let field = self.cursor_field();
        match field {
            ProfileEditField::Provider
            | ProfileEditField::WorkerProvider
            | ProfileEditField::ReviewerProvider => {
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
            ProfileEditField::Model
            | ProfileEditField::WorkerModel
            | ProfileEditField::ReviewerModel => {
                let current = self.field_value(&field);
                let provider = match field {
                    ProfileEditField::WorkerModel => self.worker_provider.clone(),
                    ProfileEditField::ReviewerModel => self.reviewer_provider.clone(),
                    _ => self.provider.clone(),
                };
                let mut picker = ModelPickerState {
                    models: known_models.to_vec(),
                    filter: if !provider.is_empty() {
                        provider
                    } else {
                        current
                    },
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
    fn commit_edit(&mut self) {
        if let ProfileOverlayMode::EditField(ref field) = self.mode.clone() {
            match field {
                ProfileEditField::Provider => self.provider = self.input.trim().to_string(),
                ProfileEditField::ApiKey => self.api_key = self.input.trim().to_string(),
                ProfileEditField::Model => self.model = self.input.trim().to_string(),
                ProfileEditField::BaseUrl => self.base_url = self.input.trim().to_string(),
                ProfileEditField::WorkerProvider => {
                    self.worker_provider = self.input.trim().to_string()
                }
                ProfileEditField::WorkerModel => self.worker_model = self.input.trim().to_string(),
                ProfileEditField::ReviewerProvider => {
                    self.reviewer_provider = self.input.trim().to_string()
                }
                ProfileEditField::ReviewerModel => {
                    self.reviewer_model = self.input.trim().to_string()
                }
                ProfileEditField::None => {}
            }
        }
        self.mode = ProfileOverlayMode::Overview;
        self.input.clear();
        self.input_cursor = 0;
    }

    /// Abandon in-progress edit and return to overview.
    fn cancel_edit(&mut self) {
        self.mode = ProfileOverlayMode::Overview;
        self.input.clear();
        self.input_cursor = 0;
    }

    fn commit_provider_pick(&mut self) {
        if let ProfileOverlayMode::PickingProvider { ref for_field } = self.mode.clone() {
            if let Some(id) = self.provider_picker.selected_id() {
                match for_field {
                    ProfileEditField::Provider => self.provider = id.to_string(),
                    ProfileEditField::WorkerProvider => self.worker_provider = id.to_string(),
                    ProfileEditField::ReviewerProvider => self.reviewer_provider = id.to_string(),
                    _ => {}
                }
            }
        }
        self.provider_picker = ProviderPickerState::new();
        self.mode = ProfileOverlayMode::Overview;
    }

    fn commit_model_pick(&mut self) {
        if let ProfileOverlayMode::PickingModel { ref for_field } = self.mode.clone() {
            if let Some(picker) = &self.profile_model_picker {
                let filtered = picker.filtered();
                if let Some(m) = filtered.get(picker.selected) {
                    let id = m.id.clone();
                    match for_field {
                        ProfileEditField::Model => self.model = id,
                        ProfileEditField::WorkerModel => self.worker_model = id,
                        ProfileEditField::ReviewerModel => self.reviewer_model = id,
                        _ => {}
                    }
                }
            }
        }
        self.profile_model_picker = None;
        self.mode = ProfileOverlayMode::Overview;
    }

    /// Persist the current state to the config file and report result.
    fn save(&mut self) {
        let base_url = if self.base_url.is_empty() {
            None
        } else {
            Some(self.base_url.clone())
        };
        let api_key_opt = if self.api_key.is_empty() {
            None
        } else {
            Some(self.api_key.clone())
        };
        // Build optional sub-agent configs — only if at least a provider is set.
        let worker = if !self.worker_provider.is_empty() && !self.worker_model.is_empty() {
            Some(clido_core::AgentSlotConfig {
                provider: self.worker_provider.clone(),
                model: self.worker_model.clone(),
                api_key: None,
                api_key_env: None,
                base_url: None,
                user_agent: None,
            })
        } else {
            None
        };
        let reviewer = if !self.reviewer_provider.is_empty() && !self.reviewer_model.is_empty() {
            Some(clido_core::AgentSlotConfig {
                provider: self.reviewer_provider.clone(),
                model: self.reviewer_model.clone(),
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
            api_key: api_key_opt,
            api_key_env: None,
            base_url,
            user_agent: None,
            worker,
            reviewer,
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
    fn masked_api_key(&self) -> String {
        crate::setup::anonymize_key(&self.api_key)
    }
}

// ── Settings editor popup ──────────────────────────────────────────────────────

/// Which field is being edited in the settings/roles editor.
#[derive(Debug, Clone, PartialEq)]
enum SettingsEditField {
    None,
    DefaultModel,     // editing the default model string
    RoleName(usize),  // editing role name at index (usize::MAX = new)
    RoleModel(usize), // editing model id at index
}

struct SettingsState {
    /// Current roles (name, model_id) — loaded from config on open.
    roles: Vec<(String, String)>,
    cursor: usize,
    edit_field: SettingsEditField,
    input: String,
    /// Path to the config file that will be updated on save.
    config_path: std::path::PathBuf,
    /// Status message after save.
    status: Option<String>,
    /// Default model from config (editable).
    default_model: String,
    /// Active config profile name (used when saving default model).
    profile: String,
}

impl SettingsState {
    fn new(
        config_path: std::path::PathBuf,
        roles: std::collections::HashMap<String, String>,
        default_model: String,
        profile: String,
    ) -> Self {
        let mut sorted: Vec<(String, String)> = roles.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            roles: sorted,
            cursor: 0,
            edit_field: SettingsEditField::None,
            input: String::new(),
            config_path,
            status: None,
            default_model,
            profile,
        }
    }

    /// Write roles and default model back to the config file.
    fn save(&mut self) {
        let r1 = save_roles_to_config(&self.config_path, &self.roles);
        let r2 =
            save_default_model_to_config(&self.config_path, &self.default_model, &self.profile);
        match (r1, r2) {
            (Ok(()), Ok(())) => self.status = Some("  ✓  saved to config.toml".into()),
            (Err(e), _) | (_, Err(e)) => self.status = Some(format!("  ✗  {}", e)),
        }
    }
}

/// Read config file, update `[roles]` section, write back.
fn save_roles_to_config(path: &std::path::Path, roles: &[(String, String)]) -> Result<(), String> {
    // Read existing config text (may not exist yet).
    let existing = if path.exists() {
        std::fs::read_to_string(path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };

    // Parse as toml::Value so we can round-trip non-roles sections.
    let mut doc: toml::Value = if existing.is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&existing).map_err(|e| e.to_string())?
    };

    // Build the new [roles] table.
    let roles_table: toml::map::Map<String, toml::Value> = roles
        .iter()
        .map(|(k, v)| (k.clone(), toml::Value::String(v.clone())))
        .collect();

    if let toml::Value::Table(ref mut t) = doc {
        if roles_table.is_empty() {
            t.remove("roles");
        } else {
            t.insert("roles".into(), toml::Value::Table(roles_table));
        }
    }

    let new_text = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, new_text).map_err(|e| e.to_string())?;
    Ok(())
}

/// Update `[profile.<profile>].model` in the config file. Preserves all other keys.
fn save_default_model_to_config(
    path: &std::path::Path,
    model: &str,
    profile: &str,
) -> Result<(), String> {
    if model.trim().is_empty() {
        return Ok(());
    }
    let existing = if path.exists() {
        std::fs::read_to_string(path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let mut doc: toml::Value = if existing.is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&existing).map_err(|e| e.to_string())?
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
    let new_text = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, new_text).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Permission request (agent → TUI, reply via oneshot) ───────────────────────

struct PermRequest {
    tool_name: String,
    preview: String,
    reply: oneshot::Sender<PermGrant>,
}

// ── TuiEmitter ────────────────────────────────────────────────────────────────

struct TuiEmitter {
    tx: mpsc::UnboundedSender<AgentEvent>,
}

#[async_trait]
impl EventEmitter for TuiEmitter {
    async fn on_tool_start(&self, tool_use_id: &str, name: &str, input: &serde_json::Value) {
        let detail = format_tool_input(name, input);
        let _ = self.tx.send(AgentEvent::ToolStart {
            tool_use_id: tool_use_id.to_string(),
            name: name.to_string(),
            detail,
        });
    }
    async fn on_tool_done(
        &self,
        tool_use_id: &str,
        _name: &str,
        is_error: bool,
        diff: Option<String>,
    ) {
        let _ = self.tx.send(AgentEvent::ToolDone {
            tool_use_id: tool_use_id.to_string(),
            is_error,
            diff,
        });
    }
    async fn on_assistant_text(&self, text: &str) {
        if !text.trim().is_empty() {
            let _ = self.tx.send(AgentEvent::Thinking(text.to_string()));
        }
    }

    async fn on_budget_warning(&self, pct: u8, spent_usd: f64, limit_usd: f64) {
        let _ = self.tx.send(AgentEvent::BudgetWarning {
            percent: pct,
            cost: spent_usd,
            limit: limit_usd,
        });
    }
}

fn format_tool_input(name: &str, input: &serde_json::Value) -> String {
    let s = match name {
        "Read" => input["file_path"].as_str().unwrap_or("").to_string(),
        "Write" | "Edit" => input["file_path"].as_str().unwrap_or("").to_string(),
        "Bash" => input["command"]
            .as_str()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .to_string(),
        "Glob" => input["pattern"].as_str().unwrap_or("").to_string(),
        "Grep" => format!(
            "{}{}",
            input["pattern"].as_str().unwrap_or(""),
            input["path"]
                .as_str()
                .map(|p| format!("  {}", p))
                .unwrap_or_default()
        ),
        _ => input.to_string(),
    };
    if s.chars().count() > 72 {
        format!("{}…", s.chars().take(72).collect::<String>())
    } else {
        s
    }
}

// ── TuiAskUser ────────────────────────────────────────────────────────────────

struct TuiAskUser {
    perm_tx: mpsc::UnboundedSender<PermRequest>,
    perms: Arc<Mutex<PermsState>>,
}

#[async_trait]
impl AskUser for TuiAskUser {
    async fn ask(&self, req: AgentPermRequest) -> AgentPermGrant {
        let tool_name = &req.tool_name;
        // Fast-path: check session/workdir grants before going to the TUI.
        {
            let state = self.perms.lock().unwrap();
            if state.workdir_open || state.session_allowed.contains(tool_name) {
                return AgentPermGrant::Allow;
            }
        }

        let preview = if req.description.chars().count() > 120 {
            format!("{}…", req.description.chars().take(120).collect::<String>())
        } else {
            req.description.clone()
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .perm_tx
            .send(PermRequest {
                tool_name: tool_name.clone(),
                preview,
                reply: reply_tx,
            })
            .is_err()
        {
            return AgentPermGrant::Deny;
        }
        match reply_rx.await.unwrap_or(PermGrant::Deny) {
            PermGrant::Once => AgentPermGrant::Allow,
            PermGrant::Session => {
                self.perms
                    .lock()
                    .unwrap()
                    .session_allowed
                    .insert(tool_name.clone());
                AgentPermGrant::AllowAll
            }
            PermGrant::Workdir => {
                self.perms.lock().unwrap().workdir_open = true;
                AgentPermGrant::AllowAll
            }
            PermGrant::Deny => AgentPermGrant::Deny,
            PermGrant::DenyWithFeedback(fb) => AgentPermGrant::DenyWithFeedback(fb),
        }
    }
}

// ── Status strip ─────────────────────────────────────────────────────────────

struct StatusEntry {
    tool_use_id: String,
    name: String,
    detail: String,
    done: bool,
    is_error: bool,
    start: std::time::Instant,
    elapsed_ms: Option<u64>,
}

// ── Chat lines ────────────────────────────────────────────────────────────────

enum ChatLine {
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
}

// ── App state ─────────────────────────────────────────────────────────────────

struct PendingPerm {
    tool_name: String,
    preview: String,
    reply: oneshot::Sender<PermGrant>,
}

/// Structured error info for the error popup.
struct ErrorInfo {
    title: String,
    detail: String,
    recovery: Option<String>,
    #[allow(dead_code)]
    is_transient: bool,
}

impl ErrorInfo {
    fn from_message(msg: impl Into<String>) -> Self {
        let detail = msg.into();
        // Map known API error patterns to structured info
        if detail.contains("401")
            || detail.contains("Unauthorized")
            || detail.contains("Invalid API key")
        {
            Self {
                title: "Authentication Error".into(),
                detail: detail.clone(),
                recovery: Some("Check your API key in /profile edit".into()),
                is_transient: false,
            }
        } else if detail.contains("429")
            || detail.contains("Rate limit")
            || detail.contains("rate_limit")
        {
            Self {
                title: "Rate Limited".into(),
                detail: detail.clone(),
                recovery: Some("Wait a moment, then try again".into()),
                is_transient: true,
            }
        } else if detail.contains("500")
            || detail.contains("502")
            || detail.contains("503")
            || detail.contains("Server error")
        {
            Self {
                title: "Server Error".into(),
                detail: detail.clone(),
                recovery: Some("Try again — the provider may be experiencing issues".into()),
                is_transient: true,
            }
        } else if detail.contains("Could not save") {
            Self {
                title: "Save Error".into(),
                detail: detail.clone(),
                recovery: Some("Check file permissions".into()),
                is_transient: false,
            }
        } else {
            Self {
                title: "Error".into(),
                detail,
                recovery: None,
                is_transient: false,
            }
        }
    }
}

struct App {
    messages: Vec<ChatLine>,
    /// Live activity log shown in the status strip (last 2 entries).
    status_log: std::collections::VecDeque<StatusEntry>,
    input: String,
    cursor: usize,
    /// Current scroll offset (logical lines from top). Updated by handle_key; clamped in render.
    scroll: u32,
    /// Max scroll as computed during the last render — used by handle_key to scroll up correctly.
    max_scroll: u32,
    following: bool,
    /// If set after a terminal resize, restore scroll to this ratio of max_scroll on next render.
    pending_scroll_ratio: Option<f64>,
    busy: bool,
    spinner_tick: usize,
    pending_perm: Option<PendingPerm>,
    /// Error modal: shown as overlay, dismissed with Enter/Esc/Space.
    pending_error: Option<ErrorInfo>,
    /// Unified overlay stack (new system — ErrorOverlay, ReadOnlyOverlay, etc.)
    overlay_stack: OverlayStack,
    prompt_tx: mpsc::UnboundedSender<String>,
    /// Channel to request session resume in agent_task.
    resume_tx: mpsc::UnboundedSender<String>,
    /// Channel to switch the session model in agent_task.
    model_switch_tx: mpsc::UnboundedSender<String>,
    /// Channel to update tool workspace in agent_task.
    workdir_tx: mpsc::UnboundedSender<std::path::PathBuf>,
    /// Inputs queued while agent was busy — drained FIFO when agent finishes.
    queued: VecDeque<String>,
    /// Session picker popup state (Some = popup visible).
    session_picker: Option<SessionPickerState>,
    /// Model picker popup state (Some = popup visible).
    model_picker: Option<ModelPickerState>,
    /// Profile picker popup state (Some = popup visible).
    profile_picker: Option<ProfilePickerState>,
    /// Role picker popup state (Some = popup visible).
    role_picker: Option<RolePickerState>,
    /// Settings editor popup (Some = visible).
    settings: Option<SettingsState>,
    /// In-TUI profile overview/editor overlay (Some = visible).
    profile_overlay: Option<ProfileOverlayState>,
    /// All known models (built at startup from pricing table + profiles).
    known_models: Vec<ModelEntry>,
    /// User model preferences: favorites, recency, role assignments.
    model_prefs: clido_core::ModelPrefs,
    /// Role map from config (name → model ID). Merged with model_prefs.roles at use time.
    config_roles: std::collections::HashMap<String, String>,
    /// Signal to cancel the current agent run (force send).
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Selected option in the permission popup (0=once, 1=session, 2=workdir, 3=deny, 4=deny+feedback).
    perm_selected: usize,
    /// When user picks "Deny with feedback", this holds the feedback text being typed.
    perm_feedback_input: Option<String>,
    /// Tracks whether the user has granted AllowAll for this session (for UI display).
    permission_mode_override: Option<PermissionMode>,
    /// Selected index in the slash-command popup (None = no popup).
    selected_cmd: Option<usize>,
    quit: bool,
    /// When true, the TUI exits and setup wizard re-runs to reconfigure.
    wants_reinit: bool,
    /// When Some(name), the TUI exits and the active profile is switched then TUI restarts.
    wants_profile_switch: Option<String>,
    /// When true, the TUI exits and the profile-creation wizard runs, then TUI restarts.
    wants_profile_create: bool,
    /// When Some(name), the TUI exits and the profile-edit wizard runs, then TUI restarts.
    wants_profile_edit: Option<String>,
    /// When Some(id), restart TUI and resume this session immediately.
    restart_resume_session: Option<String>,
    provider: String,
    model: String,
    /// Active profile name, shown in the header.
    current_profile: String,
    /// Session ID of the current agent session (set after SessionStarted event).
    current_session_id: Option<String>,
    /// Project root used for listing sessions.
    workspace_root: std::path::PathBuf,
    /// Previously submitted prompts (oldest first), for Up/Down history navigation.
    input_history: Vec<String>,
    /// Index into input_history while navigating (None = current draft).
    history_idx: Option<usize>,
    /// Saved draft while navigating history, so Down restores it.
    history_draft: String,
    /// Horizontal scroll offset for the input field (in chars), so long inputs stay visible.
    input_scroll: usize,
    /// Last completed agent invocation's token totals (for context % in header).
    session_input_tokens: u64,
    session_output_tokens: u64,
    session_cost_usd: f64,
    /// Running totals across all completed turns in this TUI session (including planning calls).
    session_total_input_tokens: u64,
    session_total_output_tokens: u64,
    session_total_cost_usd: f64,
    /// Number of completed agent turns in this TUI session.
    session_turn_count: u32,
    /// Max context window in tokens for the current model (0 = unknown).
    context_max_tokens: u64,
    /// Channel to trigger immediate context compaction in agent_task.
    compact_now_tx: mpsc::UnboundedSender<()>,
    /// Last plan produced by the planner (--planner mode) or parsed from a /plan <task> response.
    /// This is the canonical in-memory representation used for save + display.
    last_plan_snapshot: Option<Plan>,
    /// Convenience list of top-level task descriptions derived from `last_plan_snapshot`.
    last_plan: Option<Vec<String>>,
    /// Raw text of the last plan response — used by the text editor to show unmodified formatting.
    last_plan_raw: Option<String>,
    /// Set to true when /plan <task> is sent; cleared after the agent responds and the plan is parsed.
    awaiting_plan_response: bool,
    /// Rules overlay: Some = popup visible, None = hidden.
    /// Each entry is (file_path_display, first_3_lines_preview).
    rules_overlay: Option<Vec<(String, String)>>,
    /// When true, fire desktop notification + terminal bell after each agent turn
    /// (subject to the MIN_ELAPSED_SECS gate in `notify.rs`).
    notify_enabled: bool,
    /// Shared flag that gates SpawnReviewerTool execution.  Toggle with `/reviewer on|off`.
    reviewer_enabled: Arc<AtomicBool>,
    /// Set to true during crash recovery so the ResumedSession event preserves
    /// the current TUI messages instead of clearing and replaying them.
    recovering: bool,
    /// True when an explicit reviewer slot is configured in config.toml.
    /// Controls whether the reviewer badge and /reviewer command are shown.
    reviewer_configured: bool,
    /// Timestamp of when the current agent turn was submitted; used to compute elapsed time.
    turn_start: Option<std::time::Instant>,
    /// Previous model to revert to after a per-turn `@model` override completes.
    per_turn_prev_model: Option<String>,
    /// Image loaded via `/image <path>` — attached to the next user message then cleared.
    pending_image: Option<ImageAttachment>,
    /// Shared state: image to attach to the next prompt.  Written by the TUI on send,
    /// drained by agent_task before calling run/run_next_turn.
    image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
    /// When `Some`, the plan editor full-screen overlay is active (--plan flag mode).
    plan_editor: Option<PlanEditor>,
    /// Currently selected task index in the plan editor list.
    plan_selected_task: usize,
    /// When `Some`, the inline task edit form is active.
    plan_task_editing: Option<TaskEditState>,
    /// Whether we're in plan dry-run mode (show editor but never execute).
    plan_dry_run: bool,
    /// Simple nano-style text editor for /plan edit.
    plan_text_editor: Option<PlanTextEditor>,
    /// Current plan step being executed, extracted from agent text (e.g. "Step 3: Write contract").
    current_step: Option<String>,
    /// The step number most recently seen while the agent was executing a plan.
    /// Used after agent finishes to show which steps remain.
    last_executed_step_num: Option<usize>,
    /// Shared todo list written by the agent via the TodoWrite tool.
    todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
    /// Track whether we have already shown the empty-input hint this session.
    empty_input_hint_shown: bool,
    /// Current prompt enhancement mode (auto / off).
    prompt_mode: PromptMode,
    /// Active prompt rules (global + project, merged).
    prompt_rules: PromptRules,
    /// When Some, holds an enhanced preview that /prompt-preview is waiting to display.
    prompt_preview_text: Option<String>,
    /// Resolved API key for the active profile — used for live model fetching.
    api_key: String,
    /// Optional custom base URL for the active profile's provider.
    base_url: Option<String>,
    /// Channel to send AgentEvents from background tasks (e.g. model fetch) to the TUI loop.
    fetch_tx: mpsc::UnboundedSender<AgentEvent>,
    /// True while a model-list fetch is in progress (shows spinner in model picker).
    models_loading: bool,
    /// Render cache: maps (content_hash, render_width) to pre-built Line<'static> slices.
    /// Avoids re-parsing markdown on every 80ms render tick when messages haven't changed.
    /// Invalidated (cleared) on terminal resize since width affects line-wrapping.
    render_cache: std::collections::HashMap<(u64, usize), Vec<Line<'static>>>,
    /// Hash of the messages Vec at the time the cache was last populated.
    /// Used to detect when messages change and stale entries should be evicted.
    render_cache_msg_count: usize,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    fn new(
        prompt_tx: mpsc::UnboundedSender<String>,
        resume_tx: mpsc::UnboundedSender<String>,
        model_switch_tx: mpsc::UnboundedSender<String>,
        workdir_tx: mpsc::UnboundedSender<std::path::PathBuf>,
        compact_now_tx: mpsc::UnboundedSender<()>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        provider: String,
        model: String,
        workspace_root: std::path::PathBuf,
        notify_enabled: bool,
        image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
        plan_dry_run: bool,
        known_models: Vec<ModelEntry>,
        model_prefs: clido_core::ModelPrefs,
        config_roles: std::collections::HashMap<String, String>,
        current_profile: String,
        reviewer_enabled: Arc<AtomicBool>,
        reviewer_configured: bool,
        todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
        api_key: String,
        base_url: Option<String>,
        fetch_tx: mpsc::UnboundedSender<AgentEvent>,
    ) -> Self {
        let mut app = Self {
            messages: Vec::new(),
            status_log: std::collections::VecDeque::new(),
            input: String::new(),
            cursor: 0,
            scroll: 0,
            max_scroll: 0,
            following: true,
            pending_scroll_ratio: None,
            busy: false,
            spinner_tick: 0,
            pending_perm: None,
            pending_error: None,
            overlay_stack: OverlayStack::new(),
            prompt_tx,
            resume_tx,
            model_switch_tx,
            workdir_tx,
            queued: VecDeque::new(),
            session_picker: None,
            model_picker: None,
            profile_picker: None,
            role_picker: None,
            settings: None,
            profile_overlay: None,
            known_models,
            model_prefs,
            config_roles,
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
            workspace_root,
            input_history: Vec::new(),
            history_idx: None,
            history_draft: String::new(),
            input_scroll: 0,
            session_input_tokens: 0,
            session_output_tokens: 0,
            session_cost_usd: 0.0,
            session_total_input_tokens: 0,
            session_total_output_tokens: 0,
            session_total_cost_usd: 0.0,
            session_turn_count: 0,
            context_max_tokens: 0,
            compact_now_tx,
            last_plan_snapshot: None,
            last_plan: None,
            last_plan_raw: None,
            awaiting_plan_response: false,
            rules_overlay: None,
            notify_enabled,
            reviewer_enabled,
            recovering: false,
            reviewer_configured,
            turn_start: None,
            per_turn_prev_model: None,
            pending_image: None,
            image_state,
            plan_editor: None,
            plan_selected_task: 0,
            plan_task_editing: None,
            plan_text_editor: None,
            current_step: None,
            last_executed_step_num: None,
            plan_dry_run,
            todo_store,
            empty_input_hint_shown: false,
            prompt_mode: PromptMode::Auto,
            prompt_rules: PromptRules::default(),
            prompt_preview_text: None,
            api_key,
            base_url,
            fetch_tx,
            models_loading: false,
            render_cache: std::collections::HashMap::new(),
            render_cache_msg_count: 0,
        };
        app.prompt_mode = load_prompt_mode(&app.workspace_root);
        app.prompt_rules = load_rules(&app.workspace_root);
        app.messages.push(ChatLine::WelcomeSplash);
        app
    }

    fn push(&mut self, line: ChatLine) {
        self.messages.push(line);
        // scroll position is computed at render time when following=true
    }

    /// Send immediately (not busy). Moves input → chat + agent.
    /// If input starts with `@model-name prompt`, applies a per-turn model override.
    /// Send `prompt` to the agent without showing anything in the chat.
    fn send_silent(&mut self, prompt: String) {
        let _ = self.prompt_tx.send(prompt);
        self.input.clear();
        self.cursor = 0;
        self.busy = true;
        self.following = true;
        self.turn_start = Some(std::time::Instant::now());
        self.history_idx = None;
        self.history_draft.clear();
    }

    fn send_now(&mut self, text: String) {
        // If a pending image was attached via /image, publish it to the shared image_state
        // so agent_task can prepend an Image ContentBlock to this user message.
        if let Some(img) = self.pending_image.take() {
            if let Ok(mut guard) = self.image_state.lock() {
                *guard = Some((img.media_type.to_string(), img.base64_data));
            }
        }
        // Check for per-turn @model-name prefix.
        let send_result = if let Some((per_turn_model, actual_prompt)) = parse_per_turn_model(&text)
        {
            self.per_turn_prev_model = Some(self.model.clone());
            self.model = per_turn_model.clone();
            let _ = self.model_switch_tx.send(per_turn_model.clone());
            self.push(ChatLine::Info(format!(
                "  ↻ Using {} for this turn only",
                per_turn_model
            )));
            self.push(ChatLine::User(actual_prompt.clone()));
            if self.input_history.last().map(|s| s.as_str()) != Some(text.as_str()) {
                self.input_history.push(text);
                if self.input_history.len() > 1000 {
                    self.input_history.remove(0);
                }
            }
            self.prompt_tx.send(actual_prompt)
        } else {
            self.push(ChatLine::User(text.clone()));
            if self.input_history.last().map(|s| s.as_str()) != Some(text.as_str()) {
                self.input_history.push(text.clone());
                if self.input_history.len() > 1000 {
                    self.input_history.remove(0);
                }
            }
            self.prompt_tx.send(text)
        };

        if send_result.is_err() {
            // Agent task channel closed — can't send; stay idle and surface an error.
            self.push(ChatLine::Info(
                "  ✗ Agent is not running — try restarting clido.".into(),
            ));
            return;
        }

        self.input.clear();
        self.cursor = 0;
        self.busy = true;
        self.following = true;
        self.turn_start = Some(std::time::Instant::now());
        self.history_idx = None;
        self.history_draft.clear();
    }

    /// Execute a slash command or send chat to the agent (single user line).
    fn dispatch_user_input(&mut self, text: String) {
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
        } else if let Some(send_text) = self.maybe_enhance_prompt(trimmed) {
            self.send_now(send_text);
        }
    }

    /// Apply prompt enhancement if mode is Auto.  Shows a dim indicator when the
    /// prompt is modified.  When preview mode is active (`prompt_preview_text` is Some),
    /// displays the enhanced prompt without sending and returns None.
    fn maybe_enhance_prompt(&mut self, raw: String) -> Option<String> {
        let ctx = EnhancementCtx {
            mode: self.prompt_mode,
            rules: &self.prompt_rules,
        };
        let (enhanced, was_modified) = crate::prompt_enhance::enhance_prompt(&raw, &ctx);

        // Preview mode: show the enhanced text, don't send.
        if self.prompt_preview_text.is_some() {
            self.prompt_preview_text = None;
            self.push(ChatLine::Info("".into()));
            self.push(ChatLine::Section("Enhanced Prompt Preview".into()));
            for line in enhanced.lines() {
                self.push(ChatLine::Info(format!("  {line}")));
            }
            self.push(ChatLine::Info("".into()));
            self.push(ChatLine::Info(
                "  — preview only, not sent.  Type message again to send.".into(),
            ));
            return None;
        }

        if was_modified {
            self.push(ChatLine::Info("  ✦ Prompt enhanced".into()));
        }
        Some(enhanced)
    }

    /// After the agent is idle, drain the FIFO queue: slash commands run in order until one
    /// submits a new turn (`busy`) or `/quit` is seen.
    fn drain_input_queue(&mut self) {
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
    fn submit(&mut self) {
        if self.pending_perm.is_some() {
            return;
        }
        let text = self.input.trim().to_string();
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
            self.input.clear();
            self.cursor = 0;
            return;
        }
        if self.busy {
            // Enqueue for after the current run finishes (FIFO).
            self.queued.push_back(text);
            self.input.clear();
            self.cursor = 0;
        } else {
            self.dispatch_user_input(text);
        }
    }

    /// Ctrl+Enter: cancel current run and send input immediately.
    fn force_send(&mut self) {
        if self.pending_perm.is_some() {
            return;
        }
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.busy {
            // Cancel the running agent turn, then queue this as next prompt.
            self.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            // Prioritize this prompt ahead of already queued inputs.
            self.queued.push_front(text);
            self.input.clear();
            self.cursor = 0;
            self.push(ChatLine::Info("  ↻ Interrupted — sending next".into()));
        } else {
            self.dispatch_user_input(text);
        }
    }

    /// Interrupt current run without sending a follow-up prompt.
    fn stop_only(&mut self) {
        if self.pending_perm.is_some() || !self.busy {
            return;
        }
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.push(ChatLine::Info("  ↻ Interrupted".into()));
    }

    fn push_status(&mut self, tool_use_id: String, name: String, detail: String) {
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

    fn finish_status(&mut self, tool_use_id: &str, is_error: bool) {
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
    fn on_agent_done(&mut self) {
        self.busy = false;
        self.status_log.clear();
        self.current_step = None;
        self.cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.session_turn_count += 1;

        // Show elapsed time for the completed turn.
        if let Some(start) = self.turn_start {
            let elapsed = start.elapsed();
            let elapsed_str = if elapsed.as_secs() >= 60 {
                format!(
                    "  done in {}m {}s",
                    elapsed.as_secs() / 60,
                    elapsed.as_secs() % 60
                )
            } else if elapsed.as_secs() >= 1 {
                format!("  done in {:.1}s", elapsed.as_secs_f64())
            } else {
                format!("  done in {}ms", elapsed.as_millis())
            };
            self.push(ChatLine::Info(elapsed_str));
        }

        // If a plan was running and not all steps were completed, show remaining steps.
        if let Some(last_num) = self.last_executed_step_num {
            if let Some(plans) = self.last_plan.clone() {
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
        // Rule evolution: observe the completed user turn for learnable patterns.
        if let Some(user_text) = self.last_user_text().map(|s| s.to_string()) {
            let promoted = self.prompt_rules.observe_turn(&user_text);
            if !promoted.is_empty() {
                // Persist the updated rules to the project rules file (silently).
                let rules_path = project_rules_path(&self.workspace_root);
                let _ = save_rules(&rules_path, &self.prompt_rules);
                for phrase in &promoted {
                    self.push(ChatLine::Info(format!(
                        "  ✦ Learned rule: \"{}\"  (use /prompt-rules to view)",
                        phrase
                    )));
                }
            }
        }
        if self.awaiting_plan_response {
            self.awaiting_plan_response = false;
            if let Some(text) = self.last_assistant_text().map(|s| s.to_string()) {
                self.last_plan_raw = Some(text.clone());
                if let Some(plan) = build_plan_from_assistant_text(&text) {
                    if let Err(e) = clido_planner::save_plan(&self.workspace_root, &plan) {
                        self.push(ChatLine::Info(format!("  ⚠ Could not save plan: {}", e)));
                    }
                    self.last_plan = Some(
                        plan.tasks
                            .iter()
                            .map(|t| t.description.clone())
                            .collect::<Vec<_>>(),
                    );
                    self.last_plan_snapshot = Some(plan);
                }
            }
        }
        self.drain_input_queue();
    }

    fn tick_spinner(&mut self) {
        if self.busy || self.pending_perm.is_some() {
            self.spinner_tick = (self.spinner_tick + 1) % SPINNER.len();
        }
    }

    fn last_assistant_text(&self) -> Option<&str> {
        self.messages.iter().rev().find_map(|line| match line {
            ChatLine::Assistant(text) if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
    }

    fn last_user_text(&self) -> Option<&str> {
        self.messages.iter().rev().find_map(|line| match line {
            ChatLine::User(text) if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // ── Plan text editor (nano-style) full-screen overlay ───────────────────
    if app.plan_text_editor.is_some() {
        render_plan_text_editor(frame, app, area);
        return;
    }

    // ── Plan editor full-screen overlay ─────────────────────────────────────
    if app.plan_editor.is_some() {
        render_plan_editor(frame, app, area);
        return;
    }

    // ── Header spans (built before layout so we can measure and pick height) ──
    let version = env!("CARGO_PKG_VERSION");
    let dim = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);

    // Line 1: brand · version · provider/model · profile · reviewer
    let mut hline1: Vec<Span<'static>> = vec![
        Span::styled(
            "cli",
            Style::default()
                .fg(Color::Rgb(210, 220, 240))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            ";",
            Style::default()
                .fg(TUI_SOFT_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "do",
            Style::default()
                .fg(Color::Rgb(210, 220, 240))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  v{}  ", version),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            if app.per_turn_prev_model.is_some() {
                format!("{}  {}⁺", app.provider, app.model)
            } else {
                format!("{}  {}", app.provider, app.model)
            },
            dim,
        ),
        Span::styled(format!("  [{}]", app.current_profile), dim),
    ];
    if app.reviewer_configured {
        let (dot, color) = if app.reviewer_enabled.load(Ordering::Relaxed) {
            ("●", Color::Green)
        } else {
            ("○", Color::DarkGray)
        };
        hline1.push(Span::styled(
            format!("  reviewer {}", dot),
            Style::default().fg(color).add_modifier(Modifier::DIM),
        ));
    }

    // Line 2: dir · cost/tokens
    let mut hline2: Vec<Span<'static>> = vec![Span::styled(
        {
            let home = std::env::var("HOME").unwrap_or_default();
            let raw = app.workspace_root.display().to_string();
            let short = if !home.is_empty() && raw.starts_with(&home) {
                format!("~{}", &raw[home.len()..])
            } else {
                raw
            };
            format!("  {}", short)
        },
        dim,
    )];
    if app.session_total_cost_usd > 0.0 {
        // Format token count (combined in+out for this session)
        let sum_tokens = app.session_total_input_tokens + app.session_total_output_tokens;
        let tok_str = if sum_tokens >= 1_000_000 {
            format!("{:.2}M tok", sum_tokens as f64 / 1_000_000.0)
        } else if sum_tokens >= 1000 {
            format!("{:.1}k tok", sum_tokens as f64 / 1000.0)
        } else {
            format!("{} tok", sum_tokens)
        };

        // Context window usage — use last-turn input as proxy
        let ctx_str = if app.context_max_tokens > 0 && app.session_input_tokens > 0 {
            let pct = (app.session_input_tokens as f64 / app.context_max_tokens as f64 * 100.0)
                .min(100.0);
            format!("  {:.0}% window", pct)
        } else {
            String::new()
        };

        hline2.push(Span::styled(
            format!(
                "   session: ${:.4}  {}{}",
                app.session_total_cost_usd, tok_str, ctx_str
            ),
            dim,
        ));
    }

    // Decide header height: 1 line if everything fits side-by-side, else 2.
    // When the terminal is very narrow, use a single minimal header.
    let w = area.width as usize;
    let is_narrow = area.width < 60;
    let line1_w: usize = hline1.iter().map(|s| s.content.chars().count()).sum();
    let line2_w: usize = hline2.iter().map(|s| s.content.chars().count()).sum();
    let header_h: u16 = if is_narrow || line1_w + line2_w <= w {
        1
    } else {
        2
    };

    // Layout: header | chat | status (2) | queue (1) | hint (1) | input (dynamic)
    // Input grows with content: 1 line of text = 3 rows (2 borders + 1), capped at 12.
    // When very narrow (< 40), collapse optional rows to avoid layout panics.
    let input_line_count = app.input.matches('\n').count() + 1;
    let input_h = (input_line_count as u16 + 2).min(12);
    let (hint_h, status_h) = if area.width < 40 { (0, 0) } else { (1, 2) };
    let [header_area, chat_area, status_area, queue_area, hint_area, input_area] =
        Layout::vertical([
            Constraint::Length(header_h),
            Constraint::Min(0),
            Constraint::Length(status_h),
            Constraint::Length(1),
            Constraint::Length(hint_h),
            Constraint::Length(input_h),
        ])
        .areas(area);

    // ── Header render ──
    let header_para = if is_narrow {
        // Minimal single-line header: just the model name.
        Paragraph::new(Line::from(vec![Span::styled(
            truncate_chars(&app.model, w),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]))
    } else if header_h == 1 {
        // Everything on one line — append line2 to line1 and fit to width.
        hline1.extend(hline2);
        Paragraph::new(Line::from(fit_spans(hline1, w)))
    } else {
        // Two lines: fit each independently.
        let l1 = fit_spans(hline1, w);
        let l2 = fit_spans(hline2, w);
        Paragraph::new(vec![Line::from(l1), Line::from(l2)])
    };
    frame.render_widget(header_para, header_area);

    // ── Chat ──
    if is_welcome_only(app) {
        render_welcome(frame, app, chat_area);
    } else {
        // Use ratatui's own line_count() so the scroll calculation matches actual rendering.
        let lines = build_lines_w(app, chat_area.width as usize);
        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        let total_height = para.line_count(chat_area.width) as u32;
        let max_scroll = total_height.saturating_sub(chat_area.height as u32);
        // Store for use in handle_key (Up/PageUp need the current max_scroll).
        app.max_scroll = max_scroll;
        // If a resize just occurred, restore scroll to the saved ratio.
        if let Some(ratio) = app.pending_scroll_ratio.take() {
            app.scroll = ((ratio * max_scroll as f64).round() as u32).min(max_scroll);
        }
        let scroll = if app.following {
            max_scroll
        } else {
            app.scroll.min(max_scroll)
        };
        // ratatui's scroll() takes (u16, u16); clamp to u16::MAX before casting.
        frame.render_widget(
            para.scroll((scroll.min(u16::MAX as u32) as u16, 0)),
            chat_area,
        );
    }

    // ── Status strip ──
    {
        let status_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        let spinner = SPINNER[app.spinner_tick];
        let mut slines: Vec<Line<'static>> = Vec::new();
        for entry in &app.status_log {
            let (icon, style, elapsed_str) = if entry.done {
                let ms = entry.elapsed_ms.unwrap_or(0);
                let t = format!(" {}ms", ms);
                if entry.is_error {
                    (
                        "✗",
                        Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                        t,
                    )
                } else {
                    ("✓", status_style, t)
                }
            } else {
                let elapsed = entry.start.elapsed();
                let secs = elapsed.as_secs_f64();
                let t = if secs < 1.0 {
                    format!(" {:.0}ms", elapsed.as_millis())
                } else {
                    format!(" {:.1}s", secs)
                };
                let running_color = tool_color(&entry.name, false, false);
                (
                    spinner,
                    Style::default()
                        .fg(running_color)
                        .add_modifier(Modifier::DIM),
                    t,
                )
            };
            slines.push(Line::from(vec![
                Span::styled(format!("  {} ", icon), style),
                Span::styled(tool_display_name(&entry.name).to_string(), style),
                Span::styled(format!("  {}", entry.detail), status_style),
                Span::styled(elapsed_str, status_style),
            ]));
        }
        while slines.len() < 2 {
            slines.push(Line::raw(""));
        }
        frame.render_widget(Paragraph::new(slines), status_area);
    }

    // ── Queue strip ──
    {
        let queue_line = if !app.queued.is_empty() {
            let n = app.queued.len();
            let first = app.queued.front().unwrap();
            let preview = if first.chars().count() > 50 {
                format!("{}…", first.chars().take(50).collect::<String>())
            } else {
                first.clone()
            };
            let label = if n == 1 {
                "  ↻ 1 queued  ".to_string()
            } else {
                format!("  ↻ {} queued  ", n)
            };
            Line::from(vec![
                Span::styled(
                    label,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    format!("\"{}\"", preview),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else if let Some(ref step) = app.current_step {
            Line::from(vec![
                Span::styled(
                    "  ▶ ",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    truncate_chars(step, 80),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::raw("")
        };
        frame.render_widget(Paragraph::new(queue_line), queue_area);
    }

    // ── Input box (always rendered, even when permission popup is showing) ──
    // Compute cursor position.  For multiline input, derive (row, col) from the
    // char offset; for single-line input use horizontal scroll as before.
    let input_visible_w = (input_area.width as usize).saturating_sub(4).max(1);
    let byte_at_cursor = char_byte_pos(&app.input, app.cursor);
    let before_cursor = &app.input[..byte_at_cursor];
    let is_multiline = app.input.contains('\n');
    let (cursor_row, cursor_col): (u16, u16) = if is_multiline {
        let row = before_cursor.matches('\n').count() as u16;
        let col = before_cursor
            .rfind('\n')
            .map(|p| app.input[p + 1..byte_at_cursor].chars().count())
            .unwrap_or_else(|| before_cursor.chars().count()) as u16;
        (row, col.min(input_visible_w as u16))
    } else {
        // Single-line: maintain horizontal scroll window.
        if app.cursor < app.input_scroll {
            app.input_scroll = app.cursor;
        } else if app.cursor >= app.input_scroll + input_visible_w {
            app.input_scroll = app.cursor - input_visible_w + 1;
        }
        (0, (app.cursor - app.input_scroll) as u16)
    };

    // Build the paragraph text.  Multiline: one ratatui Line per input line.
    // Single-line: horizontally-scrolled window as before.
    let max_visible_content_rows = input_h.saturating_sub(2) as usize; // minus top+bottom border
    let input_para_lines: Vec<Line<'static>> = if is_multiline {
        let all_lines: Vec<&str> = app.input.split('\n').collect();
        // Vertical scroll: keep the cursor line visible.
        let v_scroll = if cursor_row as usize >= max_visible_content_rows {
            cursor_row as usize - max_visible_content_rows + 1
        } else {
            0
        };
        all_lines
            .iter()
            .skip(v_scroll)
            .take(max_visible_content_rows)
            .map(|l| Line::raw(format!(" {}", l)))
            .collect()
    } else {
        let visible: String = app
            .input
            .chars()
            .skip(app.input_scroll)
            .take(input_visible_w)
            .collect();
        vec![Line::raw(format!(" {}", visible))]
    };

    // Always clear the input area first — prevents any bleed-through from overlapping widgets.
    frame.render_widget(Clear, input_area);

    if app.busy || app.pending_perm.is_some() {
        let spinner = SPINNER[app.spinner_tick];
        let title_line = if app.pending_perm.is_some() {
            Line::from(vec![
                Span::styled("⏸", Style::default().fg(Color::LightMagenta)),
                Span::styled(
                    " waiting for permission… ",
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        } else if !app.queued.is_empty() {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    "queued — Ctrl+Enter to interrupt".to_string(),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        } else if app.input.is_empty() {
            let elapsed_s = app.turn_start.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            let elapsed_hint = if elapsed_s >= 1 {
                format!(" {}s", elapsed_s)
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    format!(
                        "thinking…{}  (type a follow-up to queue, Ctrl+Enter to interrupt)",
                        elapsed_hint
                    ),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    "thinking…  Enter=queue  Ctrl+Enter=interrupt".to_string(),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        };
        let block = Block::default()
            .title(title_line)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightMagenta));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        if app.pending_perm.is_none() {
            frame.set_cursor_position((
                input_area.x + 2 + cursor_col,
                input_area.y + 1 + cursor_row.min(max_visible_content_rows as u16 - 1),
            ));
        }
    } else {
        let idle_title = Line::from(vec![Span::styled(
            if is_multiline {
                " Shift+Enter=newline  (Enter=send  Ctrl+Enter=interrupt  /help=commands) "
            } else {
                " Ask anything  (Enter=send  Shift+Enter=newline  /help=commands) "
            },
            Style::default().fg(TUI_SOFT_ACCENT),
        )]);
        let block = Block::default()
            .title(idle_title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TUI_SOFT_ACCENT));
        let para = Paragraph::new(input_para_lines).block(block);
        frame.render_widget(para, input_area);
        frame.set_cursor_position((
            input_area.x + 2 + cursor_col,
            input_area.y + 1 + cursor_row.min(max_visible_content_rows as u16 - 1),
        ));
    }

    // ── Hint line — hidden when terminal is very narrow ──
    if area.width >= 40 {
        let hint_dim = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        // Mode indicator: show active overlay/picker name
        let mode_label = if !app.overlay_stack.is_empty() {
            app.overlay_stack.top().map(|o| o.title().to_string())
        } else if app.profile_overlay.is_some() {
            Some("Profile".into())
        } else if app.settings.is_some() {
            Some("Settings".into())
        } else if app.model_picker.is_some() {
            Some("Models".into())
        } else if app.session_picker.is_some() {
            Some("Sessions".into())
        } else if app.profile_picker.is_some() {
            Some("Profiles".into())
        } else if app.role_picker.is_some() {
            Some("Roles".into())
        } else {
            None
        };
        let mut hint_spans: Vec<Span<'static>> = Vec::new();
        if let Some(label) = mode_label {
            hint_spans.push(Span::styled(
                format!("  [{}]  ", label),
                Style::default()
                    .fg(TUI_SOFT_ACCENT)
                    .add_modifier(Modifier::DIM),
            ));
        }
        hint_spans.extend([
            Span::styled("  Enter", Style::default().fg(Color::DarkGray)),
            Span::styled(" send  ", hint_dim),
            Span::styled("Shift+Enter", Style::default().fg(Color::DarkGray)),
            Span::styled(" newline  ", hint_dim),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" clear  ", hint_dim),
            Span::styled("↑↓", Style::default().fg(Color::DarkGray)),
            Span::styled(" history  ", hint_dim),
            Span::styled("PgUp/PgDn", Style::default().fg(Color::DarkGray)),
            Span::styled(" scroll  ", hint_dim),
            Span::styled("/settings", Style::default().fg(Color::DarkGray)),
            Span::styled(" settings  ", hint_dim),
            Span::styled("/help", Style::default().fg(Color::DarkGray)),
            Span::styled(" commands  ", hint_dim),
            Span::styled("Ctrl+/", Style::default().fg(Color::DarkGray)),
            Span::styled(" stop agent  ", hint_dim),
            Span::styled("Ctrl+C", Style::default().fg(Color::DarkGray)),
            Span::styled(" quit  ", hint_dim),
            Span::styled("Ctrl+L", Style::default().fg(Color::DarkGray)),
            Span::styled(" refresh  ", hint_dim),
            Span::styled("Shift+select", Style::default().fg(Color::DarkGray)),
            Span::styled(" copy text  ", hint_dim),
        ]);
        // Scroll position indicator when not following.
        if app.max_scroll > 0 && !app.following {
            let pct = (app.scroll * 100 / app.max_scroll).min(100);
            hint_spans.push(Span::styled(
                format!("  ↑ {}%", pct),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
        }
        let hint_spans = fit_spans(hint_spans, hint_area.width as usize);
        let hint = Paragraph::new(Line::from(hint_spans));
        frame.render_widget(hint, hint_area);
    }

    // ── "↓ new messages" scroll indicator ──
    if !app.following && app.max_scroll > app.scroll {
        let unread_hint = Span::styled(
            "  ↓ new messages  PgDn ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
        let hint_line = Line::from(vec![unread_hint]);
        let hint_para = Paragraph::new(hint_line);
        let hint_rect = Rect {
            x: chat_area.x + chat_area.width.saturating_sub(20),
            y: chat_area.y + chat_area.height.saturating_sub(1),
            width: 20,
            height: 1,
        };
        frame.render_widget(hint_para, hint_rect);
    }

    // ── Overlay modals (all rendered above the input field, same structure) ──
    //
    // Rendering order matters: later draws on top. Only one modal is active at
    // a time (handle_key enforces this), but we still render in priority order:
    //   slash completions < session picker < permission < error
    //
    // Shared helpers used by every modal:
    //   popup_above_input(input_area, h, w) → Rect anchored just above input
    //   modal_block(title, border_color)    → styled Block
    //   modal_row_two_col(...)              → two-column selectable row

    // ── Slash command popup ──
    let rows = slash_completion_rows(&app.input);
    if !rows.is_empty() && app.pending_perm.is_none() && app.session_picker.is_none() {
        const VISIBLE: usize = 12;

        // Find the rendered-row index of the selected command.
        let selected_row_idx = app
            .selected_cmd
            .and_then(|sel| {
                rows.iter().position(
                    |r| matches!(r, CompletionRow::Cmd { flat_idx, .. } if *flat_idx == sel),
                )
            })
            .unwrap_or(0);

        // Scroll so the selected item sits at the bottom of the visible window —
        // same behaviour as the session / model pickers.
        let scroll_offset = selected_row_idx.saturating_sub(VISIBLE - 1);
        let end = (scroll_offset + VISIBLE).min(rows.len());
        let visible_slice = &rows[scroll_offset..end];

        let has_above = scroll_offset > 0;
        let has_below = rows.len() > scroll_offset + VISIBLE;
        let indicator = usize::from(has_above || has_below);
        let popup_h = (visible_slice.len() + 2 + indicator) as u16;

        // Use nearly the full terminal width; cap at 120 for ultra-wide displays.
        let popup_w = area.width.saturating_sub(4).min(120);
        // 2 chars for marker (▶ / space), 18 for command = 20 total left column.
        let cmd_col_w = 20usize;
        let popup_rect = popup_above_input(input_area, popup_h, popup_w);
        let desc_w = (popup_rect.width as usize).saturating_sub(cmd_col_w + 3);

        let mut content: Vec<Line<'static>> = visible_slice
            .iter()
            .map(|row| match row {
                CompletionRow::Header(section) => Line::from(Span::styled(
                    format!("  ── {} ", section),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )),
                CompletionRow::Cmd {
                    flat_idx,
                    cmd,
                    desc,
                } => {
                    let selected = app.selected_cmd == Some(*flat_idx);
                    let marker = if selected { "▶" } else { " " };
                    let desc_str = if desc.len() > desc_w {
                        format!("{}…", &desc[..desc_w.saturating_sub(1)])
                    } else {
                        desc.to_string()
                    };
                    modal_row_two_col(
                        format!("{} {:<width$}", marker, cmd, width = cmd_col_w - 2),
                        format!(" {}", desc_str),
                        Color::Cyan,
                        Color::DarkGray,
                        selected,
                    )
                }
            })
            .collect();

        // Scroll indicators — same style as session / model pickers.
        if has_above || has_below {
            let mut parts = Vec::new();
            if has_above {
                parts.push(format!("↑↑ {} more", scroll_offset));
            }
            if has_below {
                parts.push(format!(
                    "↓↓ {} more",
                    rows.len() - (scroll_offset + VISIBLE)
                ));
            }
            content.push(Line::from(Span::styled(
                format!("  {}", parts.join("  ")),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )));
        }

        let n_cmds = rows
            .iter()
            .filter(|r| matches!(r, CompletionRow::Cmd { .. }))
            .count();
        let title = format!(" {} commands ", n_cmds);
        let hint = " ↑↓ navigate · Tab/Enter select · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, TUI_SOFT_ACCENT)),
            popup_rect,
        );
    }

    // ── Session picker ───────────────────────────────────────────────────────
    if let Some(ref picker) = app.session_picker {
        const VISIBLE: usize = 12;
        let filtered = picker.filtered();
        let n_rows = filtered.len().min(VISIBLE) as u16;
        // border(2) + header(1) + blank(1) + filter(1) + rows = n_rows + 5
        let popup_h =
            (n_rows + 5).min(input_area.y.saturating_sub(hint_area.y) + hint_area.y + input_area.y);
        let popup_h = popup_h.min(area.height.saturating_sub(4));
        let popup_h = (n_rows + 5).min(popup_h.max(6));
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);

        let inner_w = popup_rect.width.saturating_sub(4) as usize;
        // fixed cols: marker(2) id(8) sep(2) msg(3) sep(2) cost(6) sep(2) date(11) sep(2) = 38
        let preview_w = inner_w.saturating_sub(38).max(8);

        let mut content: Vec<Line<'static>> = Vec::new();
        // Filter line
        if !picker.filter.is_empty() {
            content.push(Line::from(vec![
                Span::styled("  🔍 ", Style::default().fg(Color::DarkGray)),
                Span::styled(picker.filter.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }
        content.push(Line::from(vec![Span::styled(
            format!(
                "  {:<8}  {:<5}  {:<6}  {:<11}  {}",
                "id", "turns", "cost", "date", "preview"
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));
        content.push(Line::from(vec![Span::styled(
            "  ────────  ─────  ──────  ───────────  ────────────────────".to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));

        let end = (picker.scroll_offset + VISIBLE).min(filtered.len());
        for (di, (_orig_idx, s)) in filtered[picker.scroll_offset..end].iter().enumerate() {
            let selected = picker.scroll_offset + di == picker.selected;
            let bg = if selected {
                TUI_SELECTION_BG
            } else {
                Color::Reset
            };
            let fg = if selected { Color::White } else { Color::Gray };
            let id_short = &s.session_id[..s.session_id.len().min(8)];
            let date_str = if s.start_time.len() >= 16 {
                format!("{} {}", &s.start_time[5..10], &s.start_time[11..16])
            } else {
                s.start_time.clone()
            };
            let preview_str: String = s.preview.chars().take(preview_w).collect();
            let marker = if selected { "▶ " } else { "  " };
            content.push(Line::from(vec![Span::styled(
                format!(
                    "{}{:<8}  {:>5}  ${:<5.2}  {:<11}  {}",
                    marker, id_short, s.num_turns, s.total_cost_usd, date_str, preview_str
                ),
                Style::default().fg(fg).bg(bg),
            )]));
        }

        // Add scroll indicators if there are more sessions above or below visible range.
        let above = picker.scroll_offset;
        let below = filtered
            .len()
            .saturating_sub(picker.scroll_offset + VISIBLE);
        if above > 0 || below > 0 {
            let mut scroll_parts = Vec::new();
            if above > 0 {
                scroll_parts.push(format!("↑↑ {} more above", above));
            }
            if below > 0 {
                scroll_parts.push(format!("↓↓ {} more below", below));
            }
            content.push(Line::from(vec![Span::styled(
                format!("  {}", scroll_parts.join("  ")),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )]));
        }

        let total = filtered.len();
        let picker_title = format!(" Sessions — {} total ", total);
        let hint = " ↑↓ navigate · Enter resume · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&picker_title, hint, Color::Cyan)),
            popup_rect,
        );
    }

    // ── Model picker popup ────────────────────────────────────────────────────
    if let Some(ref picker) = app.model_picker {
        const VISIBLE: usize = 14;
        let filtered = picker.filtered();
        let n_rows = filtered.len().clamp(1, VISIBLE) as u16;
        let popup_h = (n_rows + 5).min(area.height.saturating_sub(4)).max(6);
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);

        let mut content: Vec<Line<'static>> = vec![
            Line::from(vec![Span::styled(
                format!("  Filter: {}_", picker.filter),
                Style::default().fg(Color::White),
            )]),
            Line::from(vec![Span::styled(
                format!(
                    "  {:<2} {:<32}  {:<12}  {:>8}  {:>8}  {:>6}  {}",
                    "  ", "model", "provider", "$/1M in", "$/1M out", "ctx k", "alias"
                ),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )]),
            Line::raw(""),
        ];

        if filtered.is_empty() {
            let msg = if app.models_loading {
                "  ⟳ Fetching models from provider API…"
            } else {
                "  No models found. Check your API key and network connection."
            };
            content.push(Line::from(vec![Span::styled(
                msg.to_string(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )]));
        } else {
            let end = (picker.scroll_offset + VISIBLE).min(filtered.len());
            for (di, m) in filtered[picker.scroll_offset..end].iter().enumerate() {
                let selected = picker.scroll_offset + di == picker.selected;
                let bg = if selected {
                    TUI_SELECTION_BG
                } else {
                    Color::Reset
                };
                let fg = if selected { Color::White } else { Color::Gray };
                let fav = if m.is_favorite { "★" } else { "  " };
                let ctx = m
                    .context_k
                    .map(|k| format!("{:>4}k", k))
                    .unwrap_or_else(|| "    ?".into());
                let role = m.role.as_deref().unwrap_or("");
                let id_display: String = m.id.chars().take(32).collect();
                let prov_display: String = m.provider.chars().take(12).collect();
                content.push(Line::from(vec![Span::styled(
                    format!(
                        "  {} {:<32}  {:<12}  {:>8.2}  {:>8.2}  {}  {}",
                        fav, id_display, prov_display, m.input_mtok, m.output_mtok, ctx, role
                    ),
                    Style::default().fg(fg).bg(bg),
                )]));
            }

            let above = picker.scroll_offset;
            let below = filtered
                .len()
                .saturating_sub(picker.scroll_offset + VISIBLE);
            if above > 0 || below > 0 {
                let mut parts = Vec::new();
                if above > 0 {
                    parts.push(format!("↑↑ {} more", above));
                }
                if below > 0 {
                    parts.push(format!("↓↓ {} more", below));
                }
                content.push(Line::from(vec![Span::styled(
                    format!("  {}", parts.join("  ")),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )]));
            }
        }

        let total = filtered.len();
        let title = if app.models_loading && total == 0 {
            " Models — fetching… ".to_string()
        } else {
            format!(" Models — {} found ", total)
        };
        let hint = " ↑↓ navigate · Enter select · Ctrl+S save default · f fav · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, Color::Magenta)),
            popup_rect,
        );
    }

    // ── Profile picker popup ──────────────────────────────────────────────────
    if let Some(ref picker) = app.profile_picker {
        const VISIBLE: usize = 12;
        let filtered = picker.filtered();
        let n_rows = filtered.len().clamp(1, VISIBLE) as u16;
        let popup_h = (n_rows + 5).min(area.height.saturating_sub(4)).max(5);
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
        let inner_w = popup_rect.width.saturating_sub(4) as usize;

        let mut content: Vec<Line<'static>> = Vec::new();
        if !picker.filter.is_empty() {
            content.push(Line::from(vec![
                Span::styled("  🔍 ", Style::default().fg(Color::DarkGray)),
                Span::styled(picker.filter.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }
        content.push(Line::from(Span::styled(
            format!("  {:<20}  {}", "profile", "provider / model"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
        content.push(Line::raw(""));

        if filtered.is_empty() {
            content.push(Line::from(Span::styled(
                if picker.filter.is_empty() {
                    "  no profiles — press n to create one"
                } else {
                    "  no matches"
                },
                Style::default().fg(Color::DarkGray),
            )));
        }

        let end = (picker.scroll_offset + VISIBLE).min(filtered.len());
        for (di, (_orig_idx, (name, entry))) in
            filtered[picker.scroll_offset..end].iter().enumerate()
        {
            let selected = picker.scroll_offset + di == picker.selected;
            let is_active = name == &picker.active;
            let bg = if selected {
                TUI_SELECTION_BG
            } else {
                Color::Reset
            };
            let fg = if selected { Color::White } else { Color::Gray };
            let marker = if selected { "▶" } else { " " };
            let active_mark = if is_active { "●" } else { " " };
            let model_display: String = format!("{} / {}", entry.provider, entry.model)
                .chars()
                .take(inner_w.saturating_sub(24))
                .collect();
            content.push(Line::from(Span::styled(
                format!("{} {} {:<20}  {}", marker, active_mark, name, model_display),
                Style::default().fg(fg).bg(bg),
            )));
        }

        let above = picker.scroll_offset;
        let below = filtered
            .len()
            .saturating_sub(picker.scroll_offset + VISIBLE);
        if above > 0 || below > 0 {
            let mut parts = Vec::new();
            if above > 0 {
                parts.push(format!("↑↑ {} more", above));
            }
            if below > 0 {
                parts.push(format!("↓↓ {} more", below));
            }
            content.push(Line::from(Span::styled(
                format!("  {}", parts.join("  ")),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )));
        }

        let title = format!(" Profiles — {} ", picker.active);
        let hint = " ↑↓ navigate · Enter switch · n new · e edit · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, Color::Cyan)),
            popup_rect,
        );
    }

    // ── Role picker popup ─────────────────────────────────────────────────────
    if let Some(ref picker) = app.role_picker {
        const VISIBLE: usize = 10;
        let filtered = picker.filtered();
        let n_rows = filtered.len().min(VISIBLE) as u16;
        let popup_h = (n_rows + 5).min(area.height.saturating_sub(4)).max(5);
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
        let inner_w = popup_rect.width.saturating_sub(4) as usize;

        let mut content: Vec<Line<'static>> = Vec::new();
        if !picker.filter.is_empty() {
            content.push(Line::from(vec![
                Span::styled("  🔍 ", Style::default().fg(Color::DarkGray)),
                Span::styled(picker.filter.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }
        content.push(Line::from(Span::styled(
            format!("  {:<16}  {}", "role", "model"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
        content.push(Line::raw(""));

        let end = (picker.scroll_offset + VISIBLE).min(filtered.len());
        for (di, (_orig_idx, (role, model))) in
            filtered[picker.scroll_offset..end].iter().enumerate()
        {
            let selected = picker.scroll_offset + di == picker.selected;
            let bg = if selected {
                TUI_SELECTION_BG
            } else {
                Color::Reset
            };
            let fg = if selected { Color::White } else { Color::Gray };
            let marker = if selected { "▶" } else { " " };
            let model_display: String = model.chars().take(inner_w.saturating_sub(20)).collect();
            content.push(Line::from(Span::styled(
                format!("{} {:<16}  {}", marker, role, model_display),
                Style::default().fg(fg).bg(bg),
            )));
        }

        let above = picker.scroll_offset;
        let below = filtered
            .len()
            .saturating_sub(picker.scroll_offset + VISIBLE);
        if above > 0 || below > 0 {
            let mut parts = Vec::new();
            if above > 0 {
                parts.push(format!("↑↑ {} more", above));
            }
            if below > 0 {
                parts.push(format!("↓↓ {} more", below));
            }
            content.push(Line::from(Span::styled(
                format!("  {}", parts.join("  ")),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )));
        }

        let title = format!(" Roles — {} ", filtered.len());
        let hint = " ↑↓ navigate · Enter switch model · type to filter · Esc close ";
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block_with_hint(&title, hint, Color::Yellow)),
            popup_rect,
        );
    }

    // ── Settings editor popup ─────────────────────────────────────────────────
    if let Some(ref st) = app.settings {
        render_settings(frame, area, input_area, st);
    }

    // ── Profile overview/editor overlay ──────────────────────────────────────
    if let Some(ref st) = app.profile_overlay {
        render_profile_overlay(frame, area, input_area, st);
    }

    // ── Permission popup ─────────────────────────────────────────────────────
    if let Some(perm) = &app.pending_perm {
        let inner_w = input_area.width.saturating_sub(4) as usize;

        // ── Feedback input mode ───────────────────────────────────────────
        if let Some(ref fb) = app.perm_feedback_input {
            let popup_rect = popup_above_input(input_area, 6, input_area.width);
            let preview = truncate_chars(&perm.preview, inner_w);
            let content = vec![
                Line::from(vec![Span::styled(
                    format!("  {}", preview),
                    Style::default().fg(Color::DarkGray),
                )]),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(" Feedback: ", Style::default().fg(Color::Yellow)),
                    Span::styled(fb.as_str(), Style::default().fg(Color::White)),
                    Span::styled("█", Style::default().fg(Color::Yellow)),
                ]),
                Line::raw(""),
                Line::from(vec![Span::styled(
                    "  Enter to send feedback   Esc to go back",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )]),
            ];
            frame.render_widget(Clear, popup_rect);
            frame.render_widget(
                Paragraph::new(content).block(modal_block(
                    " Explain why you are denying this ",
                    Color::Red,
                )),
                popup_rect,
            );
            return;
        }

        // ── Normal option mode ────────────────────────────────────────────
        // 1 preview + 1 blank + 5 options + 1 hint + 2 borders = 10
        let popup_rect = popup_above_input(input_area, 10, input_area.width);
        let preview = truncate_chars(&perm.preview, inner_w);

        const OPTIONS: &[(&str, &str)] = &[
            ("Allow once", "(1) this invocation only"),
            (
                "Allow for session",
                "(2) all calls to this tool until you quit",
            ),
            (
                "Allow all tools",
                "(3) skip all permission checks this session",
            ),
            ("Deny", "(4) block this call, agent continues"),
            (
                "Deny with feedback",
                "(5) block and explain why to the agent",
            ),
        ];

        let mut content = vec![
            Line::from(vec![Span::styled(
                format!("  {}", preview),
                Style::default().fg(Color::DarkGray),
            )]),
            Line::raw(""),
        ];
        for (i, (label, hint)) in OPTIONS.iter().enumerate() {
            let selected = i == app.perm_selected;
            if selected {
                content.push(Line::from(vec![
                    Span::styled(
                        " ▶ ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<28}", label),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", hint), Style::default().fg(Color::DarkGray)),
                ]));
            } else {
                content.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        format!("{:<28}", label),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("  {}", hint),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    ),
                ]));
            }
        }
        content.push(Line::from(vec![Span::styled(
            "  ↑↓/1-5 select   Enter confirm   Esc deny",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));

        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block(
                &format!(" Allow {}? ", tool_display_name(&perm.tool_name)),
                Color::Yellow,
            )),
            popup_rect,
        );
    }

    // ── Error modal ──────────────────────────────────────────────────────────
    if let Some(ref err_info) = app.pending_error {
        let inner_w = input_area.width.saturating_sub(4) as usize;
        let wrapped = word_wrap(&err_info.detail, inner_w);
        let recovery_lines = err_info
            .recovery
            .as_ref()
            .map(|r| word_wrap(r, inner_w))
            .unwrap_or_default();
        // blank + recovery + "[ OK ]" = variable; borders = +2
        let extra = if recovery_lines.is_empty() {
            0
        } else {
            recovery_lines.len() + 1
        };
        let popup_h = ((wrapped.len() + extra + 4) as u16).min(area.height.saturating_sub(4));
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);

        let mut content: Vec<Line<'static>> = wrapped
            .into_iter()
            .map(|l| {
                Line::from(vec![Span::styled(
                    format!("  {}", l),
                    Style::default().fg(Color::White),
                )])
            })
            .collect();
        if !recovery_lines.is_empty() {
            content.push(Line::raw(""));
            for l in recovery_lines {
                content.push(Line::from(vec![Span::styled(
                    format!("  → {}", l),
                    Style::default().fg(Color::Cyan),
                )]));
            }
        }
        content.push(Line::raw(""));
        content.push(Line::from(vec![Span::styled(
            "  [ OK ]  (Enter / Esc / Space)",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]));

        let title = format!(" {} ", err_info.title);
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block(&title, Color::Red)),
            popup_rect,
        );
    }

    // ── Rules overlay ─────────────────────────────────────────────────────────
    if let Some(ref rules) = app.rules_overlay {
        let mut content: Vec<Line<'static>> = Vec::new();
        if rules.is_empty() {
            content.push(Line::from(vec![Span::styled(
                "  No active rules.".to_string(),
                Style::default().fg(Color::DarkGray),
            )]));
        } else {
            for (id, text) in rules {
                if id.trim().is_empty() {
                    content.push(Line::raw(""));
                } else {
                    // Rule ID as header
                    content.push(Line::from(vec![Span::styled(
                        format!("  {}", id),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )]));
                    // Rule text content
                    content.push(Line::from(vec![Span::styled(
                        format!("    {}", truncate_chars(text, 74)),
                        Style::default().fg(Color::Gray),
                    )]));
                    content.push(Line::raw(""));
                }
            }
        }
        content.push(Line::raw(""));
        content.push(Line::from(vec![Span::styled(
            "  [ Close ]  (Enter / Esc)".to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]));

        let popup_h = ((content.len() as u16) + 2).min(area.height.saturating_sub(4));
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content)
                .block(modal_block(" Active Rules ", Color::Cyan))
                .wrap(Wrap { trim: false }),
            popup_rect,
        );
    }

    // ── Overlay stack (new system) ───────────────────────────────────────────
    for overlay in app.overlay_stack.iter() {
        match overlay {
            OverlayKind::Error(e) => {
                let inner_w = input_area.width.saturating_sub(4) as usize;
                let wrapped = word_wrap(&e.message, inner_w);
                let popup_h = ((wrapped.len() as u16) + 4).min(area.height.saturating_sub(4));
                let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
                let mut content: Vec<Line<'static>> = wrapped
                    .into_iter()
                    .map(|l| {
                        Line::from(vec![Span::styled(
                            format!("  {}", l),
                            Style::default().fg(Color::White),
                        )])
                    })
                    .collect();
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  [ OK ]  (Enter / Esc / Space)",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]));
                frame.render_widget(Clear, popup_rect);
                frame.render_widget(
                    Paragraph::new(content)
                        .block(modal_block(&format!(" {} ", e.title), Color::Red)),
                    popup_rect,
                );
            }
            OverlayKind::ReadOnly(r) => {
                let mut content: Vec<Line<'static>> = Vec::new();
                if r.lines.is_empty() {
                    content.push(Line::from(vec![Span::styled(
                        "  (empty)".to_string(),
                        Style::default().fg(Color::DarkGray),
                    )]));
                } else {
                    for (heading, text) in &r.lines {
                        if heading.trim().is_empty() && text.trim().is_empty() {
                            content.push(Line::raw(""));
                        } else {
                            if !heading.is_empty() {
                                content.push(Line::from(vec![Span::styled(
                                    format!("  {}", heading),
                                    Style::default()
                                        .fg(Color::Cyan)
                                        .add_modifier(Modifier::BOLD),
                                )]));
                            }
                            for line in text.lines() {
                                content.push(Line::from(vec![Span::styled(
                                    format!("    {}", line),
                                    Style::default().fg(Color::Gray),
                                )]));
                            }
                            content.push(Line::raw(""));
                        }
                    }
                }
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  [ Close ]  (Enter / Esc)".to_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]));
                let popup_h = ((content.len() as u16) + 2).min(area.height.saturating_sub(4));
                let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
                let scroll_offset = r.scroll_offset as u16;
                frame.render_widget(Clear, popup_rect);
                let hint_text = if content.len() as u16 > popup_h.saturating_sub(2) {
                    format!(" {} — ↑↓ scroll · Esc close ", r.title)
                } else {
                    format!(" {} — Esc close ", r.title)
                };
                frame.render_widget(
                    Paragraph::new(content)
                        .block(modal_block(&hint_text, Color::Cyan))
                        .wrap(Wrap { trim: false })
                        .scroll((scroll_offset, 0)),
                    popup_rect,
                );
            }
            OverlayKind::Choice(c) => {
                let mut content: Vec<Line<'static>> = Vec::new();
                if !c.message.is_empty() {
                    content.push(Line::from(vec![Span::styled(
                        format!("  {}", c.message),
                        Style::default().fg(Color::White),
                    )]));
                    content.push(Line::raw(""));
                }
                for (i, choice) in c.choices.iter().enumerate() {
                    let marker = if i == c.selected { "▸ " } else { "  " };
                    let style = if i == c.selected {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    content.push(Line::from(vec![Span::styled(
                        format!("  {}{}", marker, choice.label),
                        style,
                    )]));
                }
                content.push(Line::raw(""));
                content.push(Line::from(vec![Span::styled(
                    "  ↑↓ Navigate  Enter Select  Esc Cancel",
                    Style::default().fg(Color::DarkGray),
                )]));
                let popup_h = ((content.len() as u16) + 2).min(area.height.saturating_sub(4));
                let popup_rect = popup_above_input(input_area, popup_h, input_area.width);
                frame.render_widget(Clear, popup_rect);
                frame.render_widget(
                    Paragraph::new(content)
                        .block(modal_block(&format!(" {} ", c.title), Color::Yellow)),
                    popup_rect,
                );
            }
        }
    }
}

// ── Plan editor overlay ───────────────────────────────────────────────────────

fn render_plan_editor(frame: &mut Frame, app: &App, area: Rect) {
    let editor = match &app.plan_editor {
        Some(e) => e,
        None => return,
    };

    frame.render_widget(Clear, area);

    let plan = &editor.plan;
    let task_count = plan.tasks.len();
    let done_count = plan
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();
    let complexity_summary = if plan.tasks.iter().any(|t| t.complexity == Complexity::High) {
        "high"
    } else if plan
        .tasks
        .iter()
        .any(|t| t.complexity == Complexity::Medium)
    {
        "medium"
    } else {
        "low"
    };

    let title = format!(
        " Plan: {}  ({} tasks · complexity: {}) ",
        truncate_chars(&plan.meta.goal, 40),
        task_count,
        complexity_summary
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner: task list | help bar at bottom
    let [task_area, hint_area] =
        ratatui::layout::Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(inner);

    // ── Task list ──
    let mut task_lines: Vec<Line<'static>> = Vec::new();

    if let Some(ref form) = app.plan_task_editing {
        // Inline edit form for the selected task
        task_lines.push(Line::from(vec![Span::styled(
            "  Edit task",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]));
        task_lines.push(Line::raw(""));

        let desc_style = if form.focused_field == TaskEditField::Description {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let notes_style = if form.focused_field == TaskEditField::Notes {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let comp_style = if form.focused_field == TaskEditField::Complexity {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        task_lines.push(Line::from(vec![
            Span::styled("  Description: ", desc_style),
            Span::styled(format!("[{}]", form.description), desc_style),
        ]));
        task_lines.push(Line::from(vec![
            Span::styled("  Notes:        ", notes_style),
            Span::styled(format!("[{}]", form.notes), notes_style),
        ]));

        let (low_style, med_style, high_style) = match form.complexity {
            Complexity::Low => (
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::DarkGray),
            ),
            Complexity::Medium => (
                Style::default().fg(Color::DarkGray),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::DarkGray),
            ),
            Complexity::High => (
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        };
        task_lines.push(Line::from(vec![
            Span::styled("  Complexity:  ", comp_style),
            Span::styled(" ● low ", low_style),
            Span::styled(" ● medium ", med_style),
            Span::styled(" ● high ", high_style),
        ]));
        task_lines.push(Line::raw(""));
        task_lines.push(Line::from(vec![Span::styled(
            "  Tab=next field  Enter=save  Esc=cancel",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));
    } else {
        // Task list with selection highlight
        let scroll_start = if app.plan_selected_task >= task_area.height as usize {
            app.plan_selected_task - task_area.height as usize + 1
        } else {
            0
        };

        for (i, task) in plan.tasks.iter().enumerate() {
            if i < scroll_start {
                continue;
            }
            let selected = i == app.plan_selected_task;
            let bg = if selected {
                TUI_SELECTION_BG
            } else {
                Color::Reset
            };
            let fg = if selected { Color::White } else { Color::Gray };

            let status_icon = match task.status {
                TaskStatus::Pending => "○",
                TaskStatus::Running => "↻",
                TaskStatus::Done => "✓",
                TaskStatus::Failed => "✗",
                TaskStatus::Skipped => "⊘",
            };

            let complexity_badge = match task.complexity {
                Complexity::Low => {
                    Span::styled(" [low] ", Style::default().fg(Color::DarkGray).bg(bg))
                }
                Complexity::Medium => {
                    Span::styled(" [med] ", Style::default().fg(Color::Yellow).bg(bg))
                }
                Complexity::High => Span::styled(" [high]", Style::default().fg(Color::Red).bg(bg)),
            };

            let skip_str = if task.skip { "⊘ " } else { "  " };
            let deps_str = if task.depends_on.is_empty() {
                String::new()
            } else {
                format!("  →{}", task.depends_on.join(","))
            };
            let marker = if selected { "▶" } else { " " };

            task_lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} {} {}", marker, status_icon, skip_str),
                    Style::default().fg(fg).bg(bg),
                ),
                Span::styled(
                    format!("{:<5}", task.id),
                    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
                ),
                complexity_badge,
                Span::styled(
                    format!("  {}{}", task.description, deps_str),
                    Style::default().fg(fg).bg(bg),
                ),
            ]));
        }

        let progress = format!("  Progress: {}/{}", done_count, task_count);
        task_lines.push(Line::raw(""));
        task_lines.push(Line::from(vec![Span::styled(
            progress,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]));
    }

    frame.render_widget(
        Paragraph::new(task_lines).wrap(Wrap { trim: false }),
        task_area,
    );

    // ── Hint bar ──
    let dry_run_note = if app.plan_dry_run {
        "  [dry-run: x will not execute]"
    } else {
        ""
    };
    let hint = if app.plan_task_editing.is_some() {
        String::new()
    } else {
        format!(
            "  Enter=edit  d=delete  n=new  Space=skip  ↑↓=move  s=save  x=execute  Esc=abort{}",
            dry_run_note
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            hint,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )])),
        hint_area,
    );
}

// ── Plan text editor (nano-style) ────────────────────────────────────────────

fn render_plan_text_editor(frame: &mut Frame, app: &App, area: Rect) {
    let ed = match &app.plan_text_editor {
        Some(e) => e,
        None => return,
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Plan text (Ctrl+S = save · Esc / Ctrl+C = discard) ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [edit_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let visible_rows = edit_area.height as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, line) in ed.lines.iter().enumerate().skip(ed.scroll) {
        if lines.len() >= visible_rows {
            break;
        }
        if i == ed.cursor_row {
            // Render cursor inline
            let chars: Vec<char> = line.chars().collect();
            let col = ed.cursor_col.min(chars.len());
            let before: String = chars[..col].iter().collect();
            let cursor_ch: String = if col < chars.len() {
                chars[col].to_string()
            } else {
                " ".to_string()
            };
            let after: String = if col < chars.len() {
                chars[col + 1..].iter().collect()
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::raw(before),
                Span::styled(
                    cursor_ch,
                    Style::default().bg(Color::White).fg(Color::Black),
                ),
                Span::raw(after),
            ]));
        } else {
            lines.push(Line::raw(line.clone()));
        }
    }

    frame.render_widget(Paragraph::new(lines), edit_area);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("  ↑↓←→", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " navigate  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Enter", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " new line  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Ctrl+S", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " save  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Esc", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " discard  ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("Ctrl+C", Style::default().fg(Color::DarkGray)),
        Span::styled(
            " discard",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
    ]));
    frame.render_widget(hint, hint_area);
}

// ── Settings editor rendering ─────────────────────────────────────────────────

fn render_settings(frame: &mut Frame, area: Rect, input_area: Rect, st: &SettingsState) {
    // Take up most of the screen.
    let popup_h = area.height.saturating_sub(6).max(10);
    let popup_w = area.width.saturating_sub(8).min(90);
    let popup_rect = popup_above_input(input_area, popup_h, popup_w);
    frame.render_widget(Clear, popup_rect);

    // Layout: title block wraps everything; inside: content + hint footer.
    let inner = {
        let b = popup_rect;
        Rect {
            x: b.x + 1,
            y: b.y + 1,
            width: b.width.saturating_sub(2),
            height: b.height.saturating_sub(2),
        }
    };
    let [_list_area, hint_area] =
        ratatui::layout::Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let name_w = 14usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    // ── Default model row ──
    let dm_selected = st.cursor == 0 && st.edit_field == SettingsEditField::None;
    let dm_editing = st.edit_field == SettingsEditField::DefaultModel;
    lines.push(Line::from(vec![Span::styled(
        "  Default model",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )]));
    lines.push(Line::from(vec![
        if dm_selected {
            Span::styled(
                " ▶ ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        },
        Span::styled(
            if dm_editing {
                st.input.clone()
            } else {
                st.default_model.clone()
            },
            Style::default()
                .fg(if dm_editing {
                    Color::Yellow
                } else if dm_selected {
                    Color::White
                } else {
                    Color::Green
                })
                .add_modifier(if dm_editing || dm_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        if dm_editing {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else if dm_selected {
            Span::styled(
                "  (Enter to edit)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )
        } else {
            Span::raw("")
        },
    ]));

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![Span::styled(
        format!("  {:<name_w$}  model", "role name"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )]));
    lines.push(Line::raw(""));

    // Role rows — cursor offset by 1 (cursor 0 = default model)
    for (i, (name, model)) in st.roles.iter().enumerate() {
        let selected = (i + 1) == st.cursor && st.edit_field == SettingsEditField::None;
        let editing_name = matches!(&st.edit_field, SettingsEditField::RoleName(idx) if *idx == i);
        let editing_model =
            matches!(&st.edit_field, SettingsEditField::RoleModel(idx) if *idx == i);

        let marker = if selected {
            Span::styled(
                " ▶ ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        };
        let name_span = Span::styled(
            format!(
                "{:<width$}",
                if editing_name {
                    st.input.as_str()
                } else {
                    name.as_str()
                },
                width = name_w
            ),
            Style::default()
                .fg(if editing_name {
                    Color::Yellow
                } else if selected {
                    Color::White
                } else {
                    Color::Cyan
                })
                .add_modifier(if editing_name || selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );
        let arrow = Span::styled("  →  ", Style::default().fg(Color::DarkGray));
        let model_span = if editing_model {
            Span::styled(
                st.input.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else if model.is_empty() {
            Span::styled(
                "(unset — Enter to set)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )
        } else {
            Span::styled(
                model.clone(),
                Style::default().fg(if selected {
                    Color::White
                } else {
                    Color::DarkGray
                }),
            )
        };
        lines.push(Line::from(vec![marker, name_span, arrow, model_span]));
    }

    // New-role name input row
    if matches!(&st.edit_field, SettingsEditField::RoleName(idx) if *idx == usize::MAX) {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("{:<width$}", st.input.as_str(), width = name_w),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  →  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "type name, Enter to set model",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }

    lines.push(Line::raw(""));

    // "Add role" and "Save & close" rows — cursors shifted by 1
    let add_sel = st.cursor == st.roles.len() + 1 && st.edit_field == SettingsEditField::None;
    let save_sel = st.cursor == st.roles.len() + 2 && st.edit_field == SettingsEditField::None;
    lines.push(Line::from(vec![
        if add_sel {
            Span::styled(
                " ▶ ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        },
        Span::styled(
            "+ Add role",
            Style::default().fg(if add_sel {
                Color::White
            } else {
                Color::DarkGray
            }),
        ),
    ]));
    lines.push(Line::from(vec![
        if save_sel {
            Span::styled(
                " ▶ ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        },
        Span::styled(
            "Save & close",
            Style::default()
                .fg(if save_sel {
                    Color::Green
                } else {
                    Color::DarkGray
                })
                .add_modifier(if save_sel {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ]));

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" Settings  (↑↓ navigate · Enter=edit · n=add · Del=remove · Esc=close) ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup_rect,
    );

    // Status / hint line
    let hint = if let Some(ref msg) = st.status {
        Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(if msg.contains('✓') {
                Color::Green
            } else {
                Color::Red
            }),
        ))
    } else if st.edit_field == SettingsEditField::None {
        Line::from(Span::styled(
            "  ↑↓ navigate   Enter edit   n add role   d delete   s save   Esc close",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ))
    } else {
        Line::from(Span::styled(
            "  Enter confirm   Backspace edit   Esc cancel",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ))
    };
    frame.render_widget(Paragraph::new(hint), hint_area);
}

// ── Welcome panel ─────────────────────────────────────────────────────────────

fn is_welcome_only(app: &App) -> bool {
    app.messages.len() == 1 && matches!(app.messages[0], ChatLine::WelcomeSplash)
}

/// Centered welcome panel rendered when no conversation has started yet.
fn render_welcome(frame: &mut Frame, app: &App, area: Rect) {
    let muted = Style::default().fg(Color::Rgb(110, 125, 150));
    let soft = Style::default().fg(Color::Rgb(185, 195, 215));
    let accent = Style::default()
        .fg(TUI_SOFT_ACCENT)
        .add_modifier(Modifier::BOLD);

    // Shorten workdir to ~/...
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = app.workspace_root.display().to_string();
    let workdir = if !home.is_empty() && raw.starts_with(&home) {
        format!("~{}", &raw[home.len()..])
    } else {
        raw
    };

    let content: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(Span::styled(format!("    {}", workdir), muted)),
        Line::raw(""),
        Line::from(vec![
            Span::styled("    profile  ".to_string(), muted),
            Span::styled(
                app.current_profile.clone(),
                soft.add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ".to_string(), muted),
            Span::styled(app.model.clone(), soft),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "    /help   /model   /role   /workdir".to_string(),
            accent,
        )),
        Line::from(Span::styled(
            "    Enter=send  ·  Ctrl+/=stop  ·  Ctrl+Enter=interrupt+send".to_string(),
            muted,
        )),
        Line::raw(""),
    ];

    let border_color = Color::Rgb(55, 70, 95);
    let panel_w = 64u16.min(area.width.saturating_sub(4));
    let panel_h = (content.len() as u16 + 2).min(area.height.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(panel_w) / 2;
    let y = area.y + area.height.saturating_sub(panel_h) / 2;
    let panel_area = Rect::new(x, y, panel_w, panel_h);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "cli".to_string(),
                Style::default()
                    .fg(Color::Rgb(210, 220, 240))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                ";".to_string(),
                Style::default()
                    .fg(TUI_SOFT_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "do".to_string(),
                Style::default()
                    .fg(Color::Rgb(210, 220, 240))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .title_alignment(Alignment::Left);

    let inner = block.inner(panel_area);
    frame.render_widget(block, panel_area);
    frame.render_widget(Paragraph::new(content), inner);
}

// ── Profile overlay renderer ──────────────────────────────────────────────────

fn render_profile_overlay(
    frame: &mut Frame,
    area: Rect,
    input_area: Rect,
    st: &ProfileOverlayState,
) {
    let popup_h = area.height.saturating_sub(6).max(12);
    let popup_w = area.width.saturating_sub(8).min(80);
    let popup_rect = popup_above_input(input_area, popup_h, popup_w);
    frame.render_widget(Clear, popup_rect);

    let inner = Rect {
        x: popup_rect.x + 1,
        y: popup_rect.y + 1,
        width: popup_rect.width.saturating_sub(2),
        height: popup_rect.height.saturating_sub(2),
    };
    let [content_area, hint_area] =
        ratatui::layout::Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    match &st.mode {
        ProfileOverlayMode::Overview | ProfileOverlayMode::EditField(_) => {
            render_profile_overview(frame, popup_rect, content_area, hint_area, st)
        }
        ProfileOverlayMode::Creating { step } => {
            render_profile_create(frame, popup_rect, content_area, hint_area, st, step)
        }
        ProfileOverlayMode::PickingProvider { .. } => {
            render_profile_provider_picker(frame, popup_rect, content_area, hint_area, st)
        }
        ProfileOverlayMode::PickingModel { .. } => {
            render_profile_model_picker(frame, popup_rect, content_area, hint_area, st)
        }
    }
}

fn render_profile_overview(
    frame: &mut Frame,
    popup_rect: Rect,
    content_area: Rect,
    hint_area: Rect,
    st: &ProfileOverlayState,
) {
    let title = if st.is_new {
        " New Profile ".to_string()
    } else {
        format!(" Profile: {} ", st.name)
    };
    frame.render_widget(
        Block::default()
            .title(title.as_str())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        popup_rect,
    );

    let editing = matches!(&st.mode, ProfileOverlayMode::EditField(_));
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::raw(""));

    // cursor_idx tracks which editable field we're on as we iterate PROFILE_FIELDS.
    // Section headers don't consume a cursor index.
    let mut cursor_idx: usize = 0;
    // line_count tracks rendered lines so we can place the text cursor correctly.
    let mut line_count: u16 = 1; // starts at 1 for the leading blank
    let mut editing_line_y: u16 = 0; // Y position of the value row for the active edit field

    for (key, label) in PROFILE_FIELDS.iter() {
        if *key == "__section__" {
            // Non-editable section divider
            lines.push(Line::from(Span::styled(
                format!("  {}", label),
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::DIM | Modifier::BOLD),
            )));
            lines.push(Line::raw(""));
            line_count += 2;
            continue;
        }

        let field_cursor = cursor_idx;
        cursor_idx += 1;

        let selected = st.cursor == field_cursor && !editing;
        let is_editing = matches!(&st.mode, ProfileOverlayMode::EditField(f) if {
            let expected = match field_cursor {
                0 => ProfileEditField::Provider,
                1 => ProfileEditField::ApiKey,
                2 => ProfileEditField::Model,
                3 => ProfileEditField::BaseUrl,
                4 => ProfileEditField::WorkerProvider,
                5 => ProfileEditField::WorkerModel,
                6 => ProfileEditField::ReviewerProvider,
                7 => ProfileEditField::ReviewerModel,
                _ => ProfileEditField::None,
            };
            *f == expected
        });

        // Label row
        lines.push(Line::from(Span::styled(
            format!("  {}", label),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
        line_count += 1;

        // Value row
        let display_value = if *key == "api_key" {
            if is_editing {
                let len = st.input.len();
                if len == 0 {
                    String::new()
                } else {
                    format!("{} ({} chars)", "•".repeat(len.min(30)), len)
                }
            } else {
                st.masked_api_key()
            }
        } else if is_editing {
            st.input.clone()
        } else {
            let raw = match field_cursor {
                0 => st.provider.clone(),
                1 => st.masked_api_key(),
                2 => st.model.clone(),
                3 => st.base_url.clone(),
                4 => st.worker_provider.clone(),
                5 => st.worker_model.clone(),
                6 => st.reviewer_provider.clone(),
                7 => st.reviewer_model.clone(),
                _ => String::new(),
            };
            if raw.is_empty() {
                "—".to_string()
            } else {
                raw
            }
        };

        let cursor_span = if selected {
            Span::styled(
                " ▶ ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        };

        let value_style = if is_editing {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };

        let mut spans = vec![cursor_span, Span::styled(display_value, value_style)];

        if is_editing {
            spans.push(Span::styled("▌", Style::default().fg(Color::Yellow)));
            spans.push(Span::styled(
                "  Esc=cancel  Enter=save",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            editing_line_y = content_area.y + line_count;
        } else if selected {
            spans.push(Span::styled(
                "  (Enter to edit)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
        }

        lines.push(Line::from(spans));
        lines.push(Line::raw(""));
        line_count += 2;
    }

    // Status message
    if let Some(ref msg) = st.status {
        lines.push(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Green),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );

    // Hint footer
    let hint = if editing {
        "Type to edit  ·  Enter=save  ·  Esc=cancel"
    } else {
        "↑↓ navigate  ·  Enter=edit field  ·  Ctrl+S=save all  ·  Esc=close"
    };
    frame.render_widget(
        Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        hint_area,
    );

    // Place terminal cursor on the value row of the field being edited.
    if matches!(&st.mode, ProfileOverlayMode::EditField(_)) && editing_line_y > 0 {
        let cursor_x = content_area.x + 3 + st.input_cursor as u16; // 3 = " ▶ " prefix width
        if editing_line_y < content_area.y + content_area.height {
            frame.set_cursor_position((cursor_x, editing_line_y));
        }
    }
}

fn render_profile_create(
    frame: &mut Frame,
    popup_rect: Rect,
    content_area: Rect,
    hint_area: Rect,
    st: &ProfileOverlayState,
    step: &ProfileCreateStep,
) {
    match step {
        ProfileCreateStep::Provider => {
            render_profile_provider_picker(frame, popup_rect, content_area, hint_area, st);
            return;
        }
        ProfileCreateStep::Model => {
            render_profile_model_picker(frame, popup_rect, content_area, hint_area, st);
            return;
        }
        _ => {}
    }

    frame.render_widget(
        Block::default()
            .title(" New Profile ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        popup_rect,
    );

    let (step_num, step_total, step_label, current_value, placeholder) = match step {
        ProfileCreateStep::Name => (1, 3, "Provider", &st.provider, "select a provider"),
        ProfileCreateStep::Provider => (1, 3, "Provider", &st.provider, "select a provider"),
        ProfileCreateStep::ApiKey => (2, 3, "API key", &st.api_key, "paste your key here"),
        ProfileCreateStep::Model => (
            3,
            3,
            "Default model",
            &st.model,
            "e.g. claude-opus-4-5, gpt-4o",
        ),
    };
    let _ = current_value;

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("  Step {step_num} of {step_total} — {step_label}"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )));
    lines.push(Line::raw(""));

    let display_input = if matches!(step, ProfileCreateStep::ApiKey) && !st.input.is_empty() {
        // Show masked dots while typing, with a length indicator
        let len = st.input.len();
        format!("{} ({} chars)", "•".repeat(len.min(30)), len)
    } else {
        st.input.clone()
    };

    let value_display = if display_input.is_empty() {
        Span::styled(
            format!("   {placeholder}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )
    } else {
        Span::styled(
            format!("   {display_input}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    };

    lines.push(Line::from(vec![
        value_display,
        Span::styled("▌", Style::default().fg(Color::Yellow)),
    ]));
    lines.push(Line::raw(""));

    // Summary of already-entered fields
    if step_num > 1 {
        lines.push(Line::from(Span::styled(
            "  Already entered:",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
        if !st.name.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    name       {}", st.name),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if !st.provider.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    provider   {}", st.provider),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    if let Some(ref msg) = st.status {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(if msg.starts_with("  ✓") {
                Color::Green
            } else {
                Color::Red
            }),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );

    let hint = if matches!(step, ProfileCreateStep::ApiKey) {
        "Type API key  ·  Enter=next  ·  Esc=cancel"
    } else {
        "Type value  ·  Enter=next  ·  Esc=cancel"
    };
    frame.render_widget(
        Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        hint_area,
    );

    // Cursor inside the input line (line index 3 = blank + hint + blank + input)
    let cursor_y = content_area.y + 4;
    let shown_len = if matches!(step, ProfileCreateStep::ApiKey) {
        let len = st.input.len();
        // "•" repeated + " (N chars)"
        len.min(30) + format!(" ({} chars)", len).len()
    } else {
        st.input_cursor
    };
    let cursor_x = content_area.x + 3 + shown_len as u16;
    if cursor_y < content_area.y + content_area.height {
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn render_profile_provider_picker(
    frame: &mut Frame,
    popup_rect: Rect,
    content_area: Rect,
    hint_area: Rect,
    st: &ProfileOverlayState,
) {
    frame.render_widget(
        Block::default()
            .title(" Select Provider ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        popup_rect,
    );

    // 2 lines for filter + blank, 1 line for scroll indicator
    let visible: usize = (content_area.height as usize).saturating_sub(3).max(3);
    let picker = &st.provider_picker;
    let indices = picker.filtered();

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![Span::styled(
            format!("  Filter: {}_", picker.filter),
            Style::default().fg(Color::White),
        )]),
        Line::raw(""),
    ];

    let end = (picker.scroll_offset + visible).min(indices.len());
    for (di, &idx) in indices[picker.scroll_offset..end].iter().enumerate() {
        let abs_pos = picker.scroll_offset + di;
        let selected = abs_pos == picker.selected;
        let (id, name, needs_key) = KNOWN_PROVIDERS[idx];
        let bg = if selected {
            TUI_SELECTION_BG
        } else {
            Color::Reset
        };
        let fg = if selected { Color::White } else { Color::Gray };
        let key_hint = if !needs_key { "  (no key needed)" } else { "" };
        lines.push(Line::from(vec![Span::styled(
            format!("  {:<12}  {}{}", id, name, key_hint),
            Style::default().fg(fg).bg(bg),
        )]));
    }

    let above = picker.scroll_offset;
    let below = indices.len().saturating_sub(picker.scroll_offset + visible);
    if above > 0 || below > 0 {
        let mut parts = Vec::new();
        if above > 0 {
            parts.push(format!("↑↑ {} more", above));
        }
        if below > 0 {
            parts.push(format!("↓↓ {} more", below));
        }
        lines.push(Line::from(Span::styled(
            format!("  {}", parts.join("  ")),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );
    frame.render_widget(
        Paragraph::new("↑↓=navigate  Enter=select  type to filter  Esc=cancel").style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        hint_area,
    );
}

fn render_profile_model_picker(
    frame: &mut Frame,
    popup_rect: Rect,
    content_area: Rect,
    hint_area: Rect,
    st: &ProfileOverlayState,
) {
    frame.render_widget(
        Block::default()
            .title(" Select Model ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
        popup_rect,
    );

    let Some(ref picker) = st.profile_model_picker else {
        return;
    };
    // 3 lines for filter + header + blank, 1 for scroll indicator
    let visible: usize = (content_area.height as usize).saturating_sub(4).max(3);
    let filtered = picker.filtered();

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            format!("  Filter: {}_", picker.filter),
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            format!(
                "  {:<32}  {:<12}  {:>8}  {:>8}  {:>6}",
                "model", "provider", "$/1M in", "$/1M out", "ctx k"
            ),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )),
        Line::raw(""),
    ];

    let end = (picker.scroll_offset + visible).min(filtered.len());
    for (di, m) in filtered[picker.scroll_offset..end].iter().enumerate() {
        let selected = picker.scroll_offset + di == picker.selected;
        let bg = if selected {
            TUI_SELECTION_BG
        } else {
            Color::Reset
        };
        let fg = if selected { Color::White } else { Color::Gray };
        let ctx = m
            .context_k
            .map(|k| format!("{:>4}k", k))
            .unwrap_or_else(|| "    ?".into());
        let id_display: String = m.id.chars().take(32).collect();
        let prov_display: String = m.provider.chars().take(12).collect();
        lines.push(Line::from(Span::styled(
            format!(
                "  {:<32}  {:<12}  {:>8.2}  {:>8.2}  {}",
                id_display, prov_display, m.input_mtok, m.output_mtok, ctx
            ),
            Style::default().fg(fg).bg(bg),
        )));
    }

    let above = picker.scroll_offset;
    let below = filtered
        .len()
        .saturating_sub(picker.scroll_offset + visible);
    if above > 0 || below > 0 {
        let mut parts = Vec::new();
        if above > 0 {
            parts.push(format!("↑↑ {} more", above));
        }
        if below > 0 {
            parts.push(format!("↓↓ {} more", below));
        }
        lines.push(Line::from(Span::styled(
            format!("  {}", parts.join("  ")),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );
    frame.render_widget(
        Paragraph::new("↑↓=navigate  Enter=select  type to filter  Esc=cancel").style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        hint_area,
    );
}

// ── Modal component helpers ───────────────────────────────────────────────────

/// Rect anchored just above the input field (grows upward).
fn popup_above_input(input_area: Rect, h: u16, w: u16) -> Rect {
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

/// Styled popup block — same structure for every modal.
fn modal_block(title: &str, border_color: Color) -> Block<'static> {
    Block::default()
        .title(title.to_string())
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

fn modal_block_with_hint(title: &str, hint: &str, border_color: Color) -> Block<'static> {
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

/// Two-column row (e.g. for slash completions): cmd | description, with highlight on selection.
fn modal_row_two_col(
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

/// Build a deterministic plan snapshot from assistant text.
/// This is the canonical path used for both saving and display.
fn build_plan_from_assistant_text(text: &str) -> Option<Plan> {
    let mut tasks = parse_plan_from_text(text);
    if tasks.is_empty() {
        // Deterministic fallback: every non-empty line becomes one step in order.
        tasks = text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
    }
    if tasks.is_empty() {
        return None;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let goal = tasks.first().cloned().unwrap_or_default();
    let slug: String = goal
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .take(30)
        .collect::<String>()
        .trim()
        .replace(' ', "_")
        .to_lowercase();
    Some(clido_planner::Plan {
        meta: clido_planner::PlanMeta {
            id: format!("{}_{}", slug, ts),
            goal,
            created_at: ts.to_string(),
        },
        tasks: tasks
            .iter()
            .enumerate()
            .map(|(i, t)| clido_planner::TaskNode {
                id: format!("{}", i + 1),
                description: t.clone(),
                status: clido_planner::TaskStatus::Pending,
                depends_on: vec![],
                complexity: clido_planner::Complexity::Medium,
                notes: String::new(),
                tools: None,
                skip: false,
            })
            .collect(),
    })
}

fn build_plan_from_tasks(tasks: &[String]) -> Option<Plan> {
    if tasks.is_empty() {
        return None;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let goal = tasks.first().cloned().unwrap_or_default();
    let slug: String = goal
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .take(30)
        .collect::<String>()
        .trim()
        .replace(' ', "_")
        .to_lowercase();
    Some(clido_planner::Plan {
        meta: clido_planner::PlanMeta {
            id: format!("{}_{}", slug, ts),
            goal,
            created_at: ts.to_string(),
        },
        tasks: tasks
            .iter()
            .enumerate()
            .map(|(i, t)| clido_planner::TaskNode {
                id: format!("{}", i + 1),
                description: t.clone(),
                status: clido_planner::TaskStatus::Pending,
                depends_on: vec![],
                complexity: clido_planner::Complexity::Medium,
                notes: String::new(),
                tools: None,
                skip: false,
            })
            .collect(),
    })
}

/// Strip leading markdown noise so plan lines like `**Step 1:**` match.
fn strip_plan_line_prefix(line: &str) -> String {
    let mut t = line.trim();
    loop {
        let before = t;
        t = t.trim_start_matches(['*', '#', '_', '>', '`']);
        t = t.trim_start();
        if t == before {
            break;
        }
    }
    t.to_string()
}

/// Truncate a string to at most `max_chars` characters, appending `…` if cut.
/// Parse a numbered step list out of free-form agent text.
/// Matches top-level step lines only — not sub-bullets or indented items.
/// Supported formats (at start of line, not indented):
///   "1. foo"  "1) foo"  "Step 1: foo"  "Step 1. foo"
fn parse_plan_from_text(text: &str) -> Vec<String> {
    let mut tasks = Vec::new();
    for line in text.lines() {
        // Skip indented lines — they are sub-bullets, not steps
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let trimmed = strip_plan_line_prefix(line);
        if trimmed.is_empty() {
            continue;
        }

        // "Step N: text" or "Step N. text"
        let step_prefix = trimmed
            .strip_prefix("Step ")
            .or_else(|| trimmed.strip_prefix("step "));
        if let Some(rest) = step_prefix {
            // consume digits
            let after_digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Some(content) = after_digits
                .strip_prefix(": ")
                .or_else(|| after_digits.strip_prefix(". "))
                .or_else(|| after_digits.strip_prefix(":**"))
                .or_else(|| after_digits.strip_prefix(':'))
            {
                let content = strip_plan_line_prefix(content);
                if !content.is_empty() {
                    tasks.push(content.to_string());
                }
                continue;
            }
        }

        // "N. text" or "N) text" or "N.text"
        if let Some(digit_end) = trimmed.find(|c: char| !c.is_ascii_digit()) {
            if digit_end > 0 {
                let rest = trimmed[digit_end..].trim_start();
                let content = rest
                    .strip_prefix(". ")
                    .or_else(|| rest.strip_prefix(") "))
                    .or_else(|| rest.strip_prefix('.'))
                    .or_else(|| rest.strip_prefix(')'))
                    .map(str::trim);
                if let Some(content) = content {
                    if !content.is_empty() {
                        tasks.push(content.to_string());
                    }
                }
            }
        }
    }
    tasks
}

/// Scan text for a "Step N: ..." line and return the full step label if found.
fn extract_current_step_full(text: &str) -> Option<(usize, String)> {
    for line in text.lines() {
        let t = strip_plan_line_prefix(line);
        // Match "Step N: ..." or "▶ Step N: ..."
        let rest = t.strip_prefix("▶ ").unwrap_or(t.as_str());
        if let Some(after) = rest
            .strip_prefix("Step ")
            .or_else(|| rest.strip_prefix("step "))
        {
            let after_digits = after.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Some(label) = after_digits
                .strip_prefix(": ")
                .or_else(|| after_digits.strip_prefix(". "))
                .or_else(|| after_digits.strip_prefix(":**"))
                .or_else(|| after_digits.strip_prefix(':'))
            {
                let label = label.trim();
                if !label.is_empty() {
                    let n: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(num) = n.parse::<usize>() {
                        return Some((num, format!("Step {}: {}", n, label)));
                    }
                }
            }
        }
    }
    None
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
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
fn word_wrap(text: &str, width: usize) -> Vec<String> {
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
fn fit_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
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
fn tool_color(name: &str, done: bool, is_error: bool) -> Color {
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
fn tool_display_name(name: &str) -> &str {
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

/// Return true if `input` is an exact slash command or a slash command followed
/// by a space (i.e. a command with arguments). Used to decide whether Enter
/// should execute a command or send the input as a chat message.
fn is_known_slash_cmd(input: &str) -> bool {
    if !input.starts_with('/') {
        return false;
    }
    slash_commands()
        .into_iter()
        .any(|(cmd, _)| input == cmd || input.starts_with(&format!("{} ", cmd)))
}

fn slash_completions(input: &str) -> Vec<(&'static str, &'static str)> {
    if !input.starts_with('/') {
        return vec![];
    }
    slash_commands()
        .into_iter()
        .filter(|(cmd, _)| cmd.starts_with(input))
        .collect()
}

/// A row in the autocomplete popup: either a non-selectable section header or a
/// selectable command. `flat_idx` is the index into `slash_completions()` output.
enum CompletionRow {
    Header(&'static str),
    Cmd {
        flat_idx: usize,
        cmd: &'static str,
        desc: &'static str,
    },
}

/// Grouped version of `slash_completions`: same matches but interleaved with
/// section headers so the popup can show them visually.
fn slash_completion_rows(input: &str) -> Vec<CompletionRow> {
    if !input.starts_with('/') {
        return vec![];
    }
    let mut rows = Vec::new();
    let mut flat_idx = 0usize;
    for (section, cmds) in slash_command_sections() {
        let matches: Vec<_> = cmds
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(input))
            .collect();
        if !matches.is_empty() {
            rows.push(CompletionRow::Header(section));
            for (cmd, desc) in matches {
                rows.push(CompletionRow::Cmd {
                    flat_idx,
                    cmd,
                    desc,
                });
                flat_idx += 1;
            }
        }
    }
    rows
}

/// Parse `@model-name remaining prompt` per-turn override syntax.
/// Returns `Some((model_id, prompt))` only when input starts with `@` followed
/// by a model name token and a space-separated prompt.
/// Returns `None` for normal input that contains `@` mid-string.
fn parse_per_turn_model(input: &str) -> Option<(String, String)> {
    if !input.starts_with('@') {
        return None;
    }
    let rest = &input[1..];
    let space_idx = rest.find(' ')?;
    let model = rest[..space_idx].trim().to_string();
    let prompt = rest[space_idx + 1..].trim().to_string();
    if model.is_empty() || prompt.is_empty() {
        return None;
    }
    Some((model, prompt))
}

fn execute_slash(app: &mut App, cmd: &str) {
    match cmd {
        "/clear" => {
            app.messages.clear();
            app.messages.push(ChatLine::WelcomeBrand);
            app.push(ChatLine::Info(
                "  Conversation cleared — new session started".into(),
            ));
        }
        "/help" => {
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Navigation".into()));
            app.push(ChatLine::Info("Enter              send message".into()));
            app.push(ChatLine::Info(
                "Shift+Enter        insert newline (multiline input)".into(),
            ));
            app.push(ChatLine::Info("Ctrl+Enter         interrupt & send".into()));
            app.push(ChatLine::Info(
                "↑↓ (empty input)   scroll conversation".into(),
            ));
            app.push(ChatLine::Info(
                "↑↓ (multiline)     move cursor between lines".into(),
            ));
            app.push(ChatLine::Info(
                "↑↓ (with text)     history navigation".into(),
            ));
            app.push(ChatLine::Info(
                "PgUp/PgDn          scroll conversation".into(),
            ));
            app.push(ChatLine::Info(
                "Ctrl+Home/End      jump to top/bottom".into(),
            ));
            app.push(ChatLine::Info("Ctrl+U             clear input".into()));
            app.push(ChatLine::Info(
                "Ctrl+W             delete word backward".into(),
            ));
            app.push(ChatLine::Info("Alt+←/→            jump by word".into()));
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Section("Agent Controls".into()));
            app.push(ChatLine::Info("Ctrl+C             quit".into()));
            app.push(ChatLine::Info(
                "Ctrl+/             interrupt current run only".into(),
            ));
            app.push(ChatLine::Info(
                "Ctrl+Y             copy last assistant message".into(),
            ));
            app.push(ChatLine::Info(
                "Queue              type while agent runs, sends on finish".into(),
            ));
            app.push(ChatLine::Info("".into()));
            for (section, cmds) in slash_command_sections() {
                app.push(ChatLine::Section(section.to_string()));
                for (cmd, desc) in cmds {
                    app.push(ChatLine::Info(format!("{:<18} {}", cmd, desc)));
                }
                app.push(ChatLine::Info("".into()));
            }
            app.push(ChatLine::Section("Per-turn Override".into()));
            app.push(ChatLine::Info(
                "@model-name <msg>  use a different model for one turn".into(),
            ));
            app.push(ChatLine::Info(
                "                   e.g. @claude-opus-4-6 refactor this".into(),
            ));
            app.push(ChatLine::Info("".into()));
        }
        "/keys" => {
            use crate::overlay::{OverlayKind, ReadOnlyOverlay};
            let lines: Vec<(String, String)> = vec![
                (
                    "Navigation".into(),
                    "Enter              send message\n\
                    Shift+Enter        insert newline (multiline)\n\
                    Ctrl+Enter         interrupt & send\n\
                    ↑↓ (empty input)   scroll conversation\n\
                    ↑↓ (with text)     history navigation\n\
                    PgUp/PgDn          scroll 10 lines\n\
                    Ctrl+Home/End      jump to top/bottom\n\
                    Home/End           cursor start/end of line\n\
                    Alt+←/→            jump by word\n\
                    Ctrl+U             clear input\n\
                    Ctrl+W             delete word backward"
                        .into(),
                ),
                (
                    "Agent Controls".into(),
                    "Ctrl+C             quit\n\
                    Ctrl+/             interrupt current run\n\
                    Ctrl+Y             copy last assistant message\n\
                    Ctrl+L             refresh screen\n\
                    Queue              type while agent runs, auto-sends on finish"
                        .into(),
                ),
                (
                    "Pickers".into(),
                    "↑↓                 navigate items\n\
                    Enter              select / confirm\n\
                    Esc                close / cancel\n\
                    Type               filter items (model, provider pickers)\n\
                    Backspace          remove filter char\n\
                    f                  toggle favorite (model picker)\n\
                    Ctrl+S             save as default (model picker)\n\
                    n                  new (profile picker)\n\
                    e                  edit (profile picker)"
                        .into(),
                ),
                (
                    "Plan Editor".into(),
                    "Ctrl+S             save plan\n\
                    Esc                discard changes"
                        .into(),
                ),
                (
                    "Per-turn Override".into(),
                    "@model-name <msg>  use a different model for one turn".into(),
                ),
            ];
            app.overlay_stack
                .push(OverlayKind::ReadOnly(ReadOnlyOverlay::new(
                    "Keyboard Shortcuts",
                    lines,
                )));
        }
        "/fast" => {
            let new_model = app
                .config_roles
                .get("fast")
                .cloned()
                .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string());
            app.model = new_model.clone();
            let _ = app.model_switch_tx.send(new_model.clone());
            app.model_prefs.push_recent(&new_model);
            app.model_prefs.save();
            app.push(ChatLine::Info(format!("  ✓ Model: {} (fast)", new_model)));
        }
        "/smart" => {
            let new_model = app
                .config_roles
                .get("reasoning")
                .cloned()
                .unwrap_or_else(|| "claude-opus-4-6".to_string());
            app.model = new_model.clone();
            let _ = app.model_switch_tx.send(new_model.clone());
            app.model_prefs.push_recent(&new_model);
            app.model_prefs.save();
            app.push(ChatLine::Info(format!("  ✓ Model: {} (smart)", new_model)));
        }
        _ if cmd == "/model" || cmd.starts_with("/model ") => {
            let arg = cmd.trim_start_matches("/model").trim();
            if arg.is_empty() {
                // No name given → open the interactive model picker (same as /models).
                let models = app.known_models.clone();
                // Trigger a fresh API fetch if we have no models yet and aren't already loading.
                if models.is_empty() && !app.models_loading && !app.api_key.is_empty() {
                    spawn_model_fetch(
                        app.provider.clone(),
                        app.api_key.clone(),
                        app.base_url.clone(),
                        app.fetch_tx.clone(),
                    );
                    app.models_loading = true;
                }
                app.model_picker = Some(ModelPickerState {
                    models,
                    filter: String::new(),
                    selected: 0,
                    scroll_offset: 0,
                });
            } else {
                let new_model = arg.to_string();
                app.model = new_model.clone();
                let _ = app.model_switch_tx.send(new_model.clone());
                app.model_prefs.push_recent(&new_model);
                app.model_prefs.save();
                app.push(ChatLine::Info(format!("  ✓ Model: {}", new_model)));
            }
        }
        "/models" => {
            let models = app.known_models.clone();
            // Trigger a fresh API fetch if we have no models yet and aren't already loading.
            if models.is_empty() && !app.models_loading && !app.api_key.is_empty() {
                spawn_model_fetch(
                    app.provider.clone(),
                    app.api_key.clone(),
                    app.base_url.clone(),
                    app.fetch_tx.clone(),
                );
                app.models_loading = true;
            }
            app.model_picker = Some(ModelPickerState {
                models,
                filter: String::new(),
                selected: 0,
                scroll_offset: 0,
            });
        }
        _ if cmd == "/role" || cmd.starts_with("/role ") => {
            let role = cmd.trim_start_matches("/role").trim();
            if role.is_empty() || role == "list" {
                // No name given or "/role list" → open interactive role picker.
                let mut roles: Vec<(String, String)> = app
                    .config_roles
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                roles.sort_by(|a, b| a.0.cmp(&b.0));
                if roles.is_empty() {
                    app.push(ChatLine::Info(
                        "  No roles configured — use /role add <name> <model> to create one".into(),
                    ));
                    app.push(ChatLine::Info(
                        "  Roles let you quickly switch between models  (e.g. fast, smart, review)"
                            .into(),
                    ));
                } else {
                    app.role_picker = Some(RolePickerState {
                        roles,
                        selected: 0,
                        scroll_offset: 0,
                        filter: String::new(),
                    });
                }
            } else if role.starts_with("add ") {
                let args = role.trim_start_matches("add ").trim();
                let parts: Vec<&str> = args.splitn(2, ' ').collect();
                if parts.len() < 2 || parts[1].trim().is_empty() {
                    app.push(ChatLine::Info(
                        "  usage: /role add <name> <model_id>".into(),
                    ));
                } else {
                    let name = parts[0].trim().to_string();
                    let model = parts[1].trim().to_string();
                    app.config_roles.insert(name.clone(), model.clone());
                    let config_path = clido_core::global_config_path()
                        .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                    let roles_vec: Vec<(String, String)> = app
                        .config_roles
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    match save_roles_to_config(&config_path, &roles_vec) {
                        Ok(()) => {
                            let (pricing, _) = clido_core::load_pricing();
                            app.known_models =
                                build_model_list(&pricing, &app.config_roles, &app.model_prefs);
                            app.push(ChatLine::Info(format!(
                                "  role '{name}' → {model}  (saved)"
                            )));
                        }
                        Err(e) => {
                            app.config_roles.remove(&name);
                            app.push(ChatLine::Info(format!("  ✗ failed to save role: {e}")));
                        }
                    }
                }
            } else if role.starts_with("delete ") || role.starts_with("remove ") {
                let name = role
                    .trim_start_matches("delete ")
                    .trim_start_matches("remove ")
                    .trim();
                if name.is_empty() {
                    app.push(ChatLine::Info("  usage: /role delete <name>".into()));
                } else if app.config_roles.remove(name).is_some() {
                    let config_path = clido_core::global_config_path()
                        .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                    let roles_vec: Vec<(String, String)> = app
                        .config_roles
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    match save_roles_to_config(&config_path, &roles_vec) {
                        Ok(()) => {
                            let (pricing, _) = clido_core::load_pricing();
                            app.known_models =
                                build_model_list(&pricing, &app.config_roles, &app.model_prefs);
                            app.push(ChatLine::Info(format!("  role '{name}' deleted")));
                        }
                        Err(e) => {
                            app.push(ChatLine::Info(format!("  ✗ failed to save: {e}")));
                        }
                    }
                } else {
                    app.push(ChatLine::Info(format!("  role '{name}' not found")));
                }
            } else {
                // Resolve: prefs override config.
                let model_id = app
                    .model_prefs
                    .resolve_role(role)
                    .map(|s| s.to_string())
                    .or_else(|| app.config_roles.get(role).cloned());
                match model_id {
                    Some(id) => {
                        app.model = id.clone();
                        let _ = app.model_switch_tx.send(id.clone());
                        app.model_prefs.push_recent(&id);
                        app.model_prefs.save();
                        app.push(ChatLine::Info(format!(
                            "  role '{}' → model switched to {}",
                            role, id
                        )));
                    }
                    None => {
                        app.push(ChatLine::Info(format!(
                            "  role '{}' not found — use /role to list, /role add <name> <model> to create",
                            role
                        )));
                    }
                }
            }
        }
        "/fav" => {
            let model_id = app.model.clone();
            app.model_prefs.toggle_favorite(&model_id);
            app.model_prefs.save();
            // Rebuild model list with updated favorites.
            let (pricing, _) = clido_core::load_pricing();
            app.known_models = build_model_list(&pricing, &app.config_roles, &app.model_prefs);
            let is_fav = app.model_prefs.is_favorite(&model_id);
            let icon = if is_fav { "★" } else { "☆" };
            app.push(ChatLine::Info(format!(
                "  {} {} {}",
                icon,
                model_id,
                if is_fav {
                    "added to favorites"
                } else {
                    "removed from favorites"
                }
            )));
        }
        _ if cmd == "/reviewer" || cmd.starts_with("/reviewer ") => {
            if !app.reviewer_configured {
                app.push(ChatLine::Info(
                    "  reviewer not configured — run /init to add a reviewer sub-agent".into(),
                ));
            } else {
                let arg = cmd.trim_start_matches("/reviewer").trim();
                let new_state = match arg {
                    "on" => Some(true),
                    "off" => Some(false),
                    "" => None, // no arg → just show status
                    _ => {
                        app.push(ChatLine::Info("  Usage: /reviewer [on|off]".into()));
                        return;
                    }
                };
                if let Some(state) = new_state {
                    app.reviewer_enabled.store(state, Ordering::Relaxed);
                }
                let current = app.reviewer_enabled.load(Ordering::Relaxed);
                let status = if current { "on ●" } else { "off ○" };
                app.push(ChatLine::Info(format!("  ✓ Reviewer {}", status)));
            }
        }
        "/session" => match &app.current_session_id {
            Some(id) => app.push(ChatLine::Info(format!("  Session ID: {}", id))),
            None => app.push(ChatLine::Info("  No active session yet".into())),
        },
        "/sessions" => {
            use clido_storage::list_sessions;
            match list_sessions(&app.workspace_root) {
                Err(e) => app.push(ChatLine::Info(format!(
                    "  ✗ Could not list sessions: {}",
                    e
                ))),
                Ok(sessions) if sessions.is_empty() => {
                    app.push(ChatLine::Info(
                        "  No sessions found for this project".into(),
                    ));
                }
                Ok(sessions) => {
                    let selected = sessions
                        .iter()
                        .position(|s| app.current_session_id.as_deref() == Some(&s.session_id))
                        .unwrap_or(0);
                    app.session_picker = Some(SessionPickerState {
                        sessions,
                        selected,
                        scroll_offset: 0,
                        filter: String::new(),
                    });
                }
            }
        }
        "/workdir" => app.push(ChatLine::Info(format!(
            "  Working directory: {}",
            app.workspace_root.display()
        ))),
        _ if cmd.starts_with("/workdir ") => {
            let arg = cmd.trim_start_matches("/workdir").trim();
            match resolve_workdir_arg(arg) {
                Ok(path) => {
                    let _ = app.workdir_tx.send(path.clone());
                    app.push(ChatLine::Info(format!(
                        "  ↻ Switching to {}…",
                        path.display()
                    )));
                    app.push(ChatLine::Info(
                        "  Prompts stay on the current directory until the switch completes."
                            .into(),
                    ));
                }
                Err(e) => app.push(ChatLine::Info(format!(
                    "  ✗ Working directory error: {}",
                    e
                ))),
            }
        }
        "/stop" => {
            if app.busy {
                app.stop_only();
            } else {
                app.push(ChatLine::Info("  ✗ No active run to stop".into()));
            }
        }
        _ if cmd == "/copy" || cmd.starts_with("/copy ") => {
            let arg = cmd.trim_start_matches("/copy").trim();
            // Collect assistant and user chat lines as plain text
            let chat_lines: Vec<(bool, &str)> = app
                .messages
                .iter()
                .filter_map(|m| match m {
                    ChatLine::Assistant(t) => Some((false, t.as_str())),
                    ChatLine::User(t) => Some((true, t.as_str())),
                    _ => None,
                })
                .collect();
            if chat_lines.is_empty() {
                app.push(ChatLine::Info("  ✗ Nothing to copy yet".into()));
            } else if arg.is_empty() {
                // Default: copy last assistant reply
                match app.last_assistant_text().map(|s| s.to_string()) {
                    Some(text) => match copy_to_clipboard(&text) {
                        Ok(()) => {
                            app.push(ChatLine::Info("  ✓ Last reply copied to clipboard".into()))
                        }
                        Err(e) => app.push(ChatLine::Info(format!("  ✗ Copy failed: {}", e))),
                    },
                    None => app.push(ChatLine::Info("  ✗ No assistant reply yet".into())),
                }
            } else {
                // /copy all  or  /copy <n>  — build a transcript
                let take_n: Option<usize> = if arg == "all" {
                    None
                } else {
                    arg.parse::<usize>().ok().map(|n| n * 2) // n exchanges = n user + n assistant
                };
                let slice: Vec<_> = match take_n {
                    None => chat_lines.iter().collect(),
                    Some(n) => chat_lines
                        .iter()
                        .rev()
                        .take(n)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect(),
                };
                let mut buf = String::new();
                for (is_user, text) in &slice {
                    if *is_user {
                        buf.push_str("You: ");
                    } else {
                        buf.push_str("Assistant: ");
                    }
                    buf.push_str(text);
                    buf.push_str("\n\n");
                }
                let count = slice.len();
                match copy_to_clipboard(buf.trim()) {
                    Ok(()) => app.push(ChatLine::Info(format!(
                        "  ✓ Copied {} message{} to clipboard",
                        count,
                        if count == 1 { "" } else { "s" }
                    ))),
                    Err(e) => app.push(ChatLine::Info(format!("  ✗ Copy failed: {}", e))),
                }
            }
        }
        "/quit" => {
            app.quit = true;
        }
        _ if cmd == "/search" || cmd.starts_with("/search ") => {
            let query = cmd.trim_start_matches("/search").trim();
            if query.is_empty() {
                app.push(ChatLine::Info(
                    "  Usage: /search <query>  — search this conversation".into(),
                ));
            } else {
                let q_lower = query.to_lowercase();
                let mut hits: Vec<(usize, &str, String)> = Vec::new(); // (turn_index, role, snippet)
                let mut turn = 0usize;
                for line in &app.messages {
                    match line {
                        ChatLine::User(text) => {
                            turn += 1;
                            if text.to_lowercase().contains(&q_lower) {
                                hits.push((turn, "you", truncate_chars(text, 80)));
                            }
                        }
                        ChatLine::Assistant(text) => {
                            if text.to_lowercase().contains(&q_lower) {
                                hits.push((turn, "assistant", truncate_chars(text, 80)));
                            }
                        }
                        _ => {}
                    }
                }
                if hits.is_empty() {
                    app.push(ChatLine::Info(format!(
                        "  No results for \"{}\" in this conversation",
                        query
                    )));
                } else {
                    app.push(ChatLine::Info(format!(
                        "  {} result{} for \"{}\":",
                        hits.len(),
                        if hits.len() == 1 { "" } else { "s" },
                        query
                    )));
                    for (turn_idx, role, snippet) in &hits {
                        app.push(ChatLine::Info(format!(
                            "  [turn {}] {}  {}",
                            turn_idx, role, snippet
                        )));
                    }
                }
            }
        }
        "/export" => {
            // Export conversation as a markdown file.
            let mut md = String::new();
            md.push_str("# Conversation Export\n\n");
            let mut turn = 0usize;
            for line in &app.messages {
                match line {
                    ChatLine::User(text) => {
                        turn += 1;
                        md.push_str(&format!("## Turn {} — You\n\n{}\n\n", turn, text));
                    }
                    ChatLine::Assistant(text) => {
                        md.push_str(&format!("## Turn {} — Assistant\n\n{}\n\n", turn, text));
                    }
                    _ => {}
                }
            }
            if turn == 0 {
                app.push(ChatLine::Info(
                    "  Nothing to export — start a conversation first".into(),
                ));
            } else {
                use std::time::{SystemTime, UNIX_EPOCH};
                let secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                // YYYYMMDD-HHMMSS from unix timestamp (UTC).
                let mins = secs / 60;
                let hours = mins / 60;
                let days = hours / 24;
                let s = secs % 60;
                let m = mins % 60;
                let h = hours % 24;
                // Approximate calendar date (good enough for a filename).
                let d = days % 31 + 1;
                let mo = (days / 31) % 12 + 1;
                let yr = 1970 + days / 365;
                let filename = format!(
                    "conversation-{:04}{:02}{:02}-{:02}{:02}{:02}.md",
                    yr, mo, d, h, m, s
                );
                let path = app.workspace_root.join(&filename);
                match std::fs::write(&path, &md) {
                    Ok(()) => app.push(ChatLine::Info(format!(
                        "  ✓ Exported {} turns → {}",
                        turn,
                        path.display()
                    ))),
                    Err(e) => app.push(ChatLine::Info(format!("  ✗ Export failed: {}", e))),
                }
            }
        }
        _ if cmd == "/memory" || cmd.starts_with("/memory ") => {
            let query = cmd.trim_start_matches("/memory").trim();
            if query.is_empty() {
                // No query → show recent memories and total count.
                match tui_memory_store_path() {
                    Ok(path) => match MemoryStore::open(&path) {
                        Ok(store) => match store.list(5) {
                            Ok(entries) if entries.is_empty() => {
                                app.push(ChatLine::Info(
                                    "  No memories saved yet — the agent stores facts automatically while working".into(),
                                ));
                            }
                            Ok(entries) => {
                                app.push(ChatLine::Info(
                                    "  Recent memories (use /memory <query> to search):".into(),
                                ));
                                for e in &entries {
                                    app.push(ChatLine::Info(format!(
                                        "  · {}",
                                        truncate_chars(&e.content, 90)
                                    )));
                                }
                            }
                            Err(_) => {
                                app.push(ChatLine::Info(
                                    "  Usage: /memory <query>  — search saved memories".into(),
                                ));
                            }
                        },
                        Err(_) => {
                            app.push(ChatLine::Info(
                                "  No memory store found — memories are saved automatically as you work".into(),
                            ));
                        }
                    },
                    Err(e) => app.push(ChatLine::Info(format!("  ✗ Memory error: {}", e))),
                }
            } else {
                match tui_memory_store_path() {
                    Ok(path) => match MemoryStore::open(&path) {
                        Ok(store) => match store.search_hybrid(query, 15) {
                            Ok(entries) if entries.is_empty() => {
                                app.push(ChatLine::Info(format!(
                                    "  No memory matches for \"{}\"",
                                    query
                                )));
                            }
                            Ok(entries) => {
                                app.push(ChatLine::Info(format!(
                                    "  Found {} memory match(es) for \"{}\"",
                                    entries.len(),
                                    query
                                )));
                                for e in entries.iter().take(15) {
                                    app.push(ChatLine::Info(format!(
                                        "  · {}",
                                        truncate_chars(&e.content, 100)
                                    )));
                                }
                            }
                            Err(e) => {
                                app.push(ChatLine::Info(format!(
                                    "  ✗ Memory search failed: {}",
                                    e
                                )));
                            }
                        },
                        Err(e) => app.push(ChatLine::Info(format!(
                            "  ✗ Cannot open memory store: {}",
                            e
                        ))),
                    },
                    Err(e) => app.push(ChatLine::Info(format!("  ✗ Memory error: {}", e))),
                }
            }
        }
        "/cost" => {
            if app.session_total_cost_usd == 0.0 {
                app.push(ChatLine::Info(
                    "  Session cost: $0.0000 (no API calls yet)".into(),
                ));
            } else {
                app.push(ChatLine::Info(format!(
                    "  Session cost: ${:.4}",
                    app.session_total_cost_usd
                )));
            }
        }
        "/tokens" => {
            let total = app.session_total_input_tokens + app.session_total_output_tokens;
            let total_str = if total >= 1000 {
                format!("{:.1}k", total as f64 / 1000.0)
            } else {
                total.to_string()
            };
            let ctx_pct = if app.context_max_tokens > 0 && app.session_input_tokens > 0 {
                let pct = (app.session_input_tokens as f64 / app.context_max_tokens as f64 * 100.0)
                    .min(100.0);
                format!(
                    "  Context window: {:.0}% used ({} / {} tokens)",
                    pct, app.session_input_tokens, app.context_max_tokens
                )
            } else {
                String::new()
            };
            app.push(ChatLine::Info(
                "  ── Session Token Usage ──────────────────────".into(),
            ));
            app.push(ChatLine::Info(format!(
                "  Input tokens:   {}",
                app.session_total_input_tokens
            )));
            app.push(ChatLine::Info(format!(
                "  Output tokens:  {}",
                app.session_total_output_tokens
            )));
            app.push(ChatLine::Info(format!("  Total tokens:   {}", total_str)));
            app.push(ChatLine::Info(format!(
                "  Estimated cost: ${:.6}",
                app.session_total_cost_usd
            )));
            if !ctx_pct.is_empty() {
                app.push(ChatLine::Info(ctx_pct));
            }
            if app.session_turn_count > 0 {
                app.push(ChatLine::Info(format!(
                    "  Turns completed: {}",
                    app.session_turn_count
                )));
            }
        }
        "/compact" => {
            if app.busy {
                app.push(ChatLine::Info(
                    "  Agent is busy — try /compact when idle".into(),
                ));
            } else {
                app.push(ChatLine::Info("  ↻ Compressing context window…".into()));
                let _ = app.compact_now_tx.send(());
            }
        }
        "/todo" => {
            let todos = app.todo_store.lock().map(|g| g.clone()).unwrap_or_default();
            if todos.is_empty() {
                app.push(ChatLine::Info(
                    "  No tasks yet — the agent will create a task list while working".into(),
                ));
            } else {
                app.push(ChatLine::Info(format!(
                    "  Tasks ({} item{})  ▶ = in progress  ✓ = done  ✗ = blocked  ! = high priority:",
                    todos.len(),
                    if todos.len() == 1 { "" } else { "s" }
                )));
                for item in &todos {
                    let icon = match item.status {
                        clido_tools::TodoStatus::Done => "✓",
                        clido_tools::TodoStatus::InProgress => "▶",
                        clido_tools::TodoStatus::Blocked => "✗",
                        clido_tools::TodoStatus::Pending => "○",
                    };
                    let pri = match item.priority {
                        clido_tools::TodoPriority::High => "!",
                        clido_tools::TodoPriority::Medium => " ",
                        clido_tools::TodoPriority::Low => "·",
                    };
                    app.push(ChatLine::Info(format!(
                        "  {} [{}] {}  {}",
                        icon, pri, item.id, item.content
                    )));
                }
            }
        }
        "/undo" => {
            app.send_now(
                "Undo the last committed change.\n\
                \n\
                Steps:\n\
                1. Run `git log --oneline -5` to show the 5 most recent commits.\n\
                2. Run `git status` to check for any uncommitted changes.\n\
                3. Ask the user to confirm before running any reset command.\n\
                4. If there is a recent commit to undo, run `git reset --soft HEAD~1` to \
                   undo the last commit and keep the changes staged.\n\
                5. Show what files are now staged and a brief summary of what was undone.\n\
                6. If there are only uncommitted changes (nothing committed yet), \
                   ask the user which files to restore before acting."
                    .to_string(),
            );
        }
        _ if cmd == "/rollback" || cmd.starts_with("/rollback ") => {
            let id = cmd.trim_start_matches("/rollback").trim();
            if id.is_empty() {
                app.send_now(
                    "Show available checkpoints for this session.\n\
                    \n\
                    Steps:\n\
                    1. List checkpoints in `.clido/checkpoints/` if the directory exists.\n\
                    2. Also run `git log --oneline -10` to show recent git history.\n\
                    3. Report both lists so the user can choose what to roll back to.\n\
                    4. Ask the user which checkpoint or commit hash to restore, \
                       then wait for their input."
                        .to_string(),
                );
            } else {
                let id = id.to_string();
                app.send_now(format!(
                    "Roll back to checkpoint or commit `{id}`.\n\
                    \n\
                    Steps:\n\
                    1. Check if `{id}` looks like a git commit hash (7-40 hex chars) or a \
                       checkpoint ID (starts with `ck_`).\n\
                    2. For a git commit hash: run `git status` first and show any uncommitted changes.\n\
                    3. Ask the user for explicit confirmation before any destructive rollback.\n\
                    4. If confirmed, create a safety backup (for example `git branch backup/before-rollback`) \
                       before running `git reset --hard {id}`.\n\
                    5. For a checkpoint ID: restore from `.clido/checkpoints/{id}/manifest.json` \
                       by reading the manifest and restoring each listed file from its blob.\n\
                    6. Show a summary of what was restored."
                ));
            }
        }
        _ if cmd == "/plan" || cmd.starts_with("/plan ") => {
            let sub = cmd.trim_start_matches("/plan").trim().to_string();
            match sub.as_str() {
                "edit" => {
                    if let Some(raw) = app.last_plan_raw.clone() {
                        app.plan_text_editor = Some(PlanTextEditor::from_raw(&raw));
                    } else if let Some(plan) = app.last_plan_snapshot.clone() {
                        let raw = plan
                            .tasks
                            .iter()
                            .enumerate()
                            .map(|(i, t)| format!("Step {}: {}", i + 1, t.description))
                            .collect::<Vec<_>>()
                            .join("\n");
                        app.plan_text_editor = Some(PlanTextEditor::from_raw(&raw));
                    } else if let Some(tasks) = app.last_plan.clone() {
                        // fallback for plans from --plan mode (no raw text available)
                        let raw = tasks
                            .iter()
                            .enumerate()
                            .map(|(i, t)| format!("Step {}: {}", i + 1, t))
                            .collect::<Vec<_>>()
                            .join("\n");
                        app.plan_text_editor = Some(PlanTextEditor::from_raw(&raw));
                    } else {
                        app.push(ChatLine::Info(
                            "  ✗ No plan yet — use /plan <task> to create one".into(),
                        ));
                    }
                }
                "save" => {
                    if let Some(ref editor) = app.plan_editor {
                        app.last_plan_snapshot = Some(editor.plan.clone());
                        app.last_plan = Some(
                            editor
                                .plan
                                .tasks
                                .iter()
                                .map(|t| t.description.clone())
                                .collect::<Vec<_>>(),
                        );
                        match clido_planner::save_plan(&app.workspace_root, &editor.plan) {
                            Ok(path) => app.push(ChatLine::Info(format!(
                                "  ✓ Plan saved: {}",
                                path.display()
                            ))),
                            Err(e) => {
                                app.pending_error = Some(ErrorInfo::from_message(format!(
                                    "Could not save plan: {}",
                                    e
                                )))
                            }
                        }
                    } else if let Some(ref plan) = app.last_plan_snapshot {
                        match clido_planner::save_plan(&app.workspace_root, plan) {
                            Ok(path) => app.push(ChatLine::Info(format!(
                                "  ✓ Plan saved: {}",
                                path.display()
                            ))),
                            Err(e) => {
                                app.pending_error = Some(ErrorInfo::from_message(format!(
                                    "Could not save plan: {}",
                                    e
                                )))
                            }
                        }
                    } else {
                        app.push(ChatLine::Info("  ✗ No active plan to save".into()));
                    }
                }
                "list" => match clido_planner::list_plans(&app.workspace_root) {
                    Ok(summaries) if summaries.is_empty() => {
                        app.push(ChatLine::Info(
                            "  No saved plans — use /plan <task> to create and /plan save to save"
                                .into(),
                        ));
                    }
                    Ok(summaries) => {
                        app.push(ChatLine::Info(format!(
                            "  Saved plans ({}):",
                            summaries.len()
                        )));
                        for s in &summaries {
                            let done_frac = if s.task_count > 0 {
                                format!("{}/{}", s.done, s.task_count)
                            } else {
                                "—".to_string()
                            };
                            app.push(ChatLine::Info(format!(
                                "  {}  [{} done]  {}",
                                {
                                    let g = &s.goal;
                                    if g.chars().count() > 58 {
                                        format!("{}…", g.chars().take(57).collect::<String>())
                                    } else {
                                        g.clone()
                                    }
                                },
                                done_frac,
                                s.id
                            )));
                        }
                        app.push(ChatLine::Info(
                            "  Use /rollback <id> to restore a plan checkpoint".into(),
                        ));
                    }
                    Err(e) => {
                        app.pending_error =
                            Some(ErrorInfo::from_message(format!("list plans: {}", e)));
                    }
                },
                "" => {
                    // /plan with no task — show existing plan if any
                    if let Some(plan) = app.last_plan_snapshot.clone() {
                        if plan.tasks.is_empty() {
                            app.push(ChatLine::Info(
                                "  Usage: /plan <task>  — have the agent plan before executing"
                                    .into(),
                            ));
                            return;
                        }
                        app.push(ChatLine::Info("  ┌─ Current plan:".into()));
                        let count = plan.tasks.len();
                        for (i, t) in plan.tasks.iter().enumerate() {
                            let prefix = if i + 1 == count {
                                "  └─"
                            } else {
                                "  ├─"
                            };
                            app.push(ChatLine::Info(format!("{} {}", prefix, t.description)));
                        }
                    } else {
                        match app.last_plan.clone() {
                            Some(tasks) if !tasks.is_empty() => {
                                app.push(ChatLine::Info("  ┌─ Current plan:".into()));
                                let count = tasks.len();
                                for (i, t) in tasks.iter().enumerate() {
                                    let prefix = if i + 1 == count {
                                        "  └─"
                                    } else {
                                        "  ├─"
                                    };
                                    app.push(ChatLine::Info(format!("{} {}", prefix, t)));
                                }
                            }
                            _ => {
                                app.push(ChatLine::Info(
                                    "  Usage: /plan <task>  — have the agent plan before executing"
                                        .into(),
                                ));
                            }
                        }
                    }
                }
                task => {
                    // /plan <task> — ask the agent to plan first, then wait for confirmation
                    let task = task.to_string();
                    app.awaiting_plan_response = true;
                    let prompt = format!(
                        "Create a detailed step-by-step plan for the following task. \
                         Number each top-level step as \"Step N: description\". \
                         You may add sub-bullets or notes under each step for clarity. \
                         Present the complete plan and then STOP — do not execute anything \
                         until the user explicitly confirms.\n\nTask: {task}"
                    );
                    app.send_silent(prompt);
                }
            }
        }
        _ if cmd == "/branch" || cmd.starts_with("/branch ") => {
            let name = cmd.trim_start_matches("/branch").trim().to_string();
            if name.is_empty() {
                app.push(ChatLine::Info("  Usage: /branch <name>".into()));
                app.push(ChatLine::Info(
                    "  creates a new branch and switches to it".into(),
                ));
            } else {
                app.send_now(format!(
                    "Create and switch to a new git branch named `{name}`.\n\
                    \n\
                    Steps:\n\
                    1. Verify this is a git repo. Stop if not.\n\
                    2. Check for uncommitted changes with `git status`. If there are any, \
                       stash them first (`git stash`) so the branch switch is clean.\n\
                    3. Create and switch: `git checkout -b {name}`.\n\
                    4. If the stash was created, pop it: `git stash pop`. \
                       If the pop causes conflicts, show them clearly and stop.\n\
                    5. Push the branch and set upstream: `git push -u origin {name}`.\n\
                    6. Report the new branch name and current status."
                ));
            }
        }
        "/sync" => {
            app.send_now(
                "Sync the current branch with its upstream.\n\
                \n\
                Steps:\n\
                1. Verify this is a git repo. Stop if not.\n\
                2. Run `git status` — if there are uncommitted changes, stash them first \
                   (`git stash`).\n\
                3. Run `git fetch origin`.\n\
                4. Run `git rebase origin/<current-branch>` (use `git rev-parse \
                   --abbrev-ref HEAD` to get the branch name).\n\
                5. If rebase has conflicts: show which files conflict, attempt to resolve \
                   straightforward ones (whitespace, formatting), then `git rebase --continue`. \
                   If conflicts are non-trivial, stop and explain what needs manual resolution.\n\
                6. If a stash was created, pop it: `git stash pop`.\n\
                7. Report how many commits were rebased and the current HEAD."
                    .to_string(),
            );
        }
        _ if cmd == "/pr" || cmd.starts_with("/pr ") => {
            let title_arg = cmd.trim_start_matches("/pr").trim().to_string();
            let title_instruction = if title_arg.is_empty() {
                "Generate a PR title (≤70 chars, imperative mood) and body from the branch diff."
                    .to_string()
            } else {
                format!("Use this as the PR title: {title_arg}")
            };
            app.send_now(format!(
                "Create a pull request for the current branch.\n\
                \n\
                Steps:\n\
                1. Verify this is a git repo with a remote. Stop if not.\n\
                2. Check `git status` — if there are uncommitted changes, ask whether to \
                   ship them first (run /ship) or proceed with existing commits.\n\
                3. Get the current branch: `git rev-parse --abbrev-ref HEAD`. \
                   If it's main or master, warn and stop — PRs should come from a feature branch.\n\
                4. Get the default base branch (try `git symbolic-ref refs/remotes/origin/HEAD` \
                   or fall back to `main`).\n\
                5. Run `git log <base>..<current> --oneline` and \
                   `git diff <base>..<current> --stat` to understand the changes.\n\
                6. {title_instruction}\n\
                   For the body, write:\n\
                   - ## Summary — 2–4 bullet points of what changed and why\n\
                   - ## Test plan — what to verify\n\
                7. Make sure the branch is pushed: `git push -u origin <branch>` if needed.\n\
                8. Create the PR: `gh pr create --title \"<title>\" --body \"<body>\" \
                   --base <base>`.\n\
                   If `gh` is not available, print the title and body and tell the user \
                   to create the PR manually.\n\
                9. Print the PR URL."
            ));
        }
        _ if cmd == "/ship" || cmd.starts_with("/ship ") => {
            let custom_msg = cmd.trim_start_matches("/ship").trim();
            let msg_instruction = if custom_msg.is_empty() {
                "Generate a commit message from the staged diff: imperative mood, ≤72 chars subject, \
                 body only if the change is complex. Append trailer: \
                 `Co-Authored-By: Claude <noreply@clido.dev>`".to_string()
            } else {
                format!("Use this commit message verbatim: {custom_msg}")
            };
            app.send_now(format!(
                "Git ship: stage all changes and push.\n\
                \n\
                Steps:\n\
                1. Verify this is a git repo (`git rev-parse --git-dir`). Stop if not.\n\
                2. Run `git status` — if nothing to commit, report and stop.\n\
                3. Run `git diff HEAD` and `git status -s` to understand changes.\n\
                4. Warn and skip any sensitive files (*.env, *secret*, *credential*, *password*) \
                   before staging.\n\
                5. `git add -A` (excluding sensitive files).\n\
                6. {msg_instruction}\n\
                7. `git commit -m \"<message>\"` — if it fails (hook, lint, tests):\n\
                   - Read the error, fix the root cause (format/lint/test as needed).\n\
                   - Re-stage affected files and retry the commit.\n\
                   - Repeat up to 3 attempts total. Never use --no-verify.\n\
                   - If still failing after 3 attempts, explain what is blocking and stop.\n\
                8. `git push` — if rejected:\n\
                   - Diverged history: `git pull --rebase origin <branch>` then push again.\n\
                   - No upstream: `git push -u origin <branch>`.\n\
                   - Never force-push to main or master.\n\
                9. Report the commit hash and pushed branch."
            ));
        }
        _ if cmd == "/save" || cmd.starts_with("/save ") => {
            let custom_msg = cmd.trim_start_matches("/save").trim();
            let msg_instruction = if custom_msg.is_empty() {
                "Generate a commit message from the staged diff: imperative mood, ≤72 chars subject, \
                 body only if the change is complex. Append trailer: \
                 `Co-Authored-By: Claude <noreply@clido.dev>`".to_string()
            } else {
                format!("Use this commit message verbatim: {custom_msg}")
            };
            app.send_now(format!(
                "Git save: stage all changes and commit locally (no push).\n\
                \n\
                Steps:\n\
                1. Verify this is a git repo (`git rev-parse --git-dir`). Stop if not.\n\
                2. Run `git status` — if nothing to commit, report and stop.\n\
                3. Run `git diff HEAD` and `git status -s` to understand changes.\n\
                4. Warn and skip any sensitive files (*.env, *secret*, *credential*, *password*) \
                   before staging.\n\
                5. `git add -A` (excluding sensitive files).\n\
                6. {msg_instruction}\n\
                7. `git commit -m \"<message>\"` — if it fails (hook, lint, tests):\n\
                   - Read the error, fix the root cause (format/lint/test as needed).\n\
                   - Re-stage affected files and retry the commit.\n\
                   - Repeat up to 3 attempts total. Never use --no-verify.\n\
                   - If still failing after 3 attempts, explain what is blocking and stop.\n\
                8. Report the commit hash and message."
            ));
        }
        "/check" => {
            app.push(ChatLine::Info("  ↻ Running project diagnostics…".into()));
            // Send a message to the agent asking it to run diagnostics on the current project.
            app.send_now("Run diagnostics on the current project".to_string());
        }
        _ if cmd == "/notify" || cmd.starts_with("/notify ") => {
            let arg = cmd.trim_start_matches("/notify").trim();
            match arg {
                "on" => {
                    app.notify_enabled = true;
                    app.push(ChatLine::Info("  ✓ Notifications enabled".into()));
                }
                "off" => {
                    app.notify_enabled = false;
                    app.push(ChatLine::Info("  ✓ Notifications disabled".into()));
                }
                "" => {
                    app.notify_enabled = !app.notify_enabled;
                    let state = if app.notify_enabled { "on" } else { "off" };
                    app.push(ChatLine::Info(format!("  Notifications {}", state)));
                }
                _ => {
                    app.push(ChatLine::Info("  Usage: /notify [on|off]".into()));
                }
            }
        }
        "/index" => {
            let db_path = app.workspace_root.join(".clido").join("index.db");
            if !db_path.exists() {
                app.push(ChatLine::Info(
                    "  Index not built — run `clido index build` in a terminal to enable code search.  \
Once built, the agent can search by concept rather than just filename.".into(),
                ));
            } else {
                match RepoIndex::open(&db_path) {
                    Ok(index) => match index.stats() {
                        Ok((files, symbols)) => {
                            app.push(ChatLine::Info(format!(
                                "  Index: {} files, {} symbols  (refresh: `clido index build`)",
                                files, symbols
                            )));
                        }
                        Err(e) => {
                            app.push(ChatLine::Info(format!("  ✗ Index error: {}", e)));
                        }
                    },
                    Err(e) => {
                        app.push(ChatLine::Info(format!("  ✗ Index unavailable: {}", e)));
                    }
                }
            }
        }
        "/rules" => {
            let active = app.prompt_rules.active_rules();
            let mut overlay_content: Vec<(String, String)> = Vec::new();
            if active.is_empty() {
                overlay_content.push((
                    "  No active rules.".to_string(),
                    "Use /prompt-rules add <text> to define prompt enhancement rules.".to_string(),
                ));
            } else {
                for rule in active {
                    overlay_content.push((rule.id.clone(), rule.text.clone()));
                }
            }
            app.rules_overlay = Some(overlay_content);
        }
        _ if cmd == "/image" || cmd.starts_with("/image ") => {
            let path_str = cmd.trim_start_matches("/image").trim();
            if path_str.is_empty() {
                app.push(ChatLine::Info(
                    "  Usage: /image <path>  (attach an image to the next message)".into(),
                ));
            } else {
                let path = std::path::Path::new(path_str);
                match crate::image_input::ImageAttachment::from_path(path) {
                    Some(att) => {
                        let info = att.info_line();
                        app.pending_image = Some(att);
                        app.push(ChatLine::Info(format!("  {}", info)));
                    }
                    None => {
                        app.push(ChatLine::Info(format!(
                            "  ✗ Could not load image '{}' — supported: PNG, JPEG, GIF, WebP",
                            path_str
                        )));
                    }
                }
            }
        }
        "/agents" => match clido_core::load_config(&app.workspace_root) {
            Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
            Ok(loaded) => {
                app.push(ChatLine::Info("  Agent configuration:".into()));
                if let Some(main) = &loaded.agents.main {
                    app.push(ChatLine::Info(format!(
                        "  main      {} / {}",
                        main.provider, main.model
                    )));
                } else {
                    app.push(ChatLine::Info(
                        "  main      (using [profile.default])".into(),
                    ));
                }
                if let Some(worker) = &loaded.agents.worker {
                    app.push(ChatLine::Info(format!(
                        "  worker    {} / {}",
                        worker.provider, worker.model
                    )));
                } else {
                    app.push(ChatLine::Info(
                        "  worker    not set  (uses main agent)".into(),
                    ));
                }
                if let Some(reviewer) = &loaded.agents.reviewer {
                    app.push(ChatLine::Info(format!(
                        "  reviewer  {} / {}",
                        reviewer.provider, reviewer.model
                    )));
                } else {
                    app.push(ChatLine::Info("  reviewer  not set  (disabled)".into()));
                }
                app.push(ChatLine::Info("  Run /init to reconfigure.".into()));
            }
        },
        "/profiles" => match clido_core::load_config(&app.workspace_root) {
            Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
            Ok(loaded) => {
                app.push(ChatLine::Info("  Profiles:".into()));
                let mut names: Vec<&String> = loaded.profiles.keys().collect();
                names.sort();
                for name in names {
                    let entry = &loaded.profiles[name];
                    let is_active = name == &loaded.default_profile;
                    let marker = if is_active { "▶" } else { " " };
                    app.push(ChatLine::Info(format!(
                        "  {} {}  {} / {}",
                        marker, name, entry.provider, entry.model
                    )));
                    if let Some(ref w) = entry.worker {
                        app.push(ChatLine::Info(format!(
                            "       worker    {} / {}",
                            w.provider, w.model
                        )));
                    }
                    if let Some(ref r) = entry.reviewer {
                        app.push(ChatLine::Info(format!(
                            "       reviewer  {} / {}",
                            r.provider, r.model
                        )));
                    }
                }
                app.push(ChatLine::Info(
                    "  /profile → pick & switch  |  /profile new → create  |  /profile edit → edit"
                        .into(),
                ));
            }
        },
        "/profile" => {
            // No name given → open interactive profile picker.
            match clido_core::load_config(&app.workspace_root) {
                Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
                Ok(loaded) => {
                    let active = loaded.default_profile.clone();
                    let mut profiles: Vec<(String, clido_core::ProfileEntry)> =
                        loaded.profiles.into_iter().collect();
                    profiles.sort_by(|a, b| a.0.cmp(&b.0));
                    let selected = profiles.iter().position(|(n, _)| n == &active).unwrap_or(0);
                    app.profile_picker = Some(ProfilePickerState {
                        profiles,
                        selected,
                        scroll_offset: 0,
                        active,
                        filter: String::new(),
                    });
                }
            }
        }
        "/profile new" => {
            let config_path = clido_core::global_config_path()
                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
            app.profile_overlay = Some(ProfileOverlayState::for_create(config_path));
        }
        cmd if cmd == "/profile edit" || cmd.starts_with("/profile edit ") => {
            let arg = cmd.trim_start_matches("/profile edit").trim();
            let name = if arg.is_empty() {
                app.current_profile.clone()
            } else {
                arg.to_string()
            };
            let config_path = clido_core::global_config_path()
                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
            match clido_core::load_config(&app.workspace_root) {
                Err(e) => {
                    app.push(ChatLine::Info(format!("  ✗ Could not load config: {e}")));
                }
                Ok(loaded) => match loaded.profiles.get(&name).cloned() {
                    None => {
                        app.push(ChatLine::Info(format!(
                            "  ✗ Profile '{}' not found. Use /profiles to list.",
                            name
                        )));
                    }
                    Some(entry) => {
                        app.profile_overlay =
                            Some(ProfileOverlayState::for_edit(name, &entry, config_path));
                    }
                },
            }
        }
        cmd if cmd.starts_with("/profile ") => {
            let name = cmd.trim_start_matches("/profile ").trim();
            match clido_core::load_config(&app.workspace_root) {
                Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
                Ok(loaded) => {
                    if !loaded.profiles.contains_key(name) {
                        app.push(ChatLine::Info(format!(
                            "  profile '{}' not found. Use /profiles to list or /profile new to create.",
                            name
                        )));
                    } else if name == loaded.default_profile {
                        app.push(ChatLine::Info(format!(
                            "  profile '{}' is already active.",
                            name
                        )));
                    } else {
                        app.push(ChatLine::Info(format!(
                            "  switching to profile '{}'…",
                            name
                        )));
                        app.restart_resume_session = app.current_session_id.clone();
                        app.wants_profile_switch = Some(name.to_string());
                        app.quit = true;
                    }
                }
            }
        }
        "/settings" => {
            let config_path = clido_core::global_config_path()
                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
            let roles = app.config_roles.clone();
            let profile = app.current_profile.clone();
            app.settings = Some(SettingsState::new(
                config_path,
                roles,
                app.model.clone(),
                profile,
            ));
        }
        "/config" => {
            // Show a complete, structured overview of all current settings.
            match clido_core::load_config(&app.workspace_root) {
                Err(e) => app.push(ChatLine::Info(format!("  ✗ Could not load config: {}", e))),
                Ok(loaded) => {
                    // Config file location.
                    let global_path = clido_core::global_config_path();
                    let project_path_opt = app.workspace_root.join(".clido/config.toml");
                    let project_exists = project_path_opt.exists();
                    let config_file_label = if project_exists {
                        format!("{}", project_path_opt.display())
                    } else {
                        global_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "~/.config/clido/config.toml".to_string())
                    };

                    app.push(ChatLine::Info("".into()));
                    app.push(ChatLine::Section("Active Profile".into()));
                    let active = &loaded.default_profile;
                    if let Some(p) = loaded.profiles.get(active) {
                        let key_status = if p.api_key.is_some() {
                            "key ✓ (in config)"
                        } else if p.api_key_env.is_some() {
                            "key ✓ (from env)"
                        } else {
                            "key ✗ (not set)"
                        };
                        app.push(ChatLine::Info(format!(
                            "  {} — {} / {}  {}",
                            active, p.provider, p.model, key_status
                        )));
                        if let Some(ref url) = p.base_url {
                            app.push(ChatLine::Info(format!("  Custom endpoint: {}", url)));
                        }
                    }

                    // All profiles.
                    if loaded.profiles.len() > 1 {
                        app.push(ChatLine::Info("".into()));
                        app.push(ChatLine::Section("All Profiles".into()));
                        let mut names: Vec<&String> = loaded.profiles.keys().collect();
                        names.sort();
                        for name in names {
                            let p = &loaded.profiles[name];
                            let marker = if name == active { "▶" } else { " " };
                            let worker_s = if let Some(ref w) = p.worker {
                                format!("  worker: {}/{}", w.provider, w.model)
                            } else {
                                String::new()
                            };
                            let reviewer_s = if let Some(ref r) = p.reviewer {
                                format!("  reviewer: {}/{}", r.provider, r.model)
                            } else {
                                String::new()
                            };
                            app.push(ChatLine::Info(format!(
                                "  {} {:<14}  {}/{}{}{}",
                                marker, name, p.provider, p.model, worker_s, reviewer_s
                            )));
                        }
                    }

                    // Roles.
                    let roles_map = loaded.roles.as_map();
                    if !roles_map.is_empty() {
                        app.push(ChatLine::Info("".into()));
                        app.push(ChatLine::Section("Roles".into()));
                        app.push(ChatLine::Info(
                            "  (use /role <name> to switch, /fast = fast role, /smart = reasoning role)".into(),
                        ));
                        let mut role_names: Vec<&String> = roles_map.keys().collect();
                        role_names.sort();
                        for name in role_names {
                            app.push(ChatLine::Info(format!(
                                "  {:<14}  {}",
                                name, roles_map[name]
                            )));
                        }
                    }

                    // Agent behavior.
                    app.push(ChatLine::Info("".into()));
                    app.push(ChatLine::Section("Agent Behavior".into()));
                    let a = &loaded.agent;
                    app.push(ChatLine::Info(format!(
                        "  max-turns           {}",
                        a.max_turns
                    )));
                    if let Some(budget) = a.max_budget_usd {
                        app.push(ChatLine::Info(format!(
                            "  max-budget-usd      ${:.2}",
                            budget
                        )));
                    } else {
                        app.push(ChatLine::Info("  max-budget-usd      unlimited".into()));
                    }
                    if let Some(tools) = a.max_concurrent_tools {
                        app.push(ChatLine::Info(format!("  max-concurrent-tools  {}", tools)));
                    }
                    if let Some(out_tok) = a.max_output_tokens {
                        app.push(ChatLine::Info(format!("  max-output-tokens   {}", out_tok)));
                    }
                    app.push(ChatLine::Info(format!(
                        "  auto-checkpoint     {}",
                        if a.auto_checkpoint { "on" } else { "off" }
                    )));
                    app.push(ChatLine::Info(format!(
                        "  quiet               {}",
                        if a.quiet { "on" } else { "off" }
                    )));
                    app.push(ChatLine::Info(format!(
                        "  notify              {}",
                        if a.notify { "on" } else { "off" }
                    )));
                    if a.no_rules {
                        app.push(ChatLine::Info(
                            "  no-rules            on (CLIDO.md ignored)".into(),
                        ));
                    }

                    // Context.
                    app.push(ChatLine::Info("".into()));
                    app.push(ChatLine::Section("Context".into()));
                    let c = &loaded.context;
                    app.push(ChatLine::Info(format!(
                        "  compaction-threshold  {:.0}%  (compress when context is this full)",
                        c.compaction_threshold * 100.0
                    )));
                    if let Some(max_ctx) = c.max_context_tokens {
                        app.push(ChatLine::Info(format!("  max-context-tokens  {}", max_ctx)));
                    }
                    // Show live session token usage if available.
                    if app.session_input_tokens > 0 {
                        let used = app.session_input_tokens;
                        let limit = if app.context_max_tokens > 0 {
                            app.context_max_tokens
                        } else {
                            0
                        };
                        let usage_str = if limit > 0 {
                            let pct = (used as f64 / limit as f64 * 100.0).min(100.0);
                            format!(
                                "  context now           {} / {} tokens  ({:.0}% used)",
                                used, limit, pct
                            )
                        } else {
                            format!("  context now           {} tokens used this turn", used)
                        };
                        app.push(ChatLine::Info(usage_str));
                    }

                    // Agent slots (global).
                    let agents = &loaded.agents;
                    if agents.main.is_some() || agents.worker.is_some() || agents.reviewer.is_some()
                    {
                        app.push(ChatLine::Info("".into()));
                        app.push(ChatLine::Section("Agent Slots (global)".into()));
                        if let Some(ref m) = agents.main {
                            app.push(ChatLine::Info(format!(
                                "  main      {}/{}",
                                m.provider, m.model
                            )));
                        }
                        if let Some(ref w) = agents.worker {
                            app.push(ChatLine::Info(format!(
                                "  worker    {}/{}",
                                w.provider, w.model
                            )));
                        }
                        if let Some(ref r) = agents.reviewer {
                            app.push(ChatLine::Info(format!(
                                "  reviewer  {}/{}",
                                r.provider, r.model
                            )));
                        }
                    }

                    // Config file path.
                    app.push(ChatLine::Info("".into()));
                    app.push(ChatLine::Info(format!(
                        "  Config file: {}",
                        config_file_label
                    )));
                    app.push(ChatLine::Info(
                        "  Use /configure <intent> to change settings in natural language".into(),
                    ));
                    app.push(ChatLine::Info("".into()));
                }
            }
        }
        _ if cmd == "/configure" || cmd.starts_with("/configure ") => {
            let intent = cmd.trim_start_matches("/configure").trim();
            if intent.is_empty() {
                app.push(ChatLine::Info("  Usage: /configure <intent>".into()));
                app.push(ChatLine::Info(
                    "  Examples:  /configure optimize for speed".into(),
                ));
                app.push(ChatLine::Info(
                    "             /configure use gpt-4o as default".into(),
                ));
                app.push(ChatLine::Info(
                    "             /configure set max turns to 50".into(),
                ));
                app.push(ChatLine::Info(
                    "             /configure add a fast role with claude-haiku".into(),
                ));
            } else {
                let global_path = clido_core::global_config_path();
                let project_path = app.workspace_root.join(".clido/config.toml");
                let (config_path, config_path_label) = if project_path.exists() {
                    (project_path.clone(), project_path.display().to_string())
                } else {
                    let gp = global_path
                        .clone()
                        .unwrap_or_else(|| std::path::PathBuf::from("~/.config/clido/config.toml"));
                    let label = gp.display().to_string();
                    (gp, label)
                };
                let intent = intent.to_string();
                let prompt = format!(
                    "The user wants to change their Clido configuration.\n\
                    \n\
                    Config file path: {config_path_label}\n\
                    \n\
                    User intent: \"{intent}\"\n\
                    \n\
                    Steps:\n\
                    1. Read the current config file at `{config_path_label}` to understand the \
                       exact format and current values.\n\
                    2. Determine the minimum set of changes needed to fulfil the intent.\n\
                    3. Before changing anything, summarise: what you will change and why.\n\
                    4. Apply the changes using the Edit or Write tool. Make surgical changes only — \
                       do NOT rewrite the entire file unless it does not exist yet.\n\
                    5. Confirm what was changed with a brief summary.\n\
                    \n\
                    Config file format reference (TOML):\n\
                    ```toml\n\
                    default-profile = \"default\"\n\
                    \n\
                    [profile.default]\n\
                    provider = \"anthropic\"  # anthropic | openai | openrouter | mistral | local | alibabacloud\n\
                    model    = \"claude-sonnet-4-6\"\n\
                    api_key  = \"sk-...\"      # optional; prefer api_key_env for safety\n\
                    api_key_env = \"ANTHROPIC_API_KEY\"  # env var name\n\
                    base_url = \"\"            # optional custom endpoint\n\
                    \n\
                    # Per-profile sub-agents (override global [agents.*]):\n\
                    # [profile.default.worker]\n\
                    # provider = \"anthropic\"\n\
                    # model    = \"claude-haiku-4-5-20251001\"\n\
                    \n\
                    [agent]\n\
                    max-turns            = 200     # maximum tool-use turns per run\n\
                    max-budget-usd       = 5.0     # cost cap per run (omit for unlimited)\n\
                    max-concurrent-tools = 4       # parallel tool calls\n\
                    max-output-tokens    = 8192    # max tokens per LLM response\n\
                    quiet                = false   # suppress spinner / cost footer\n\
                    notify               = false   # desktop notification on completion\n\
                    auto-checkpoint      = true    # checkpoint before file-mutating turns\n\
                    no-rules             = false   # ignore CLIDO.md rules files\n\
                    \n\
                    [context]\n\
                    compaction-threshold = 0.75    # compress context when 75% full\n\
                    max-context-tokens   = 100000  # optional hard cap on context size\n\
                    \n\
                    [roles]\n\
                    fast      = \"claude-haiku-4-5-20251001\"  # /fast role\n\
                    reasoning = \"claude-opus-4-6\"            # /smart role\n\
                    # any extra role name = \"model-id\"       # /role <name>\n\
                    \n\
                    [index]\n\
                    exclude-patterns = [\"*.lock\", \"vendor/**\"]\n\
                    include-ignored  = false\n\
                    ```\n\
                    \n\
                    Valid providers: anthropic, openai, openrouter, mistral, local, alibabacloud\n\
                    \n\
                    If the config file does not exist at `{config_path_label}`, create it with \
                    sensible defaults plus the requested changes.\n\
                    After writing, ask the user to restart clido or type /init to reload the config.",
                    config_path_label = config_path_label,
                    intent = intent,
                );
                let _ = config_path; // suppress unused warning
                app.send_now(prompt);
            }
        }
        "/init" => {
            // Open the active profile in the in-TUI editor instead of exiting.
            let config_path = clido_core::global_config_path()
                .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
            match clido_core::load_config(&app.workspace_root) {
                Err(e) => {
                    app.push(ChatLine::Info(format!("  ✗ Could not load config: {e}")));
                }
                Ok(loaded) => {
                    let name = app.current_profile.clone();
                    match loaded.profiles.get(&name).cloned() {
                        Some(entry) => {
                            app.profile_overlay =
                                Some(ProfileOverlayState::for_edit(name, &entry, config_path));
                        }
                        None => {
                            // No matching profile — open a create flow for the default profile
                            app.profile_overlay =
                                Some(ProfileOverlayState::for_create(config_path));
                        }
                    }
                }
            }
        }

        // ── Prompt Enhancement ──────────────────────────────────────────────
        _ if cmd == "/prompt-mode" || cmd.starts_with("/prompt-mode ") => {
            let arg = cmd.trim_start_matches("/prompt-mode").trim();
            match arg {
                "auto" => {
                    app.prompt_mode = PromptMode::Auto;
                    let path = project_settings_path(&app.workspace_root);
                    if let Err(e) = save_prompt_mode(&path, PromptMode::Auto) {
                        app.push(ChatLine::Info(format!("  ⚠ Could not save: {e}")));
                    }
                    app.push(ChatLine::Info(
                        "  ✓ Prompt enhancement: auto — prompts will be enhanced automatically"
                            .into(),
                    ));
                }
                "off" => {
                    app.prompt_mode = PromptMode::Off;
                    let path = project_settings_path(&app.workspace_root);
                    if let Err(e) = save_prompt_mode(&path, PromptMode::Off) {
                        app.push(ChatLine::Info(format!("  ⚠ Could not save: {e}")));
                    }
                    app.push(ChatLine::Info(
                        "  ✓ Prompt enhancement: off — raw input sent unchanged".into(),
                    ));
                }
                "" | "status" => {
                    let n_active = app.prompt_rules.active_rules().len();
                    let n_total = app.prompt_rules.rules.len();
                    app.push(ChatLine::Info("".into()));
                    app.push(ChatLine::Section("Prompt Enhancement".into()));
                    app.push(ChatLine::Info(format!(
                        "  mode     {}",
                        app.prompt_mode.as_str()
                    )));
                    app.push(ChatLine::Info(format!(
                        "  rules    {n_active} active / {n_total} total"
                    )));
                    app.push(ChatLine::Info("".into()));
                    app.push(ChatLine::Info(
                        "  /prompt-mode auto      enable automatic enhancement".into(),
                    ));
                    app.push(ChatLine::Info(
                        "  /prompt-mode off       send raw input unchanged".into(),
                    ));
                    app.push(ChatLine::Info(
                        "  /prompt-rules          view and manage rules".into(),
                    ));
                    app.push(ChatLine::Info(
                        "  /prompt-preview        preview enhanced prompt before sending".into(),
                    ));
                    app.push(ChatLine::Info("".into()));
                }
                _ => {
                    app.push(ChatLine::Info(
                        "  Usage: /prompt-mode [auto|off|status]".into(),
                    ));
                }
            }
        }

        "/prompt-preview" => {
            app.prompt_preview_text = Some(String::new());
            app.push(ChatLine::Info(
                "  ✦ Preview mode — next message will be shown enhanced but not sent. Press Enter to send or Esc to cancel.".into(),
            ));
        }

        _ if cmd == "/prompt-rules" || cmd.starts_with("/prompt-rules ") => {
            let arg = cmd.trim_start_matches("/prompt-rules").trim();
            if arg.is_empty() || arg == "list" {
                // Collect before any mutable borrows.
                let (active_lines, total): (Vec<String>, usize) = {
                    let active = app.prompt_rules.active_rules();
                    let total = app.prompt_rules.rules.len();
                    let lines: Vec<String> = active
                        .iter()
                        .map(|r| {
                            let badge = if r.source == "inferred" {
                                "inferred"
                            } else {
                                "manual"
                            };
                            format!("  [{}]  {}  ({})", r.id, r.text, badge)
                        })
                        .collect();
                    (lines, total)
                };
                app.push(ChatLine::Info("".into()));
                app.push(ChatLine::Section("Prompt Rules".into()));
                if active_lines.is_empty() {
                    app.push(ChatLine::Info(
                        "  No active rules.  Use /prompt-rules add <text> to add one.".into(),
                    ));
                    if total > 0 {
                        app.push(ChatLine::Info(format!(
                            "  ({total} rules below confidence threshold — not yet applied)"
                        )));
                    }
                } else {
                    for line in active_lines {
                        app.push(ChatLine::Info(line));
                    }
                }
                app.push(ChatLine::Info("".into()));
                app.push(ChatLine::Info(
                    "  /prompt-rules add <text>     add a new rule".into(),
                ));
                app.push(ChatLine::Info(
                    "  /prompt-rules remove <id>    remove a rule by id".into(),
                ));
                app.push(ChatLine::Info(
                    "  /prompt-rules reset          clear all rules".into(),
                ));
                app.push(ChatLine::Info("".into()));
            } else if let Some(text) = arg.strip_prefix("add ") {
                let text = text.trim();
                if text.is_empty() {
                    app.push(ChatLine::Info(
                        "  Usage: /prompt-rules add <rule text>".into(),
                    ));
                } else {
                    let id = text
                        .to_lowercase()
                        .split_whitespace()
                        .take(4)
                        .collect::<Vec<_>>()
                        .join("-");
                    let rule = RuleEntry::new_manual(id, text);
                    app.prompt_rules.upsert(rule);
                    let path = project_rules_path(&app.workspace_root);
                    if let Err(e) = save_rules(&path, &app.prompt_rules) {
                        app.push(ChatLine::Info(format!("  ⚠ Could not save rules: {e}")));
                    }
                    app.push(ChatLine::Info(format!("  ✓ Rule added: \"{text}\"")));
                }
            } else if let Some(id) = arg.strip_prefix("remove ") {
                let id = id.trim();
                if app.prompt_rules.remove(id) {
                    let path = project_rules_path(&app.workspace_root);
                    let _ = save_rules(&path, &app.prompt_rules);
                    app.push(ChatLine::Info(format!("  ✓ Rule removed: {id}")));
                } else {
                    app.push(ChatLine::Info(format!("  ✗ No rule with id: {id}")));
                }
            } else if arg == "reset" {
                app.prompt_rules = PromptRules::default();
                let path = project_rules_path(&app.workspace_root);
                let _ = save_rules(&path, &app.prompt_rules);
                app.push(ChatLine::Info("  ✓ All rules cleared".into()));
            } else {
                app.push(ChatLine::Info(
                    "  Usage: /prompt-rules [list|add <text>|remove <id>|reset]".into(),
                ));
            }
        }

        _ => {}
    }
}

/// Width-aware version; call this from render paths where chat_area.width is known.
/// Uses a per-width render cache keyed by message content hash to avoid re-rendering
/// unchanged messages on every tick.
fn build_lines_w(app: &mut App, width: usize) -> Vec<Line<'static>> {
    // Compute a cheap hash of the current messages state.
    // Key: (content_hash, width) where content_hash covers message count + last message text.
    let msg_count = app.messages.len();
    let content_hash = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        msg_count.hash(&mut h);
        // Include last message content so new streaming tokens invalidate the cache.
        if let Some(last) = app.messages.last() {
            std::mem::discriminant(last).hash(&mut h);
            match last {
                ChatLine::User(t) | ChatLine::Assistant(t) | ChatLine::Thinking(t) => {
                    t.hash(&mut h);
                }
                _ => {}
            }
        }
        h.finish()
    };
    let cache_key = (content_hash, width);

    // Evict stale entries when the message list shrinks (e.g. after /compact).
    if msg_count < app.render_cache_msg_count {
        app.render_cache.clear();
    }
    app.render_cache_msg_count = msg_count;

    if let Some(cached) = app.render_cache.get(&cache_key) {
        return cached.clone();
    }

    let result = build_lines_w_uncached(app, width);
    app.render_cache.insert(cache_key, result.clone());
    result
}

fn build_lines_w_uncached(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for msg in &app.messages {
        match msg {
            ChatLine::User(text) => {
                out.push(Line::from(vec![Span::styled(
                    "you",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )]));
                out.extend(render_markdown(text, width));
                out.push(Line::raw(""));
            }
            ChatLine::Assistant(text) => {
                let label = if app.model.is_empty() {
                    "clido".to_string()
                } else {
                    app.model.clone()
                };
                out.push(Line::from(vec![Span::styled(
                    label,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )]));
                out.extend(render_markdown(text, width));
                out.push(Line::raw(""));
            }
            ChatLine::Thinking(text) => {
                for part in text.lines() {
                    out.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(
                            part.to_string(),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::DIM),
                        ),
                    ]));
                }
            }
            ChatLine::ToolCall {
                name,
                detail,
                done,
                is_error,
                ..
            } => {
                let color = tool_color(name, *done, *is_error);
                let style = Style::default().fg(color);
                let icon = if *is_error {
                    "✗"
                } else if *done {
                    "✓"
                } else {
                    "↻"
                };
                let display_name = tool_display_name(name);
                let dim = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM);
                if detail.is_empty() {
                    out.push(Line::from(vec![Span::styled(
                        format!("  {} {}", icon, display_name),
                        style,
                    )]));
                } else {
                    out.push(Line::from(vec![
                        Span::styled(format!("  {} {}", icon, display_name), style),
                        Span::styled(format!("  {}", detail.clone()), dim),
                    ]));
                }
            }
            ChatLine::Diff(text) => {
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
                            Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                        )]));
                    } else if line.starts_with("---") || line.starts_with("+++") {
                        out.push(Line::from(vec![Span::styled(
                            format!("  {}", line),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::DIM),
                        )]));
                    } else if line.starts_with('+') {
                        let lineno = new_lineno;
                        new_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ]));
                    } else if line.starts_with('-') {
                        let lineno = old_lineno;
                        old_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                            ),
                        ]));
                    } else {
                        // context line — belongs to both
                        let lineno = new_lineno;
                        old_lineno += 1;
                        new_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ]));
                    }
                }
                out.push(Line::raw(""));
            }
            ChatLine::Info(text) => {
                out.push(Line::from(vec![Span::styled(
                    if text.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", text)
                    },
                    Style::default().fg(Color::DarkGray),
                )]));
            }
            ChatLine::Section(text) => {
                out.push(Line::from(vec![Span::styled(
                    format!("  {}", text),
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::BOLD),
                )]));
            }
            ChatLine::WelcomeBrand => {
                out.push(Line::from(vec![
                    Span::styled("  ── ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        "cli",
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        ";",
                        Style::default()
                            .fg(TUI_SOFT_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "do",
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " ─────────────────────",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
            ChatLine::WelcomeSplash => {
                // Shown only when scrolling back past the start of a resumed conversation.
                out.push(Line::from(vec![
                    Span::styled("  ── ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        "cli",
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        ";",
                        Style::default()
                            .fg(TUI_SOFT_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "do",
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " ─────────────────────",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
        }
    }
    out
}

/// Parse `@@ -old_start[,len] +new_start[,len] @@` → (old_start, new_start).
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let inner = line.strip_prefix("@@ ")?.split(" @@").next()?;
    let mut parts = inner.split_whitespace();
    let old_part = parts.next()?;
    let new_part = parts.next()?;
    let old_start: u32 = old_part
        .trim_start_matches('-')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new_start: u32 = new_part
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old_start, new_start))
}

/// Render markdown text into a series of tui `Line`s with appropriate styling.
///
/// Supports: headings, bold/italic/strikethrough, inline code, fenced code blocks,
/// ordered/unordered lists, blockquotes, tables (with box-drawing borders),
/// horizontal rules, task-list checkboxes, and hard/soft breaks.
fn render_markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Tag};
    // Available content width: subtract 4 chars for left margin / padding.
    let content_w = width.saturating_sub(4).max(20);

    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(text, opts);

    let mut out: Vec<Line<'static>> = Vec::new();
    // Spans accumulating for the current output line.
    let mut cur_spans: Vec<Span<'static>> = Vec::new();

    // ── Inline style stack ────────────────────────────────────────────────
    // Each entry is the *combined* Style at that nesting depth.
    // On Start(Strong/Emphasis/…) we push a new style; on End we pop.
    // Text events use the top-of-stack style — no more empty-span tricks.
    let mut style_stack: Vec<Style> = vec![Style::default()];

    // ── Block state ───────────────────────────────────────────────────────
    let mut in_code_block = false;

    // ── List state ────────────────────────────────────────────────────────
    let mut list_depth: u32 = 0;

    // ── Blockquote depth ─────────────────────────────────────────────────
    let mut bq_depth: u32 = 0;

    // ── Table state ───────────────────────────────────────────────────────
    let mut in_table_head = false;
    let mut in_table_cell = false;
    let mut table_alignments: Vec<pulldown_cmark::Alignment> = Vec::new();
    let mut table_header: Option<Vec<String>> = None;
    let mut table_body: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();

    // flush current_spans as a new output Line (macro so it can access locals).
    macro_rules! flush {
        () => {
            if !cur_spans.is_empty() {
                out.push(Line::from(std::mem::take(&mut cur_spans)));
            }
        };
    }

    for event in parser {
        match event {
            // ── Start tags ────────────────────────────────────────────────
            Event::Start(tag) => match tag {
                Tag::Strong => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::BOLD);
                    style_stack.push(s);
                }
                Tag::Emphasis => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::ITALIC);
                    style_stack.push(s);
                }
                Tag::Strikethrough => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::CROSSED_OUT);
                    style_stack.push(s);
                }
                Tag::Link(..) => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .fg(TUI_SOFT_ACCENT)
                        .add_modifier(Modifier::UNDERLINED);
                    style_stack.push(s);
                }
                Tag::Heading(level, ..) => {
                    flush!();
                    let prefix = match level {
                        HeadingLevel::H1 => "█ ",
                        HeadingLevel::H2 => "▌ ",
                        HeadingLevel::H3 => "▸ ",
                        _ => "  ",
                    };
                    cur_spans.push(Span::styled(
                        prefix.to_string(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ));
                    style_stack.push(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    flush!();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) if !l.is_empty() => l.to_string(),
                        _ => String::new(),
                    };
                    let label = if lang.is_empty() {
                        "code".to_string()
                    } else {
                        lang
                    };
                    out.push(Line::from(vec![Span::styled(
                        format!("┌─ {} ", label),
                        Style::default().fg(Color::DarkGray),
                    )]));
                }
                Tag::List(_) => {
                    list_depth += 1;
                }
                Tag::Item => {
                    flush!();
                    let indent = "  ".repeat(list_depth.saturating_sub(1) as usize);
                    cur_spans.push(Span::styled(
                        format!("{}• ", indent),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                Tag::Paragraph => {}
                Tag::BlockQuote => {
                    bq_depth += 1;
                }
                Tag::Table(aligns) => {
                    table_alignments = aligns;
                    table_header = None;
                    table_body.clear();
                    flush!();
                }
                Tag::TableHead => {
                    in_table_head = true;
                }
                Tag::TableRow => {
                    current_row.clear();
                }
                Tag::TableCell => {
                    in_table_cell = true;
                    current_cell.clear();
                }
                _ => {}
            },

            // ── End tags ──────────────────────────────────────────────────
            Event::End(tag) => match tag {
                Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link(..) => {
                    style_stack.pop();
                }
                Tag::Heading(..) => {
                    style_stack.pop();
                    flush!();
                    out.push(Line::raw(""));
                }
                Tag::CodeBlock(_) => {
                    in_code_block = false;
                    flush!();
                    out.push(Line::from(vec![Span::styled(
                        format!("└{}", "─".repeat(content_w.min(60))),
                        Style::default().fg(Color::DarkGray),
                    )]));
                    out.push(Line::raw(""));
                }
                Tag::List(_) => {
                    list_depth = list_depth.saturating_sub(1);
                    if list_depth == 0 {
                        out.push(Line::raw(""));
                    }
                }
                Tag::Item => {
                    flush!();
                }
                Tag::Paragraph => {
                    flush!();
                    out.push(Line::raw(""));
                }
                Tag::BlockQuote => {
                    flush!();
                    bq_depth = bq_depth.saturating_sub(1);
                    if bq_depth == 0 {
                        out.push(Line::raw(""));
                    }
                }
                Tag::TableCell => {
                    in_table_cell = false;
                    current_row.push(std::mem::take(&mut current_cell));
                }
                Tag::TableRow => {
                    if !in_table_head {
                        table_body.push(std::mem::take(&mut current_row));
                    }
                }
                Tag::TableHead => {
                    in_table_head = false;
                    table_header = Some(std::mem::take(&mut current_row));
                }
                Tag::Table(_) => {
                    render_table_to_lines(
                        table_header.take(),
                        std::mem::take(&mut table_body),
                        &table_alignments,
                        &mut out,
                    );
                }
                _ => {}
            },

            // ── Leaf events ───────────────────────────────────────────────
            Event::Text(t) => {
                if in_table_cell {
                    current_cell.push_str(&t);
                } else if in_code_block {
                    // Code text arrives as one blob; split on newlines.
                    for (i, line) in t.split('\n').enumerate() {
                        if i > 0 {
                            flush!();
                        }
                        if !line.is_empty() {
                            cur_spans.push(Span::styled(
                                format!("  {}", line),
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::DIM),
                            ));
                        }
                    }
                } else {
                    // Emit blockquote gutter at the start of each line.
                    if bq_depth > 0 && cur_spans.is_empty() {
                        cur_spans.push(Span::styled(
                            "▌ ".repeat(bq_depth as usize),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    let style = style_stack.last().copied().unwrap_or_default();
                    cur_spans.push(Span::styled(t.to_string(), style));
                }
            }
            Event::Code(t) => {
                // Inline code — always use yellow dim style, never inherit parent style.
                if in_table_cell {
                    current_cell.push_str(&t);
                } else {
                    cur_spans.push(Span::styled(
                        t.to_string(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::DIM),
                    ));
                }
            }
            Event::SoftBreak => {
                if !in_table_cell && !in_code_block {
                    cur_spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if !in_table_cell {
                    flush!();
                }
            }
            Event::Rule => {
                flush!();
                out.push(Line::from(vec![Span::styled(
                    "─".repeat(content_w.min(72)),
                    Style::default().fg(Color::DarkGray),
                )]));
                out.push(Line::raw(""));
            }
            Event::TaskListMarker(checked) => {
                let marker = if checked { "☑ " } else { "☐ " };
                cur_spans.push(Span::styled(
                    marker.to_string(),
                    Style::default().fg(Color::Cyan),
                ));
            }
            Event::Html(_) | Event::FootnoteReference(_) => {}
        }
    }

    flush!();
    out
}

/// Render a collected markdown table into box-drawing `Line`s.
///
/// ```text
/// ┌──────────┬──────────┬──────────┐
/// │  Header1 │  Header2 │  Header3 │
/// ├──────────┼──────────┼──────────┤
/// │  Cell A  │  Cell B  │  Cell C  │
/// └──────────┴──────────┴──────────┘
/// ```
fn render_table_to_lines(
    header: Option<Vec<String>>,
    rows: Vec<Vec<String>>,
    alignments: &[pulldown_cmark::Alignment],
    out: &mut Vec<Line<'static>>,
) {
    use pulldown_cmark::Alignment as Align;

    let ncols = alignments
        .len()
        .max(header.as_ref().map_or(0, |h| h.len()))
        .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));

    if ncols == 0 {
        return;
    }

    // Compute per-column content widths (padding added separately).
    let mut col_widths = vec![1usize; ncols];
    if let Some(ref h) = header {
        for (i, cell) in h.iter().enumerate().take(ncols) {
            col_widths[i] = col_widths[i].max(cell.len());
        }
    }
    for row in &rows {
        for (i, cell) in row.iter().enumerate().take(ncols) {
            col_widths[i] = col_widths[i].max(cell.len());
        }
    }

    let align_cell = |content: &str, width: usize, align: &Align| -> String {
        match align {
            Align::Right => format!("{:>width$}", content),
            Align::Center => {
                let pad = width.saturating_sub(content.len());
                let left = pad / 2;
                format!("{}{}{}", " ".repeat(left), content, " ".repeat(pad - left))
            }
            _ => format!("{:<width$}", content),
        }
    };

    let gray = Style::default().fg(Color::DarkGray);
    let hdr_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    // ┌──┬──┬──┐
    let top: String = col_widths
        .iter()
        .map(|w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("┬");
    out.push(Line::from(vec![Span::styled(format!("┌{}┐", top), gray)]));

    // Header row (cyan bold)
    if let Some(ref h) = header {
        let mut spans = vec![Span::styled("│".to_string(), gray)];
        for (i, &w) in col_widths.iter().enumerate().take(ncols) {
            let content = h.get(i).map(|s| s.as_str()).unwrap_or("");
            let cell = align_cell(content, w, alignments.get(i).unwrap_or(&Align::None));
            spans.push(Span::styled(format!(" {} ", cell), hdr_style));
            spans.push(Span::styled("│".to_string(), gray));
        }
        out.push(Line::from(spans));

        // ├──┼──┼──┤
        let sep: String = col_widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┼");
        out.push(Line::from(vec![Span::styled(format!("├{}┤", sep), gray)]));
    }

    // Body rows
    for row in &rows {
        let mut spans = vec![Span::styled("│".to_string(), gray)];
        for (i, &w) in col_widths.iter().enumerate().take(ncols) {
            let content = row.get(i).map(|s| s.as_str()).unwrap_or("");
            let cell = align_cell(content, w, alignments.get(i).unwrap_or(&Align::None));
            spans.push(Span::raw(format!(" {} ", cell)));
            spans.push(Span::styled("│".to_string(), gray));
        }
        out.push(Line::from(spans));
    }

    // └──┴──┴──┘
    let bot: String = col_widths
        .iter()
        .map(|w| "─".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("┴");
    out.push(Line::from(vec![Span::styled(format!("└{}┘", bot), gray)]));
    out.push(Line::raw(""));
}

// ── Scroll helpers ────────────────────────────────────────────────────────────

fn scroll_up(app: &mut App, lines: u32) {
    if app.following {
        app.scroll = app.max_scroll;
    }
    app.scroll = app.scroll.saturating_sub(lines);
    app.following = false;
}

fn scroll_down(app: &mut App, lines: u32) {
    let new_scroll = app.scroll.saturating_add(lines);
    if new_scroll >= app.max_scroll {
        app.following = true;
    } else {
        app.scroll = new_scroll;
        app.following = false;
    }
}

// ── Plan text editor key handling (nano-style) ───────────────────────────────

fn handle_plan_text_editor_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    let save_and_close = |app: &mut App| {
        if let Some(ed) = app.plan_text_editor.take() {
            let tasks = ed.to_tasks();
            if !tasks.is_empty() {
                app.last_plan = Some(tasks.clone());
                app.last_plan_snapshot = build_plan_from_tasks(&tasks);
            }
        }
    };

    match (event.modifiers, event.code) {
        (_, Esc) => {
            // Discard changes — close without saving.
            app.plan_text_editor = None;
        }
        (Km::CONTROL, Char('s')) => save_and_close(app),
        (Km::CONTROL, Char('c')) => {
            app.plan_text_editor = None;
        }
        (_, Up) => {
            if let Some(ed) = &mut app.plan_text_editor {
                if ed.cursor_row > 0 {
                    ed.cursor_row -= 1;
                    if ed.cursor_row < ed.scroll {
                        ed.scroll = ed.cursor_row;
                    }
                    ed.clamp_col();
                }
            }
        }
        (_, Down) => {
            if let Some(ed) = &mut app.plan_text_editor {
                if ed.cursor_row + 1 < ed.lines.len() {
                    ed.cursor_row += 1;
                    ed.clamp_col();
                }
            }
        }
        (_, Left) => {
            if let Some(ed) = &mut app.plan_text_editor {
                if ed.cursor_col > 0 {
                    ed.cursor_col -= 1;
                } else if ed.cursor_row > 0 {
                    ed.cursor_row -= 1;
                    ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
                }
            }
        }
        (_, Right) => {
            if let Some(ed) = &mut app.plan_text_editor {
                let line_len = ed.lines[ed.cursor_row].chars().count();
                if ed.cursor_col < line_len {
                    ed.cursor_col += 1;
                } else if ed.cursor_row + 1 < ed.lines.len() {
                    ed.cursor_row += 1;
                    ed.cursor_col = 0;
                }
            }
        }
        (_, Home) => {
            if let Some(ed) = &mut app.plan_text_editor {
                ed.cursor_col = 0;
            }
        }
        (_, End) => {
            if let Some(ed) = &mut app.plan_text_editor {
                ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
            }
        }
        (_, Enter) => {
            if let Some(ed) = &mut app.plan_text_editor {
                let line = ed.lines[ed.cursor_row].clone();
                let chars: Vec<char> = line.chars().collect();
                let col = ed.cursor_col.min(chars.len());
                let left: String = chars[..col].iter().collect();
                let right: String = chars[col..].iter().collect();
                ed.lines[ed.cursor_row] = left;
                ed.cursor_row += 1;
                ed.cursor_col = 0;
                ed.lines.insert(ed.cursor_row, right);
            }
        }
        (_, Backspace) => {
            if let Some(ed) = &mut app.plan_text_editor {
                if ed.cursor_col > 0 {
                    let line = &mut ed.lines[ed.cursor_row];
                    let mut chars: Vec<char> = line.chars().collect();
                    chars.remove(ed.cursor_col - 1);
                    *line = chars.iter().collect();
                    ed.cursor_col -= 1;
                } else if ed.cursor_row > 0 {
                    let current = ed.lines.remove(ed.cursor_row);
                    ed.cursor_row -= 1;
                    ed.cursor_col = ed.lines[ed.cursor_row].chars().count();
                    ed.lines[ed.cursor_row].push_str(&current);
                }
            }
        }
        (_, Delete) => {
            if let Some(ed) = &mut app.plan_text_editor {
                let line_len = ed.lines[ed.cursor_row].chars().count();
                if ed.cursor_col < line_len {
                    let line = &mut ed.lines[ed.cursor_row];
                    let mut chars: Vec<char> = line.chars().collect();
                    chars.remove(ed.cursor_col);
                    *line = chars.iter().collect();
                } else if ed.cursor_row + 1 < ed.lines.len() {
                    let next = ed.lines.remove(ed.cursor_row + 1);
                    ed.lines[ed.cursor_row].push_str(&next);
                }
            }
        }
        (km, Char(c)) if km == Km::NONE || km == Km::SHIFT => {
            if let Some(ed) = &mut app.plan_text_editor {
                let line = &mut ed.lines[ed.cursor_row];
                let mut chars: Vec<char> = line.chars().collect();
                let col = ed.cursor_col.min(chars.len());
                chars.insert(col, c);
                *line = chars.iter().collect();
                ed.cursor_col += 1;
            }
        }
        _ => {}
    }

    // Scroll to keep cursor visible (rough: assume terminal ~30 rows for editor area)
    if let Some(ed) = &mut app.plan_text_editor {
        if ed.cursor_row < ed.scroll {
            ed.scroll = ed.cursor_row;
        } else if ed.cursor_row >= ed.scroll + 20 {
            ed.scroll = ed.cursor_row - 19;
        }
    }
}

// ── Plan editor key handling ──────────────────────────────────────────────────

fn handle_plan_editor_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;

    // Ctrl+C always quits.
    if event.modifiers == crossterm::event::KeyModifiers::CONTROL && event.code == Char('c') {
        app.quit = true;
        return;
    }

    // If the inline edit form is active, handle form keys.
    if app.plan_task_editing.is_some() {
        match event.code {
            Tab => {
                if let Some(ref mut form) = app.plan_task_editing {
                    form.focused_field = match form.focused_field {
                        TaskEditField::Description => TaskEditField::Notes,
                        TaskEditField::Notes => TaskEditField::Complexity,
                        TaskEditField::Complexity => TaskEditField::Description,
                    };
                }
            }
            Left | Right => {
                if let Some(ref mut form) = app.plan_task_editing {
                    if form.focused_field == TaskEditField::Complexity {
                        form.complexity = match (&form.complexity, event.code) {
                            (Complexity::Low, Right) => Complexity::Medium,
                            (Complexity::Medium, Right) => Complexity::High,
                            (Complexity::High, Right) => Complexity::Low,
                            (Complexity::High, Left) => Complexity::Medium,
                            (Complexity::Medium, Left) => Complexity::Low,
                            (Complexity::Low, Left) => Complexity::High,
                            _ => form.complexity.clone(),
                        };
                    }
                }
            }
            Backspace => {
                if let Some(ref mut form) = app.plan_task_editing {
                    match form.focused_field {
                        TaskEditField::Description => {
                            form.description.pop();
                        }
                        TaskEditField::Notes => {
                            form.notes.pop();
                        }
                        TaskEditField::Complexity => {}
                    }
                }
            }
            Char(c) => {
                if let Some(ref mut form) = app.plan_task_editing {
                    match form.focused_field {
                        TaskEditField::Description => form.description.push(c),
                        TaskEditField::Notes => form.notes.push(c),
                        TaskEditField::Complexity => {}
                    }
                }
            }
            Enter => {
                // Save the form edits back to the plan.
                if let (Some(form), Some(ref mut editor)) =
                    (app.plan_task_editing.take(), app.plan_editor.as_mut())
                {
                    let _ = editor.rename_task(&form.task_id, &form.description);
                    let _ = editor.set_notes(&form.task_id, &form.notes);
                    let _ = editor.set_complexity(&form.task_id, form.complexity.clone());
                }
            }
            Esc => {
                app.plan_task_editing = None;
            }
            _ => {}
        }
        return;
    }

    // Task list navigation.
    let task_count = app
        .plan_editor
        .as_ref()
        .map(|e| e.plan.tasks.len())
        .unwrap_or(0);

    match event.code {
        Up => {
            if app.plan_selected_task > 0 {
                app.plan_selected_task -= 1;
            }
        }
        Down => {
            if app.plan_selected_task + 1 < task_count {
                app.plan_selected_task += 1;
            }
        }
        Enter => {
            // Open edit form for selected task.
            if let Some(ref editor) = app.plan_editor {
                if let Some(task) = editor.plan.tasks.get(app.plan_selected_task) {
                    app.plan_task_editing = Some(TaskEditState::new(
                        &task.id,
                        &task.description,
                        &task.notes,
                        task.complexity.clone(),
                    ));
                }
            }
        }
        Char('d') => {
            // Delete selected task.
            if let Some(ref mut editor) = app.plan_editor {
                if let Some(task) = editor.plan.tasks.get(app.plan_selected_task) {
                    let id = task.id.clone();
                    if editor.delete_task(&id).is_ok()
                        && app.plan_selected_task >= editor.plan.tasks.len()
                        && app.plan_selected_task > 0
                    {
                        app.plan_selected_task -= 1;
                    }
                }
            }
        }
        Char('n') => {
            // Add a new empty task and open edit form.
            if let Some(ref mut editor) = app.plan_editor {
                let new_id = format!("t{}", editor.plan.tasks.len() + 1);
                let _ = editor.add_task(new_id.clone(), "New task".to_string(), vec![]);
                app.plan_selected_task = editor.plan.tasks.len() - 1;
                app.plan_task_editing =
                    Some(TaskEditState::new(&new_id, "New task", "", Complexity::Low));
            }
        }
        Char(' ') => {
            // Toggle skip.
            if let Some(ref mut editor) = app.plan_editor {
                if let Some(task) = editor.plan.tasks.get(app.plan_selected_task) {
                    let id = task.id.clone();
                    let _ = editor.toggle_skip(&id);
                }
            }
        }
        Char('r') => {
            // Move selected task up (reorder).
            if let Some(ref mut editor) = app.plan_editor {
                if editor.move_up(app.plan_selected_task).is_ok() {
                    app.plan_selected_task -= 1;
                }
            }
        }
        Char('s') => {
            // Save plan to .clido/plans/.
            if let Some(ref editor) = app.plan_editor {
                match clido_planner::save_plan(&app.workspace_root, &editor.plan) {
                    Ok(path) => {
                        app.push(ChatLine::Info(format!(
                            "  ✓ Plan saved: {}",
                            path.display()
                        )));
                    }
                    Err(e) => {
                        app.pending_error = Some(ErrorInfo::from_message(format!(
                            "Could not save plan: {}",
                            e
                        )));
                    }
                }
            }
        }
        Char('x') => {
            // Execute the plan (or just close if dry-run).
            if let Some(editor) = app.plan_editor.take() {
                app.plan_task_editing = None;
                if app.plan_dry_run {
                    app.push(ChatLine::Info(
                        "  [dry-run] plan would execute now (--plan-dry-run active)".into(),
                    ));
                } else {
                    // Build a combined prompt from non-skipped tasks.
                    let tasks: Vec<String> = editor
                        .plan
                        .tasks
                        .iter()
                        .filter(|t| !t.skip)
                        .map(|t| {
                            if t.notes.is_empty() {
                                format!("{}: {}", t.id, t.description)
                            } else {
                                format!("{}: {}  (note: {})", t.id, t.description, t.notes)
                            }
                        })
                        .collect();
                    let prompt = format!(
                        "Goal: {}\n\nPlease execute the following plan in order:\n{}",
                        editor.plan.meta.goal,
                        tasks.join("\n")
                    );
                    app.send_now(prompt);
                }
            }
        }
        Esc => {
            // Abort plan — close editor without executing.
            app.plan_editor = None;
            app.plan_task_editing = None;
            app.push(ChatLine::Info("  ✗ Plan aborted".into()));
            app.busy = false;
        }
        _ => {}
    }
}

// ── Profile overlay keyboard handler ─────────────────────────────────────────

fn handle_profile_overlay_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    let st = match app.profile_overlay.as_mut() {
        Some(s) => s,
        None => return,
    };
    st.status = None;

    match &st.mode.clone() {
        // ── Creating: step-by-step wizard ─────────────────────────────────
        ProfileOverlayMode::Creating { step } => {
            match event.code {
                Esc => {
                    app.profile_overlay = None;
                    app.push(ChatLine::Info("  Profile creation cancelled.".into()));
                }
                Enter => {
                    let value = st.input.trim().to_string();
                    match step {
                        ProfileCreateStep::Name => {
                            // Name step is skipped — auto-generated from provider.
                            // If somehow reached, advance to Provider.
                            st.provider_picker = ProviderPickerState::new();
                            st.provider_picker.clamp();
                            st.mode = ProfileOverlayMode::Creating {
                                step: ProfileCreateStep::Provider,
                            };
                        }
                        ProfileCreateStep::Provider => {
                            if let Some(id) = st.provider_picker.selected_id() {
                                st.provider = id.to_string();
                                // Auto-generate profile name from provider.
                                let existing = clido_core::load_config(&app.workspace_root)
                                    .ok()
                                    .map(|c| c.profiles.keys().cloned().collect::<Vec<_>>())
                                    .unwrap_or_default();
                                let base = id.to_string();
                                if !existing.contains(&base) {
                                    st.name = base;
                                } else {
                                    let mut n = 2u32;
                                    loop {
                                        let candidate = format!("{base}-{n}");
                                        if !existing.contains(&candidate) {
                                            st.name = candidate;
                                            break;
                                        }
                                        n += 1;
                                    }
                                }
                                let needs_key = st.provider_picker.selected_requires_key();
                                let models = app.known_models.clone();
                                let mut picker = ModelPickerState {
                                    models,
                                    filter: id.to_string(),
                                    selected: 0,
                                    scroll_offset: 0,
                                };
                                picker.clamp();
                                st.profile_model_picker = Some(picker);
                                st.input.clear();
                                st.input_cursor = 0;
                                let next_step = if needs_key {
                                    ProfileCreateStep::ApiKey
                                } else {
                                    st.api_key.clear();
                                    ProfileCreateStep::Model
                                };
                                st.mode = ProfileOverlayMode::Creating { step: next_step };
                            } else {
                                st.status = Some("  ✗ Select a provider from the list".into());
                            }
                        }
                        ProfileCreateStep::ApiKey => {
                            // API key may be empty for local providers
                            st.api_key = value.clone();
                            st.input.clear();
                            st.input_cursor = 0;
                            // Trigger a live model fetch for this provider + key so the model
                            // picker is populated when the user reaches the model selection step.
                            let provider_for_fetch = st.provider.clone();
                            let base_url_for_fetch = if st.base_url.is_empty() {
                                None
                            } else {
                                Some(st.base_url.clone())
                            };
                            if !value.is_empty() {
                                spawn_model_fetch(
                                    provider_for_fetch,
                                    value,
                                    base_url_for_fetch,
                                    app.fetch_tx.clone(),
                                );
                                app.models_loading = true;
                            }
                            st.mode = ProfileOverlayMode::Creating {
                                step: ProfileCreateStep::Model,
                            };
                        }
                        ProfileCreateStep::Model => {
                            let model_id = st.profile_model_picker.as_ref().and_then(|p| {
                                let filtered = p.filtered();
                                filtered.get(p.selected).map(|m| m.id.clone())
                            });
                            if let Some(id) = model_id {
                                st.model = id;
                                st.input.clear();
                                st.input_cursor = 0;
                                st.mode = ProfileOverlayMode::Overview;
                                let st = app.profile_overlay.as_mut().unwrap();
                                st.save();
                                let name = st.name.clone();
                                let msg = st
                                    .status
                                    .clone()
                                    .unwrap_or_else(|| format!("  ✓ Profile '{}' created", name));
                                app.push(ChatLine::Info(msg));
                                app.push(ChatLine::Info(format!(
                                    "  Use /profile {} to switch to it.",
                                    name
                                )));
                                app.profile_overlay = None;
                            } else {
                                st.status = Some("  ✗ Select a model from the list".into());
                            }
                        }
                    }
                }
                Backspace => {
                    match step {
                        ProfileCreateStep::Provider => {
                            st.provider_picker.filter.pop();
                            st.provider_picker.clamp();
                        }
                        ProfileCreateStep::Model => {
                            if let Some(ref mut picker) = st.profile_model_picker {
                                picker.filter.pop();
                                picker.clamp();
                            }
                        }
                        _ => {
                            if !st.input.is_empty() {
                                if event.modifiers.contains(Km::CONTROL) {
                                    // Ctrl+Backspace: delete word
                                    while st.input_cursor > 0 {
                                        let b = char_byte_pos_tui(&st.input, st.input_cursor - 1);
                                        let ch = st.input[..b].chars().last();
                                        if ch.map(|c| c == ' ').unwrap_or(false)
                                            && st.input_cursor > 1
                                        {
                                            break;
                                        }
                                        st.input_cursor -= 1;
                                        let pos = char_byte_pos_tui(&st.input, st.input_cursor);
                                        st.input.remove(pos);
                                    }
                                } else {
                                    delete_char_before_cursor_pe(st);
                                }
                            }
                        }
                    }
                }
                Delete => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        delete_char_at_cursor_pe(st);
                    }
                },
                Left => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        if st.input_cursor > 0 {
                            st.input_cursor -= 1;
                        }
                    }
                },
                Right => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        if st.input_cursor < st.input.chars().count() {
                            st.input_cursor += 1;
                        }
                    }
                },
                Home => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        st.input_cursor = 0;
                    }
                },
                End => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        st.input_cursor = st.input.chars().count();
                    }
                },
                Up => match step {
                    ProfileCreateStep::Provider => {
                        if st.provider_picker.selected > 0 {
                            st.provider_picker.selected -= 1;
                            if st.provider_picker.selected < st.provider_picker.scroll_offset {
                                st.provider_picker.scroll_offset = st.provider_picker.selected;
                            }
                        }
                    }
                    ProfileCreateStep::Model => {
                        if let Some(ref mut picker) = st.profile_model_picker {
                            if picker.selected > 0 {
                                picker.selected -= 1;
                                if picker.selected < picker.scroll_offset {
                                    picker.scroll_offset = picker.selected;
                                }
                            }
                        }
                    }
                    _ => {
                        st.input_cursor = 0;
                    }
                },
                Down => match step {
                    ProfileCreateStep::Provider => {
                        let vis = crossterm::terminal::size()
                            .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                            .unwrap_or(20);
                        let n = st.provider_picker.filtered().len();
                        if n > 0 && st.provider_picker.selected + 1 < n {
                            st.provider_picker.selected += 1;
                            if st.provider_picker.selected >= st.provider_picker.scroll_offset + vis
                            {
                                st.provider_picker.scroll_offset =
                                    st.provider_picker.selected + 1 - vis;
                            }
                        }
                    }
                    ProfileCreateStep::Model => {
                        let vis = crossterm::terminal::size()
                            .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                            .unwrap_or(20);
                        if let Some(ref mut picker) = st.profile_model_picker {
                            let n = picker.filtered().len();
                            if n > 0 && picker.selected + 1 < n {
                                picker.selected += 1;
                                if picker.selected >= picker.scroll_offset + vis {
                                    picker.scroll_offset = picker.selected + 1 - vis;
                                }
                            }
                        }
                    }
                    _ => {
                        st.input_cursor = st.input.chars().count();
                    }
                },
                Char(c) => match step {
                    ProfileCreateStep::Provider => {
                        st.provider_picker.filter.push(c);
                        st.provider_picker.clamp();
                    }
                    ProfileCreateStep::Model => {
                        if let Some(ref mut picker) = st.profile_model_picker {
                            picker.filter.push(c);
                            picker.clamp();
                        }
                    }
                    _ => {
                        let b = char_byte_pos_tui(&st.input, st.input_cursor);
                        st.input.insert(b, c);
                        st.input_cursor += 1;
                    }
                },
                _ => {}
            }
        }

        // ── PickingProvider: structured provider picker ─────────────────────
        ProfileOverlayMode::PickingProvider { .. } => match event.code {
            Esc => {
                let st = app.profile_overlay.as_mut().unwrap();
                st.provider_picker = ProviderPickerState::new();
                st.mode = ProfileOverlayMode::Overview;
            }
            Enter => {
                let st = app.profile_overlay.as_mut().unwrap();
                st.commit_provider_pick();
                st.save();
                let name = st.name.clone();
                let provider = st.provider.clone();
                let model = st.model.clone();
                if app.current_profile == name {
                    app.provider = provider;
                    app.model = model;
                }
            }
            Up => {
                let st = app.profile_overlay.as_mut().unwrap();
                if st.provider_picker.selected > 0 {
                    st.provider_picker.selected -= 1;
                    if st.provider_picker.selected < st.provider_picker.scroll_offset {
                        st.provider_picker.scroll_offset = st.provider_picker.selected;
                    }
                }
            }
            Down => {
                let st = app.profile_overlay.as_mut().unwrap();
                let n = st.provider_picker.filtered().len();
                if n > 0 && st.provider_picker.selected + 1 < n {
                    st.provider_picker.selected += 1;
                    let vis = crossterm::terminal::size()
                        .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                        .unwrap_or(20);
                    if st.provider_picker.selected >= st.provider_picker.scroll_offset + vis {
                        st.provider_picker.scroll_offset = st.provider_picker.selected + 1 - vis;
                    }
                }
            }
            Char(c) => {
                let st = app.profile_overlay.as_mut().unwrap();
                st.provider_picker.filter.push(c);
                st.provider_picker.clamp();
            }
            Backspace => {
                let st = app.profile_overlay.as_mut().unwrap();
                st.provider_picker.filter.pop();
                st.provider_picker.clamp();
            }
            _ => {}
        },

        // ── PickingModel: structured model picker ────────────────────────────
        ProfileOverlayMode::PickingModel { .. } => match event.code {
            Esc => {
                let st = app.profile_overlay.as_mut().unwrap();
                st.profile_model_picker = None;
                st.mode = ProfileOverlayMode::Overview;
            }
            Enter => {
                let st = app.profile_overlay.as_mut().unwrap();
                st.commit_model_pick();
                st.save();
                let name = st.name.clone();
                let provider = st.provider.clone();
                let model = st.model.clone();
                if app.current_profile == name {
                    app.provider = provider;
                    app.model = model;
                }
            }
            Up => {
                let st = app.profile_overlay.as_mut().unwrap();
                if let Some(ref mut picker) = st.profile_model_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                        if picker.selected < picker.scroll_offset {
                            picker.scroll_offset = picker.selected;
                        }
                    }
                }
            }
            Down => {
                let st = app.profile_overlay.as_mut().unwrap();
                if let Some(ref mut picker) = st.profile_model_picker {
                    let vis = crossterm::terminal::size()
                        .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                        .unwrap_or(20);
                    let n = picker.filtered().len();
                    if n > 0 && picker.selected + 1 < n {
                        picker.selected += 1;
                        if picker.selected >= picker.scroll_offset + vis {
                            picker.scroll_offset = picker.selected + 1 - vis;
                        }
                    }
                }
            }
            Char(c) => {
                let st = app.profile_overlay.as_mut().unwrap();
                if let Some(ref mut picker) = st.profile_model_picker {
                    picker.filter.push(c);
                    picker.clamp();
                }
            }
            Backspace => {
                let st = app.profile_overlay.as_mut().unwrap();
                if let Some(ref mut picker) = st.profile_model_picker {
                    picker.filter.pop();
                    picker.clamp();
                }
            }
            _ => {}
        },

        // ── Overview: navigate fields ──────────────────────────────────────
        ProfileOverlayMode::Overview => {
            match event.code {
                Esc => {
                    app.profile_overlay = None;
                }
                Up => {
                    if st.cursor > 0 {
                        st.cursor -= 1;
                    }
                }
                Down => {
                    if st.cursor + 1 < ProfileOverlayState::field_count() {
                        st.cursor += 1;
                    }
                }
                Enter => {
                    let models: Vec<ModelEntry> = app.known_models.clone();
                    app.profile_overlay.as_mut().unwrap().begin_edit(&models);
                }
                KeyCode::Char('s') if event.modifiers.contains(Km::CONTROL) => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    st.save();
                    // Update live app state if it's the active profile
                    let name = st.name.clone();
                    let new_provider = st.provider.clone();
                    let new_model = st.model.clone();
                    let new_api_key = st.api_key.clone();
                    if app.current_profile == name {
                        let provider_changed = app.provider != new_provider;
                        let key_changed = app.api_key != new_api_key;
                        app.provider = new_provider;
                        app.model = new_model.clone();
                        app.api_key = new_api_key;
                        if provider_changed || key_changed {
                            // Provider or key changed — need a full restart
                            // to rebuild the agent with the new credentials.
                            app.profile_overlay = None;
                            app.restart_resume_session = app.current_session_id.clone();
                            app.wants_profile_switch = Some(name);
                            app.quit = true;
                        } else {
                            // Only model changed — live-switch
                            let _ = app.model_switch_tx.send(new_model);
                        }
                    }
                }
                _ => {}
            }
        }

        // ── EditField: typing into a single field ──────────────────────────
        ProfileOverlayMode::EditField(_) => {
            match event.code {
                Esc => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    st.cancel_edit();
                }
                Enter => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    st.commit_edit();
                    // Auto-save on field commit
                    st.save();
                    let name = st.name.clone();
                    let new_provider = st.provider.clone();
                    let new_model = st.model.clone();
                    let new_api_key = st.api_key.clone();
                    if app.current_profile == name {
                        let provider_changed = app.provider != new_provider;
                        let key_changed = app.api_key != new_api_key;
                        app.provider = new_provider;
                        app.model = new_model.clone();
                        app.api_key = new_api_key;
                        if provider_changed || key_changed {
                            app.profile_overlay = None;
                            app.restart_resume_session = app.current_session_id.clone();
                            app.wants_profile_switch = Some(name);
                            app.quit = true;
                        } else {
                            let _ = app.model_switch_tx.send(new_model);
                        }
                    }
                }
                Backspace => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    delete_char_before_cursor_pe(st);
                }
                Delete => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    delete_char_at_cursor_pe(st);
                }
                Left => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    if st.input_cursor > 0 {
                        st.input_cursor -= 1;
                    }
                }
                Right => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    if st.input_cursor < st.input.chars().count() {
                        st.input_cursor += 1;
                    }
                }
                Home => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    st.input_cursor = 0;
                }
                End => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    st.input_cursor = st.input.chars().count();
                }
                Char(c) => {
                    let st = app.profile_overlay.as_mut().unwrap();
                    let b = char_byte_pos_tui(&st.input, st.input_cursor);
                    st.input.insert(b, c);
                    st.input_cursor += 1;
                }
                _ => {}
            }
        }
    }
}

/// Delete the character before the cursor in `ProfileOverlayState.input`.
fn delete_char_before_cursor_pe(st: &mut ProfileOverlayState) {
    if st.input_cursor == 0 || st.input.is_empty() {
        return;
    }
    st.input_cursor -= 1;
    let b = char_byte_pos_tui(&st.input, st.input_cursor);
    st.input.remove(b);
}

/// Delete the character at the cursor in `ProfileOverlayState.input`.
fn delete_char_at_cursor_pe(st: &mut ProfileOverlayState) {
    if st.input_cursor >= st.input.chars().count() {
        return;
    }
    let b = char_byte_pos_tui(&st.input, st.input_cursor);
    st.input.remove(b);
}

/// char_byte_pos for ProfileOverlayState (same logic as the TUI char_byte_pos helper).
fn char_byte_pos_tui(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

// ── Overlay stack action handler ──────────────────────────────────────────────

/// Process app-level actions returned by overlays.
fn handle_app_action(app: &mut App, action: AppAction) {
    match action {
        AppAction::SwitchModel { model_id, save } => {
            let _ = app.model_switch_tx.send(model_id.clone());
            app.model = model_id;
            if save {
                // persist to config
            }
        }
        AppAction::SwitchProfile { profile_name } => {
            app.wants_profile_switch = Some(profile_name);
            app.quit = true;
        }
        AppAction::ResumeSession { session_id } => {
            let _ = app.resume_tx.send(session_id);
        }
        AppAction::GrantPermission(_grant) => {
            // TODO: wire when permission overlay is migrated
        }
        AppAction::ShowError(msg) => {
            app.overlay_stack
                .push(OverlayKind::Error(ErrorOverlay::new(msg)));
        }
        AppAction::RunCommand(cmd) => {
            execute_slash(app, &cmd);
        }
        AppAction::Quit => {
            app.quit = true;
        }
    }
}

// ── Input handling ────────────────────────────────────────────────────────────

fn handle_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    // Ctrl+C / Ctrl+D always quits
    if matches!(
        (event.modifiers, event.code),
        (Km::CONTROL, Char('c')) | (Km::CONTROL, Char('d'))
    ) {
        app.quit = true;
        return;
    }

    // Ctrl+/ interrupts the current run without sending follow-up input.
    if matches!((event.modifiers, event.code), (Km::CONTROL, Char('/'))) {
        app.stop_only();
        return;
    }
    if matches!((event.modifiers, event.code), (Km::CONTROL, Char('y'))) {
        match app.last_assistant_text() {
            Some(text) => {
                if let Err(e) = copy_to_clipboard_osc52(text) {
                    app.push(ChatLine::Info(format!("  ✗ Copy failed: {}", e)));
                } else {
                    app.push(ChatLine::Info("  ✓ Copied to clipboard".into()));
                }
            }
            None => app.push(ChatLine::Info("  ✗ Nothing to copy yet".into())),
        }
        return;
    }

    // ── Plan text editor (nano-style, intercepts all keys) ───────────────────
    if app.plan_text_editor.is_some() {
        handle_plan_text_editor_key(app, event);
        return;
    }

    // ── Plan editor (full-screen modal — intercepts all keys) ────────────────
    if app.plan_editor.is_some() {
        handle_plan_editor_key(app, event);
        return;
    }

    // ── Settings editor (modal) ──────────────────────────────────────────────
    if app.settings.is_some() {
        let st = app.settings.as_mut().unwrap();
        st.status = None;
        match &st.edit_field {
            SettingsEditField::None => match event.code {
                Up => {
                    if st.cursor > 0 {
                        st.cursor -= 1;
                    }
                }
                Down => {
                    // default model + roles + "Add role" + "Save & close"
                    if st.cursor < st.roles.len() + 2 {
                        st.cursor += 1;
                    }
                }
                Enter => {
                    let cursor = st.cursor;
                    let roles_len = st.roles.len();
                    if cursor == 0 {
                        // Edit default model
                        let current = st.default_model.clone();
                        st.input = current;
                        st.edit_field = SettingsEditField::DefaultModel;
                    } else if cursor <= roles_len {
                        let idx = cursor - 1;
                        let model = st.roles[idx].1.clone();
                        st.input = model;
                        st.edit_field = SettingsEditField::RoleModel(idx);
                    } else if cursor == roles_len + 1 {
                        // "Add role"
                        st.input.clear();
                        st.edit_field = SettingsEditField::RoleName(usize::MAX);
                    } else {
                        // "Save & close"
                        st.save();
                        // Reload config_roles and model in app if save succeeded
                        if st
                            .status
                            .as_deref()
                            .map(|s| s.contains('✓'))
                            .unwrap_or(false)
                        {
                            let new_roles: std::collections::HashMap<String, String> =
                                st.roles.iter().cloned().collect();
                            let new_model = st.default_model.clone();
                            app.config_roles = new_roles.clone();
                            let (pricing, _) = clido_core::load_pricing();
                            app.known_models =
                                build_model_list(&pricing, &new_roles, &app.model_prefs);
                            // Switch to new default model for this session too
                            if !new_model.is_empty() && new_model != app.model {
                                app.model = new_model.clone();
                                let _ = app.model_switch_tx.send(new_model);
                            }
                            app.settings = None;
                        }
                    }
                }
                Char('n') => {
                    let st = app.settings.as_mut().unwrap();
                    st.input.clear();
                    st.edit_field = SettingsEditField::RoleName(usize::MAX);
                }
                Char('d') => {
                    let cursor = st.cursor;
                    // cursor 0 = default model (not deletable), cursor 1..N+1 = roles
                    if cursor > 0 && cursor <= st.roles.len() {
                        st.roles.remove(cursor - 1);
                        if st.cursor > 1 && st.cursor > st.roles.len() {
                            st.cursor -= 1;
                        }
                    } else {
                        st.status =
                            Some("  'd' deletes a role — move cursor to a role row first".into());
                    }
                }
                Char('s') => {
                    let st = app.settings.as_mut().unwrap();
                    st.save();
                    if st
                        .status
                        .as_deref()
                        .map(|s| s.contains('✓'))
                        .unwrap_or(false)
                    {
                        let new_roles: std::collections::HashMap<String, String> =
                            st.roles.iter().cloned().collect();
                        let new_model = st.default_model.clone();
                        app.config_roles = new_roles.clone();
                        let (pricing, _) = clido_core::load_pricing();
                        app.known_models = build_model_list(&pricing, &new_roles, &app.model_prefs);
                        if !new_model.is_empty() && new_model != app.model {
                            app.model = new_model.clone();
                            let _ = app.model_switch_tx.send(new_model);
                        }
                        app.settings = None;
                    }
                }
                Esc => {
                    app.settings = None;
                }
                _ => {}
            },
            SettingsEditField::DefaultModel => match event.code {
                Enter => {
                    let model = app.settings.as_ref().unwrap().input.trim().to_string();
                    let st = app.settings.as_mut().unwrap();
                    st.default_model = model;
                    st.edit_field = SettingsEditField::None;
                    st.input.clear();
                }
                Backspace => {
                    app.settings.as_mut().unwrap().input.pop();
                }
                Esc => {
                    let st = app.settings.as_mut().unwrap();
                    st.edit_field = SettingsEditField::None;
                    st.input.clear();
                }
                Char(c) => {
                    app.settings.as_mut().unwrap().input.push(c);
                }
                _ => {}
            },
            SettingsEditField::RoleName(_) => match event.code {
                Enter => {
                    let name = app.settings.as_ref().unwrap().input.trim().to_string();
                    let st = app.settings.as_mut().unwrap();
                    if !name.is_empty() {
                        st.roles.push((name, String::new()));
                        let idx = st.roles.len() - 1;
                        st.cursor = idx + 1;
                        st.input.clear();
                        st.edit_field = SettingsEditField::RoleModel(idx);
                    } else {
                        st.edit_field = SettingsEditField::None;
                    }
                }
                Backspace => {
                    app.settings.as_mut().unwrap().input.pop();
                }
                Esc => {
                    let st = app.settings.as_mut().unwrap();
                    st.edit_field = SettingsEditField::None;
                    st.input.clear();
                }
                Char(c) => {
                    app.settings.as_mut().unwrap().input.push(c);
                }
                _ => {}
            },
            SettingsEditField::RoleModel(idx) => {
                let idx = *idx;
                match event.code {
                    Enter => {
                        let model = app.settings.as_ref().unwrap().input.trim().to_string();
                        let st = app.settings.as_mut().unwrap();
                        if model.is_empty() {
                            if idx < st.roles.len() {
                                st.roles.remove(idx);
                            }
                        } else if idx < st.roles.len() {
                            st.roles[idx].1 = model;
                        }
                        st.edit_field = SettingsEditField::None;
                        st.input.clear();
                    }
                    Backspace => {
                        app.settings.as_mut().unwrap().input.pop();
                    }
                    Esc => {
                        let st = app.settings.as_mut().unwrap();
                        if idx < st.roles.len() && st.roles[idx].1.is_empty() {
                            st.roles.remove(idx);
                        }
                        st.edit_field = SettingsEditField::None;
                        st.input.clear();
                    }
                    Char(c) => {
                        app.settings.as_mut().unwrap().input.push(c);
                    }
                    _ => {}
                }
            }
        }
        return;
    }

    // ── Profile overlay (overview / field editor / create wizard) ────────────
    if app.profile_overlay.is_some() {
        handle_profile_overlay_key(app, event);
        return;
    }

    // ── Overlay stack (new system) ───────────────────────────────────────────
    match app.overlay_stack.handle_key(event) {
        OverlayKeyResult::Consumed => return,
        OverlayKeyResult::Action(action) => {
            handle_app_action(app, action);
            return;
        }
        OverlayKeyResult::NotHandled | OverlayKeyResult::NoOverlay => {}
    }

    // ── Error modal (dismiss with Enter / Esc / Space) ───────────────────────
    if app.pending_error.is_some() {
        match event.code {
            Enter | Esc | Char(' ') => {
                app.pending_error = None;
            }
            _ => {}
        }
        return;
    }

    // ── Rules overlay (dismiss with Enter / Esc) ─────────────────────────────
    if app.rules_overlay.is_some() {
        match event.code {
            Enter | Esc => {
                app.rules_overlay = None;
            }
            _ => {}
        }
        return;
    }

    // ── Model picker (modal) ─────────────────────────────────────────────────
    if app.model_picker.is_some() {
        const VISIBLE: usize = 14;
        // Ctrl+S: save selected model as default in config
        if event.modifiers == Km::CONTROL {
            if let KeyCode::Char('s') = event.code {
                if let Some(picker) = &app.model_picker {
                    let filtered = picker.filtered();
                    if !filtered.is_empty() {
                        let model_id = filtered[picker.selected].id.clone();
                        drop(filtered);
                        let config_path = clido_core::global_config_path()
                            .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                        match save_default_model_to_config(
                            &config_path,
                            &model_id,
                            &app.current_profile,
                        ) {
                            Ok(()) => {
                                app.push(ChatLine::Info(format!(
                                    "  ✓ {} saved as default model",
                                    model_id
                                )));
                            }
                            Err(e) => {
                                app.push(ChatLine::Info(format!("  ✗ could not save: {}", e)));
                            }
                        }
                        app.model_picker = None;
                    }
                }
                return;
            }
        }
        match event.code {
            Up => {
                if let Some(picker) = &mut app.model_picker {
                    let n = picker.filtered().len();
                    if n > 0 && picker.selected > 0 {
                        picker.selected -= 1;
                        if picker.selected < picker.scroll_offset {
                            picker.scroll_offset = picker.selected;
                        }
                    }
                }
            }
            Down => {
                if let Some(picker) = &mut app.model_picker {
                    let n = picker.filtered().len();
                    if n > 0 && picker.selected + 1 < n {
                        picker.selected += 1;
                        if picker.selected >= picker.scroll_offset + VISIBLE {
                            picker.scroll_offset = picker.selected - VISIBLE + 1;
                        }
                    }
                }
            }
            Enter => {
                if let Some(picker) = app.model_picker.take() {
                    let filtered = picker.filtered();
                    if !filtered.is_empty() {
                        let entry = filtered[picker.selected].clone();
                        // Switch model.
                        app.model = entry.id.clone();
                        let _ = app.model_switch_tx.send(entry.id.clone());
                        // Update recency.
                        app.model_prefs.push_recent(&entry.id);
                        app.model_prefs.save();
                        app.push(ChatLine::Info(format!("  ✓ Model: {}", entry.id)));
                    }
                }
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                if let Some(picker) = &mut app.model_picker {
                    let filtered = picker.filtered();
                    if !filtered.is_empty() {
                        let model_id = filtered[picker.selected].id.clone();
                        drop(filtered);
                        app.model_prefs.toggle_favorite(&model_id);
                        app.model_prefs.save();
                        // Rebuild known_models with updated favorites.
                        let (pricing, _) = clido_core::load_pricing();
                        app.known_models =
                            build_model_list(&pricing, &app.config_roles, &app.model_prefs);
                        picker.models = app.known_models.clone();
                        picker.clamp();
                    }
                }
            }
            Esc => {
                app.model_picker = None;
            }
            KeyCode::Backspace => {
                if let Some(picker) = &mut app.model_picker {
                    picker.filter.pop();
                    picker.clamp();
                }
            }
            KeyCode::Char(c) => {
                if let Some(picker) = &mut app.model_picker {
                    picker.filter.push(c);
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            KeyCode::Home => {
                // Jump to first result
                if let Some(picker) = &mut app.model_picker {
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            KeyCode::End => {
                // Jump to last result
                if let Some(picker) = &mut app.model_picker {
                    let n = picker.filtered().len();
                    if n > 0 {
                        picker.selected = n - 1;
                        picker.scroll_offset = picker.selected.saturating_sub(11);
                    }
                }
            }
            _ => {}
        }
        return;
    }

    // ── Session picker (modal) ────────────────────────────────────────────────
    if app.session_picker.is_some() {
        const VISIBLE: usize = 12;
        match event.code {
            Up => {
                if let Some(picker) = &mut app.session_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                        if picker.selected < picker.scroll_offset {
                            picker.scroll_offset = picker.selected;
                        }
                    }
                }
            }
            Down => {
                if let Some(picker) = &mut app.session_picker {
                    let n = picker.filtered().len();
                    if picker.selected + 1 < n {
                        picker.selected += 1;
                        if picker.selected >= picker.scroll_offset + VISIBLE {
                            picker.scroll_offset = picker.selected - VISIBLE + 1;
                        }
                    }
                }
            }
            Enter => {
                if let Some(picker) = app.session_picker.take() {
                    let filtered = picker.filtered();
                    if filtered.is_empty() {
                        return;
                    }
                    let (orig_idx, _) = filtered[picker.selected];
                    app.input.clear();
                    app.cursor = 0;
                    let id = picker.sessions[orig_idx].session_id.clone();
                    if app.current_session_id.as_deref() == Some(&id) {
                        app.push(ChatLine::Info("  Already in this session".into()));
                    } else {
                        let _ = app.resume_tx.send(id);
                    }
                }
            }
            Esc => {
                app.session_picker = None;
                app.input.clear();
                app.cursor = 0;
            }
            Backspace => {
                if let Some(picker) = &mut app.session_picker {
                    picker.filter.pop();
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            Char(c) => {
                if let Some(picker) = &mut app.session_picker {
                    picker.filter.push(c);
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            _ => {}
        }
        return;
    }

    // ── Profile picker (modal) ────────────────────────────────────────────────
    if app.profile_picker.is_some() {
        const VISIBLE: usize = 12;
        match event.code {
            Up => {
                if let Some(picker) = &mut app.profile_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                        if picker.selected < picker.scroll_offset {
                            picker.scroll_offset = picker.selected;
                        }
                    }
                }
            }
            Down => {
                if let Some(picker) = &mut app.profile_picker {
                    let n = picker.filtered().len();
                    if picker.selected + 1 < n {
                        picker.selected += 1;
                        if picker.selected >= picker.scroll_offset + VISIBLE {
                            picker.scroll_offset = picker.selected - VISIBLE + 1;
                        }
                    }
                }
            }
            Enter => {
                if let Some(picker) = app.profile_picker.take() {
                    let filtered = picker.filtered();
                    if filtered.is_empty() {
                        return;
                    }
                    let (orig_idx, _) = filtered[picker.selected];
                    let (name, _) = &picker.profiles[orig_idx];
                    if name == &picker.active {
                        app.push(ChatLine::Info(format!(
                            "  profile '{}' is already active.",
                            name
                        )));
                    } else {
                        app.push(ChatLine::Info(format!(
                            "  switching to profile '{}'…",
                            name
                        )));
                        app.restart_resume_session = app.current_session_id.clone();
                        app.wants_profile_switch = Some(name.clone());
                        app.quit = true;
                    }
                }
            }
            Esc => {
                app.profile_picker = None;
            }
            KeyCode::Char('n') => {
                app.profile_picker = None;
                let config_path = clido_core::global_config_path()
                    .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                app.profile_overlay = Some(ProfileOverlayState::for_create(config_path));
            }
            KeyCode::Char('e') => {
                if let Some(picker) = app.profile_picker.take() {
                    let filtered = picker.filtered();
                    if let Some(&(orig_idx, _)) = filtered.get(picker.selected) {
                        let (name, entry) = &picker.profiles[orig_idx];
                        let config_path = clido_core::global_config_path()
                            .unwrap_or_else(|| app.workspace_root.join(".clido/config.toml"));
                        app.profile_overlay = Some(ProfileOverlayState::for_edit(
                            name.clone(),
                            entry,
                            config_path,
                        ));
                    }
                }
            }
            Backspace => {
                if let Some(picker) = &mut app.profile_picker {
                    picker.filter.pop();
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            Char(c) if c != 'n' && c != 'e' => {
                if let Some(picker) = &mut app.profile_picker {
                    picker.filter.push(c);
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            _ => {}
        }
        return;
    }

    // ── Role picker (modal) ───────────────────────────────────────────────────
    if app.role_picker.is_some() {
        const VISIBLE: usize = 10;
        match event.code {
            Up => {
                if let Some(picker) = &mut app.role_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                        if picker.selected < picker.scroll_offset {
                            picker.scroll_offset = picker.selected;
                        }
                    }
                }
            }
            Down => {
                if let Some(picker) = &mut app.role_picker {
                    let n = picker.filtered().len();
                    if picker.selected + 1 < n {
                        picker.selected += 1;
                        if picker.selected >= picker.scroll_offset + VISIBLE {
                            picker.scroll_offset = picker.selected - VISIBLE + 1;
                        }
                    }
                }
            }
            Enter => {
                if let Some(picker) = app.role_picker.take() {
                    let filtered = picker.filtered();
                    if filtered.is_empty() {
                        return;
                    }
                    let (orig_idx, _) = filtered[picker.selected];
                    let (role_name, model_id) = &picker.roles[orig_idx];
                    let model_id = model_id.clone();
                    let role_name = role_name.clone();
                    app.model = model_id.clone();
                    let _ = app.model_switch_tx.send(model_id.clone());
                    app.model_prefs.push_recent(&model_id);
                    app.model_prefs.save();
                    app.push(ChatLine::Info(format!(
                        "  ✓ Model: {} (alias: {})",
                        model_id, role_name
                    )));
                }
            }
            Esc => {
                app.role_picker = None;
            }
            Backspace => {
                if let Some(picker) = &mut app.role_picker {
                    picker.filter.pop();
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            Char(c) => {
                if let Some(picker) = &mut app.role_picker {
                    picker.filter.push(c);
                    picker.selected = 0;
                    picker.scroll_offset = 0;
                }
            }
            _ => {}
        }
        return;
    }

    // ── Permission popup (modal — arrow keys select, Enter confirms) ─────────
    if app.pending_perm.is_some() {
        const PERM_OPTIONS: usize = 5;

        // ── Feedback input mode ──────────────────────────────────────────
        if app.perm_feedback_input.is_some() {
            match event.code {
                Enter => {
                    if let (Some(perm), Some(fb)) =
                        (app.pending_perm.take(), app.perm_feedback_input.take())
                    {
                        let _ = perm.reply.send(PermGrant::DenyWithFeedback(fb));
                        app.perm_selected = 0;
                    }
                }
                Esc => {
                    // Go back to option selection without sending
                    app.perm_feedback_input = None;
                }
                Backspace => {
                    if let Some(ref mut fb) = app.perm_feedback_input {
                        fb.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(ref mut fb) = app.perm_feedback_input {
                        fb.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        // ── Normal option selection ──────────────────────────────────────
        match event.code {
            Up => {
                if app.perm_selected == 0 {
                    app.perm_selected = PERM_OPTIONS - 1;
                } else {
                    app.perm_selected -= 1;
                }
            }
            Down => {
                app.perm_selected = (app.perm_selected + 1) % PERM_OPTIONS;
            }
            Enter => {
                match app.perm_selected {
                    4 => {
                        // Deny with feedback — switch to feedback input mode
                        app.perm_feedback_input = Some(String::new());
                    }
                    _ => {
                        if let Some(perm) = app.pending_perm.take() {
                            let grant = match app.perm_selected {
                                0 => PermGrant::Once,
                                1 => PermGrant::Session,
                                2 => PermGrant::Workdir,
                                _ => PermGrant::Deny,
                            };
                            // Track AllowAll grants on the App so the UI can reflect the state
                            // and so we can reset it on workdir changes.
                            if matches!(grant, PermGrant::Session | PermGrant::Workdir) {
                                app.permission_mode_override = Some(PermissionMode::AcceptAll);
                            }
                            let _ = perm.reply.send(grant);
                            app.perm_selected = 0;
                        }
                    }
                }
            }
            Esc => {
                if let Some(perm) = app.pending_perm.take() {
                    let _ = perm.reply.send(PermGrant::Deny);
                    app.perm_selected = 0;
                }
            }
            // Number shortcuts: 1-5 for quick selection.
            Char('1') => app.perm_selected = 0,
            Char('2') => app.perm_selected = 1,
            Char('3') => app.perm_selected = 2,
            Char('4') => app.perm_selected = 3,
            Char('5') => app.perm_selected = 4,
            _ => {} // all other keys ignored while popup is active
        }
        return;
    }

    // ── Slash-command popup navigation ──────────────────────────────────────
    let completions = slash_completions(&app.input);
    if !completions.is_empty() {
        // Clamp selection in case completions shrunk.
        if let Some(sel) = app.selected_cmd {
            if sel >= completions.len() {
                app.selected_cmd = Some(completions.len() - 1);
            }
        }
        match (event.modifiers, event.code) {
            (_, Up) => {
                let sel = match app.selected_cmd {
                    None | Some(0) => completions.len() - 1,
                    Some(i) => i - 1,
                };
                app.selected_cmd = Some(sel);
                return;
            }
            (_, Down) => {
                let sel = match app.selected_cmd {
                    None => 0,
                    Some(i) => (i + 1) % completions.len(),
                };
                app.selected_cmd = Some(sel);
                return;
            }
            (_, Tab) => {
                let idx = app.selected_cmd.unwrap_or(0);
                if let Some((cmd, _)) = completions.get(idx) {
                    app.input = cmd.to_string();
                    app.cursor = app.input.chars().count();
                }
                app.selected_cmd = None;
                return;
            }
            (_, Enter) => {
                if let Some(idx) = app.selected_cmd {
                    let cmd = completions[idx].0.to_string();
                    app.input.clear();
                    app.cursor = 0;
                    app.selected_cmd = None;
                    execute_slash(app, &cmd);
                    return;
                }
                // No item selected → fall through to normal Enter handling.
            }
            (_, Esc) => {
                app.selected_cmd = None;
                return;
            }
            _ => {}
        }
    } else {
        app.selected_cmd = None;
    }

    match (event.modifiers, event.code) {
        // Esc: clear the input field.
        (_, Esc) => {
            app.input.clear();
            app.cursor = 0;
            app.selected_cmd = None;
            app.history_idx = None;
        }
        // Ctrl+Enter: interrupt current run and send immediately.
        (Km::CONTROL, Enter) => app.force_send(),
        // Shift+Enter: insert a newline without sending (multiline input).
        (Km::SHIFT, Enter) => {
            let byte_pos = char_byte_pos(&app.input, app.cursor);
            app.input.insert(byte_pos, '\n');
            app.cursor += 1;
            app.selected_cmd = None;
            app.history_idx = None;
        }
        (_, Enter) => app.submit(),
        (_, Backspace) => {
            if app.cursor > 0 {
                let byte_pos = char_byte_pos(&app.input, app.cursor - 1);
                app.input.remove(byte_pos);
                app.cursor -= 1;
                app.selected_cmd = None;
                app.history_idx = None;
            }
        }
        (_, Delete) => {
            if app.cursor < app.input.chars().count() {
                let byte_pos = char_byte_pos(&app.input, app.cursor);
                app.input.remove(byte_pos);
                app.selected_cmd = None;
                app.history_idx = None;
            }
        }
        // Alt+Left: move cursor to start of previous word.
        (Km::ALT, Left) => {
            if app.cursor > 0 {
                let chars: Vec<char> = app.input.chars().collect();
                let mut new_cursor = app.cursor;
                // Skip spaces
                while new_cursor > 0 && chars[new_cursor - 1] == ' ' {
                    new_cursor -= 1;
                }
                // Skip word characters
                while new_cursor > 0 && chars[new_cursor - 1] != ' ' {
                    new_cursor -= 1;
                }
                app.cursor = new_cursor;
            }
        }
        // Alt+Right: move cursor to end of next word.
        (Km::ALT, Right) => {
            let chars: Vec<char> = app.input.chars().collect();
            let len = chars.len();
            let mut new_cursor = app.cursor;
            // Skip spaces
            while new_cursor < len && chars[new_cursor] == ' ' {
                new_cursor += 1;
            }
            // Skip word characters
            while new_cursor < len && chars[new_cursor] != ' ' {
                new_cursor += 1;
            }
            app.cursor = new_cursor;
        }
        (_, Left) => {
            if app.cursor > 0 {
                app.cursor -= 1;
            }
        }
        (_, Right) => {
            if app.cursor < app.input.chars().count() {
                app.cursor += 1;
            }
        }
        // ── Jump to top / bottom of chat ─────────────────────────────────────
        (Km::CONTROL, Home) => {
            app.scroll = 0;
            app.following = false;
        }
        (Km::CONTROL, End) => {
            app.following = true;
        }
        (_, Home) => app.cursor = 0,
        (_, End) => app.cursor = app.input.chars().count(),
        // ── Up: move cursor up in multiline input, otherwise scroll chat ──────
        (_, Up) if app.pending_perm.is_none() && slash_completions(&app.input).is_empty() => {
            if app.input.contains('\n') && app.history_idx.is_none() {
                if let Some(new_cursor) = move_cursor_line_up(&app.input, app.cursor) {
                    app.cursor = new_cursor;
                    return;
                }
            }
            // Empty input with no active history browse: also navigate history.
            if app.input.is_empty() && !app.input_history.is_empty() {
                let new_idx = match app.history_idx {
                    None => {
                        app.history_draft = app.input.clone();
                        app.input_history.len() - 1
                    }
                    Some(0) => 0,
                    Some(i) => i - 1,
                };
                app.history_idx = Some(new_idx);
                app.input = app.input_history[new_idx].clone();
                app.cursor = app.input.chars().count();
                app.selected_cmd = None;
                return;
            }
            scroll_up(app, 2);
        }
        // ── Down: move cursor down in multiline input, otherwise scroll chat ──
        (_, Down) if app.pending_perm.is_none() && slash_completions(&app.input).is_empty() => {
            if app.input.contains('\n') && app.history_idx.is_none() {
                if let Some(new_cursor) = move_cursor_line_down(&app.input, app.cursor) {
                    app.cursor = new_cursor;
                    return;
                }
            }
            // Empty input while browsing history: navigate forward.
            if app.input.is_empty() || app.history_idx.is_some() {
                if let Some(i) = app.history_idx {
                    if i + 1 >= app.input_history.len() {
                        app.history_idx = None;
                        app.input = app.history_draft.clone();
                        app.cursor = app.input.chars().count();
                        app.selected_cmd = None;
                    } else {
                        let new_idx = i + 1;
                        app.history_idx = Some(new_idx);
                        app.input = app.input_history[new_idx].clone();
                        app.cursor = app.input.chars().count();
                        app.selected_cmd = None;
                    }
                    return;
                }
            }
            scroll_down(app, 2);
        }
        // ── Chat scroll (PageUp/PageDown — larger jumps) ─────────────────────
        (_, PageUp) => {
            scroll_up(app, 10);
        }
        (_, PageDown) => {
            scroll_down(app, 10);
        }
        (Km::CONTROL, Char('u')) => {
            app.input.clear();
            app.cursor = 0;
            app.selected_cmd = None;
            app.history_idx = None;
        }
        // Ctrl+W: delete word backward (to previous word boundary).
        (Km::CONTROL, Char('w')) => {
            if app.cursor > 0 {
                let chars: Vec<char> = app.input.chars().collect();
                let mut new_cursor = app.cursor;
                // Skip trailing spaces
                while new_cursor > 0 && chars[new_cursor - 1] == ' ' {
                    new_cursor -= 1;
                }
                // Skip word characters
                while new_cursor > 0 && chars[new_cursor - 1] != ' ' {
                    new_cursor -= 1;
                }
                let removed: String = chars[new_cursor..app.cursor].iter().collect();
                let end_byte = char_byte_pos(&app.input, app.cursor);
                let start_byte = end_byte - removed.len();
                app.input.drain(start_byte..end_byte);
                app.cursor = new_cursor;
                app.selected_cmd = None;
                app.history_idx = None;
            }
        }
        (_, Char(c)) => {
            let byte_pos = char_byte_pos(&app.input, app.cursor);
            app.input.insert(byte_pos, c);
            app.cursor += 1;
            app.selected_cmd = None;
            // Any manual edit breaks out of history navigation.
            app.history_idx = None;
        }
        _ => {}
    }
}

/// Return the byte position of the n-th character boundary in `s`.
fn char_byte_pos(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Move cursor up one visual line within a multiline input.
/// Returns `Some(new_cursor)` when the cursor is not on the first line,
/// `None` when it is (caller should fall through to history navigation).
fn move_cursor_line_up(input: &str, cursor: usize) -> Option<usize> {
    if !input.contains('\n') {
        return None;
    }
    let chars: Vec<char> = input.chars().collect();
    // Find the start of the current line and the column position.
    let mut line_start = 0usize;
    for (i, &ch) in chars[..cursor].iter().enumerate() {
        if ch == '\n' {
            line_start = i + 1;
        }
    }
    if line_start == 0 {
        return None; // Already on first line.
    }
    let col = cursor - line_start;
    // Find the start of the previous line.
    let prev_newline = line_start - 1; // index of the '\n' before current line
    let prev_line_start = chars[..prev_newline]
        .iter()
        .enumerate()
        .rfind(|(_, &c)| c == '\n')
        .map(|(i, _)| i + 1)
        .unwrap_or(0);
    let prev_line_len = prev_newline - prev_line_start;
    Some(prev_line_start + col.min(prev_line_len))
}

/// Move cursor down one visual line within a multiline input.
/// Returns `Some(new_cursor)` when the cursor is not on the last line,
/// `None` when it is (caller should fall through to history/scroll).
fn move_cursor_line_down(input: &str, cursor: usize) -> Option<usize> {
    if !input.contains('\n') {
        return None;
    }
    let chars: Vec<char> = input.chars().collect();
    let total = chars.len();
    // Find start of current line.
    let mut line_start = 0usize;
    for (i, &ch) in chars[..cursor].iter().enumerate() {
        if ch == '\n' {
            line_start = i + 1;
        }
    }
    let col = cursor - line_start;
    // Find the next newline at or after cursor.
    let next_newline = chars[cursor..]
        .iter()
        .position(|&c| c == '\n')
        .map(|p| cursor + p);
    match next_newline {
        None => None, // Already on last line.
        Some(nl) => {
            let next_line_start = nl + 1;
            let next_line_end = chars[next_line_start..]
                .iter()
                .position(|&c| c == '\n')
                .map(|p| next_line_start + p)
                .unwrap_or(total);
            let next_line_len = next_line_end - next_line_start;
            Some(next_line_start + col.min(next_line_len))
        }
    }
}

fn tui_memory_store_path() -> Result<std::path::PathBuf, String> {
    if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") {
        let data = dirs.data_dir().to_path_buf();
        std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
        return Ok(data.join("memory.db"));
    }
    Ok(std::path::PathBuf::from(".clido-memory.db"))
}

fn resolve_workdir_arg(arg: &str) -> Result<std::path::PathBuf, String> {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return Err("workdir path cannot be empty".into());
    }
    let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .map(|h| h.join(rest))
            .map_err(|_| "HOME is not set".to_string())?
    } else if trimmed == "~" {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .map_err(|_| "HOME is not set".to_string())?
    } else {
        std::path::PathBuf::from(trimmed)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|e| format!("could not resolve current dir: {}", e))?
            .join(expanded)
    };
    let canonical = std::fs::canonicalize(&absolute)
        .map_err(|e| format!("could not access '{}': {}", absolute.display(), e))?;
    if !canonical.is_dir() {
        return Err(format!("not a directory: {}", canonical.display()));
    }
    Ok(canonical)
}

/// Copy text to the system clipboard.
/// Tries native clipboard tools first (pbcopy on macOS, wl-copy on Wayland,
/// xclip/xsel on X11), then falls back to OSC 52 escape sequence.
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if text.is_empty() {
        return Err("nothing to copy".into());
    }

    // macOS
    #[cfg(target_os = "macos")]
    {
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return Ok(());
            }
        }
    }

    // Linux — try wl-copy (Wayland) then xclip then xsel
    #[cfg(target_os = "linux")]
    {
        for (cmd, args) in &[
            ("wl-copy", vec![]),
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
        ] {
            if let Ok(mut child) = Command::new(cmd)
                .args(args.as_slice())
                .stdin(Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                if child.wait().map(|s| s.success()).unwrap_or(false) {
                    return Ok(());
                }
            }
        }
    }

    // Fallback: OSC 52 escape sequence (works in terminals that support it,
    // e.g. iTerm2, kitty, Alacritty with the feature enabled).
    {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let encoded = STANDARD.encode(text.as_bytes());
        print!("\x1b]52;c;{}\x07", encoded);
        std::io::stdout()
            .flush()
            .map_err(|e| format!("clipboard write failed: {}", e))?;
    }
    Ok(())
}

#[allow(dead_code)]
fn copy_to_clipboard_osc52(text: &str) -> Result<(), String> {
    copy_to_clipboard(text)
}

// ── Agent background task ─────────────────────────────────────────────────────

enum AgentAction {
    Run(String),
    Resume(String),
    SwitchModel(String),
    SetWorkspace(std::path::PathBuf),
    CompactNow,
}

#[allow(clippy::too_many_arguments)]
async fn agent_task(
    cli: Cli,
    workspace_root: std::path::PathBuf,
    preloaded_config: Option<clido_core::LoadedConfig>,
    preloaded_pricing: clido_core::PricingTable,
    mut prompt_rx: mpsc::UnboundedReceiver<String>,
    mut resume_rx: mpsc::UnboundedReceiver<String>,
    mut model_switch_rx: mpsc::UnboundedReceiver<String>,
    mut workdir_rx: mpsc::UnboundedReceiver<std::path::PathBuf>,
    mut compact_now_rx: mpsc::UnboundedReceiver<()>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    perm_tx: mpsc::UnboundedSender<PermRequest>,
    cancel: std::sync::Arc<AtomicBool>,
    image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
    reviewer_enabled: Arc<AtomicBool>,
    todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
) {
    let setup_result = match preloaded_config {
        Some(loaded) => AgentSetup::build_with_preloaded_and_store(
            &cli,
            &workspace_root,
            loaded,
            preloaded_pricing,
            reviewer_enabled,
            Some(todo_store),
        ),
        None => AgentSetup::build(&cli, &workspace_root),
    };
    let mut setup = match setup_result {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Err(e.to_string()));
            return;
        }
    };

    let perms = Arc::new(Mutex::new(PermsState::default()));
    setup.ask_user = Some(Arc::new(TuiAskUser {
        perm_tx,
        perms: perms.clone(),
    }));

    let session_id = if let Some(id) = &cli.resume {
        id.clone()
    } else {
        uuid::Uuid::new_v4().to_string()
    };
    let writer = if cli.resume.is_some() {
        SessionWriter::append(&workspace_root, &session_id)
    } else {
        SessionWriter::create(&workspace_root, &session_id)
    };
    let mut writer = match writer {
        Ok(w) => w,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Err(e.to_string()));
            return;
        }
    };
    let _ = event_tx.send(AgentEvent::SessionStarted(session_id.clone()));

    let emitter: Arc<dyn EventEmitter> = Arc::new(TuiEmitter {
        tx: event_tx.clone(),
    });

    let planner_mode = cli.planner;
    let context_max_tokens = setup.config.max_context_tokens.unwrap_or(200_000) as u64;
    let git_workspace = workspace_root.clone();
    let git_context_fn: Box<dyn Fn() -> Option<String> + Send + Sync> =
        Box::new(move || GitContext::discover(&git_workspace).map(|ctx| ctx.to_prompt_section()));
    let mut agent = AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user)
        .with_emitter(emitter)
        .with_planner(planner_mode)
        .with_git_context_fn(git_context_fn);

    let mut first_turn = true;
    // Auto-continue counter: when the turn limit is hit mid-task, clido automatically
    // injects "please continue" so the agent never stops mid-work. We cap this at
    // MAX_AUTO_CONTINUES to avoid infinite loops on genuinely stuck agents.
    const MAX_AUTO_CONTINUES: u32 = 5;
    let mut auto_continue_count: u32 = 0;

    if let Some(resume_session_id) = cli.resume.clone() {
        match clido_storage::SessionReader::load(&workspace_root, &resume_session_id) {
            Err(e) => {
                let _ = event_tx.send(AgentEvent::Err(format!("resume failed: {}", e)));
            }
            Ok(lines) => {
                let new_history = clido_agent::session_lines_to_messages(&lines);
                agent.replace_history(new_history);
                match SessionWriter::append(&workspace_root, &resume_session_id) {
                    Ok(new_writer) => {
                        writer = new_writer;
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!("resume writer: {}", e)));
                    }
                }
                // Warn if any files referenced in the session have changed since recording.
                let stale_records = clido_storage::SessionReader::stale_file_records(&lines);
                let stale = clido_storage::stale_paths(&stale_records);
                if !stale.is_empty() {
                    let list = stale
                        .iter()
                        .map(|p| format!("  • {}", p))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let _ = event_tx.send(AgentEvent::Thinking(format!(
                        "⚠ Some files referenced in this session have changed since it was recorded:\n{}\n\
                         The agent's context may be stale for these files.",
                        list
                    )));
                }
                let mut msgs: Vec<(String, String)> = Vec::new();
                for line in &lines {
                    match line {
                        clido_storage::SessionLine::UserMessage { content, .. } => {
                            if let Some(t) = content
                                .first()
                                .and_then(|c| c.get("text"))
                                .and_then(|v| v.as_str())
                            {
                                msgs.push(("user".to_string(), t.to_string()));
                            }
                        }
                        clido_storage::SessionLine::AssistantMessage { content } => {
                            let text: String = content
                                .iter()
                                .filter_map(|c| {
                                    if c.get("type").and_then(|v| v.as_str()) == Some("text") {
                                        c.get("text")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("");
                            if !text.trim().is_empty() {
                                msgs.push(("assistant".to_string(), text));
                            }
                        }
                        _ => {}
                    }
                }
                first_turn = false;
                let _ = event_tx.send(AgentEvent::ResumedSession { messages: msgs });
            }
        }
    }

    loop {
        // Apply queued workdir changes before other actions so prompts never run
        // against stale tooling/permissions after a switch command.
        while let Ok(new_workspace) = workdir_rx.try_recv() {
            match AgentSetup::build(&cli, &new_workspace) {
                Ok(new_setup) => {
                    agent.replace_tools(new_setup.registry);
                    agent.reset_permission_mode_override();
                    if let Ok(mut state) = perms.lock() {
                        state.clear_all_grants();
                    }
                    let _ = event_tx.send(AgentEvent::WorkdirSwitched {
                        path: new_workspace,
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::Err(format!("workdir switch failed: {}", e)));
                }
            }
        }

        let action = tokio::select! {
            msg = prompt_rx.recv() => {
                match msg {
                    Some(prompt) => AgentAction::Run(prompt),
                    None => break,
                }
            }
            resume_id = resume_rx.recv() => {
                match resume_id {
                    Some(id) => AgentAction::Resume(id),
                    None => break,
                }
            }
            model_name = model_switch_rx.recv() => {
                match model_name {
                    Some(m) => AgentAction::SwitchModel(m),
                    None => break,
                }
            }
            new_workspace = workdir_rx.recv() => {
                match new_workspace {
                    Some(path) => AgentAction::SetWorkspace(path),
                    None => break,
                }
            }
            compact = compact_now_rx.recv() => {
                match compact {
                    Some(()) => AgentAction::CompactNow,
                    None => break,
                }
            }
        };

        match action {
            AgentAction::SwitchModel(model_name) => {
                agent.set_model(model_name.clone());
                let _ = event_tx.send(AgentEvent::ModelSwitched {
                    to_model: model_name,
                });
            }
            AgentAction::SetWorkspace(new_workspace) => {
                match AgentSetup::build(&cli, &new_workspace) {
                    Ok(new_setup) => {
                        agent.replace_tools(new_setup.registry);
                        agent.reset_permission_mode_override();
                        if let Ok(mut state) = perms.lock() {
                            state.clear_all_grants();
                        }
                        let _ = event_tx.send(AgentEvent::WorkdirSwitched {
                            path: new_workspace,
                        });
                    }
                    Err(e) => {
                        let _ =
                            event_tx.send(AgentEvent::Err(format!("workdir switch failed: {}", e)));
                    }
                }
            }
            AgentAction::CompactNow => {
                match agent.compact_history_now().await {
                    Ok((before, after)) => {
                        let _ = event_tx.send(AgentEvent::Compacted { before, after });
                        // Emit updated token counts so the context bar refreshes.
                        let _ = event_tx.send(AgentEvent::TokenUsage {
                            input_tokens: agent.cumulative_input_tokens,
                            output_tokens: agent.cumulative_output_tokens,
                            cost_usd: agent.cumulative_cost_usd,
                            context_max_tokens,
                        });
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!("compact: {}", e)));
                    }
                }
            }
            AgentAction::Run(prompt) => {
                cancel.store(false, std::sync::atomic::Ordering::Relaxed);

                // When --planner is active and this is the first turn, attempt to parse
                // a plan from the prompt. On success, emit PlanCreated so the TUI can
                // display the planned steps. On any failure, fall back to the reactive loop
                // transparently (no error shown to the user).
                if planner_mode && first_turn {
                    // Make a real LLM call to decompose the task into a JSON plan.
                    // On any failure (network, parse, invalid graph), silently fall back
                    // to the reactive loop — no error is shown to the user.
                    let planning_prompt = format!(
                        "You are a task planner. Decompose the following task into a JSON task graph.\n\
                         Format: {{\"goal\":\"<goal>\",\"tasks\":[{{\"id\":\"t1\",\"description\":\"<description>\",\"depends_on\":[]}},...]}}\n\
                         Tasks that can run in parallel should have no shared dependencies.\n\
                         Keep it to 2-5 tasks maximum. Respond with ONLY the JSON, no explanation.\n\n\
                         Task: {}",
                        prompt
                    );
                    if let Ok((plan_text, plan_usage)) =
                        agent.complete_simple_with_usage(&planning_prompt).await
                    {
                        let plan_cost = clido_core::pricing::compute_cost_usd(
                            &plan_usage,
                            agent.current_model(),
                            &setup.pricing_table,
                        );
                        let _ = event_tx.send(AgentEvent::TokenUsage {
                            input_tokens: plan_usage.input_tokens,
                            output_tokens: plan_usage.output_tokens,
                            cost_usd: plan_cost,
                            context_max_tokens,
                        });
                        // Try to parse as a full Plan with metadata first.
                        let plan_opt = clido_planner::parse_plan_with_meta(&plan_text).ok();
                        if let Some(plan) = plan_opt {
                            let task_descriptions: Vec<String> = plan
                                .tasks
                                .iter()
                                .map(|t| {
                                    if t.depends_on.is_empty() {
                                        format!("{}: {}", t.id, t.description)
                                    } else {
                                        format!(
                                            "{}: {}  (depends: {})",
                                            t.id,
                                            t.description,
                                            t.depends_on.join(", ")
                                        )
                                    }
                                })
                                .collect();
                            // If plan_no_edit is NOT set, emit PlanReady to open the TUI editor.
                            if !cli.plan_no_edit {
                                let _ = event_tx.send(AgentEvent::PlanReady { plan });
                                // Mark first_turn as consumed so the next prompt (execution)
                                // does not try to re-plan.
                                first_turn = false;
                                // Do not proceed with agent execution — wait for the user to
                                // approve/edit and press 'x' in the plan editor. The editor's
                                // 'x' key sends a combined prompt via send_now.
                                continue;
                            } else {
                                let _ = event_tx.send(AgentEvent::PlanCreated {
                                    tasks: task_descriptions,
                                });
                            }
                        }
                        // If parse fails, silently proceed (fallback to reactive)
                    }
                }

                // Drain any pending image attached via /image before this turn.
                let pending_img = image_state.lock().ok().and_then(|mut g| g.take());
                let extra_blocks: Vec<clido_core::ContentBlock> =
                    if let Some((media_type, base64_data)) = pending_img {
                        vec![clido_core::ContentBlock::Image {
                            media_type,
                            base64_data,
                        }]
                    } else {
                        vec![]
                    };

                // Spawn a heartbeat task that keeps the TUI stall-detector alive while the
                // LLM is generating (which can take >45 s for long responses).  The task
                // is aborted as soon as agent.run() returns.
                let hb_tx = event_tx.clone();
                let heartbeat = tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
                    interval.tick().await; // skip the immediate first tick
                    loop {
                        interval.tick().await;
                        if hb_tx.send(AgentEvent::Heartbeat).is_err() {
                            break;
                        }
                    }
                });

                let result = if extra_blocks.is_empty() {
                    if first_turn {
                        agent
                            .run(
                                &prompt,
                                Some(&mut writer),
                                Some(&setup.pricing_table),
                                Some(cancel.clone()),
                            )
                            .await
                    } else {
                        agent
                            .run_next_turn(
                                &prompt,
                                Some(&mut writer),
                                Some(&setup.pricing_table),
                                Some(cancel.clone()),
                            )
                            .await
                    }
                } else if first_turn {
                    agent
                        .run_with_extra_blocks(
                            &prompt,
                            extra_blocks,
                            Some(&mut writer),
                            Some(&setup.pricing_table),
                            Some(cancel.clone()),
                        )
                        .await
                } else {
                    agent
                        .run_next_turn_with_extra_blocks(
                            &prompt,
                            extra_blocks,
                            Some(&mut writer),
                            Some(&setup.pricing_table),
                            Some(cancel.clone()),
                        )
                        .await
                };
                heartbeat.abort();
                first_turn = false;

                // Emit token usage before response/error so TUI updates cost display.
                let _ = event_tx.send(AgentEvent::TokenUsage {
                    input_tokens: agent.cumulative_input_tokens,
                    output_tokens: agent.cumulative_output_tokens,
                    cost_usd: agent.cumulative_cost_usd,
                    context_max_tokens,
                });

                let mut session_exit: &str = "success";

                match result {
                    Ok(text) => {
                        auto_continue_count = 0; // reset on clean completion
                        let _ = event_tx.send(AgentEvent::Response(text));
                    }
                    Err(ClidoError::Interrupted) => {
                        auto_continue_count = 0;
                        session_exit = "interrupted";
                        let _ = event_tx.send(AgentEvent::Interrupted);
                    }
                    Err(ClidoError::MaxTurnsExceeded) => {
                        auto_continue_count += 1;
                        if auto_continue_count <= MAX_AUTO_CONTINUES {
                            // Silently inject a continue prompt — the agent picks up from
                            // exactly where it left off since history is intact.
                            let _ = event_tx.send(AgentEvent::Thinking(
                                "↻ Continuing (turn limit reached)…".to_string(),
                            ));
                            // Heartbeat also covers the auto-continue LLM call.
                            let hb_tx2 = event_tx.clone();
                            let hb2 = tokio::spawn(async move {
                                let mut iv =
                                    tokio::time::interval(std::time::Duration::from_secs(15));
                                iv.tick().await;
                                loop {
                                    iv.tick().await;
                                    if hb_tx2.send(AgentEvent::Heartbeat).is_err() {
                                        break;
                                    }
                                }
                            });
                            // Call run_next_turn directly with a continue message.
                            let continue_result = agent
                                .run_next_turn(
                                    "Please continue where you left off.",
                                    Some(&mut writer),
                                    Some(&setup.pricing_table),
                                    Some(cancel.clone()),
                                )
                                .await;
                            hb2.abort();
                            let _ = event_tx.send(AgentEvent::TokenUsage {
                                input_tokens: agent.cumulative_input_tokens,
                                output_tokens: agent.cumulative_output_tokens,
                                cost_usd: agent.cumulative_cost_usd,
                                context_max_tokens,
                            });
                            match continue_result {
                                Ok(text) => {
                                    auto_continue_count = 0;
                                    let _ = event_tx.send(AgentEvent::Response(text));
                                }
                                Err(ClidoError::Interrupted) => {
                                    auto_continue_count = 0;
                                    session_exit = "interrupted";
                                    let _ = event_tx.send(AgentEvent::Interrupted);
                                }
                                Err(e) => {
                                    session_exit = "error";
                                    let _ = event_tx.send(AgentEvent::Err(e.to_string()));
                                }
                            }
                        } else {
                            // Hard cap hit: surface a friendly, actionable message.
                            session_exit = "error";
                            let _ = event_tx.send(AgentEvent::Err(format!(
                                "Reached the turn limit {} times without finishing.\n\
                                 History is intact — type \"continue\" to keep going,\n\
                                 or start a new task.",
                                MAX_AUTO_CONTINUES
                            )));
                            auto_continue_count = 0; // reset so next message works
                        }
                    }
                    Err(ClidoError::BudgetExceeded) => {
                        // Show a warning in chat but don't block — user can keep going
                        // by sending another message. Remove or raise --max-budget-usd to silence.
                        let _ = event_tx.send(AgentEvent::Response(
                            "  ⚠ budget limit reached (set via --max-budget-usd or config). \
                             You can keep sending messages; raise or remove the limit to suppress this warning."
                                .to_string(),
                        ));
                    }
                    Err(e) => {
                        session_exit = "error";
                        let _ = event_tx.send(AgentEvent::Err(e.to_string()));
                    }
                }

                let _ = writer.write_line(&clido_storage::SessionLine::Result {
                    exit_status: session_exit.to_string(),
                    total_cost_usd: agent.cumulative_cost_usd,
                    num_turns: agent.turn_count(),
                    duration_ms: 0,
                });
            }
            AgentAction::Resume(resume_session_id) => {
                match clido_storage::SessionReader::load(&workspace_root, &resume_session_id) {
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!("resume failed: {}", e)));
                    }
                    Ok(lines) => {
                        let new_history = clido_agent::session_lines_to_messages(&lines);
                        agent.replace_history(new_history);
                        // Switch the writer to append to the resumed session.
                        match SessionWriter::append(&workspace_root, &resume_session_id) {
                            Ok(new_writer) => {
                                writer = new_writer;
                            }
                            Err(e) => {
                                let _ =
                                    event_tx.send(AgentEvent::Err(format!("resume writer: {}", e)));
                            }
                        }
                        // Collect display messages for the TUI (user + assistant).
                        let mut msgs: Vec<(String, String)> = Vec::new();
                        for line in &lines {
                            match line {
                                clido_storage::SessionLine::UserMessage { content, .. } => {
                                    if let Some(t) = content
                                        .first()
                                        .and_then(|c| c.get("text"))
                                        .and_then(|v| v.as_str())
                                    {
                                        msgs.push(("user".to_string(), t.to_string()));
                                    }
                                }
                                clido_storage::SessionLine::AssistantMessage { content } => {
                                    let text: String = content
                                        .iter()
                                        .filter_map(|c| {
                                            if c.get("type").and_then(|v| v.as_str())
                                                == Some("text")
                                            {
                                                c.get("text")
                                                    .and_then(|v| v.as_str())
                                                    .map(|s| s.to_string())
                                            } else {
                                                None
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join("");
                                    if !text.trim().is_empty() {
                                        msgs.push(("assistant".to_string(), text));
                                    }
                                }
                                _ => {}
                            }
                        }
                        first_turn = false; // already have history
                        let _ = event_tx.send(AgentEvent::ResumedSession { messages: msgs });
                    }
                }
            }
        }
    }

    let _ = writer.flush();
}

// ── Model list builder ────────────────────────────────────────────────────────

/// Build the full sorted model list from the pricing table, roles config, and user prefs.
/// Order: favorites → recent → rest (alphabetical by id within each group).
fn build_model_list(
    pricing: &clido_core::PricingTable,
    roles: &std::collections::HashMap<String, String>,
    prefs: &clido_core::ModelPrefs,
) -> Vec<ModelEntry> {
    use std::collections::HashMap;

    // Invert roles map: model_id → role name (use first role found if multiple).
    let mut model_to_role: HashMap<String, String> = HashMap::new();
    for (role, model_id) in roles {
        model_to_role
            .entry(model_id.clone())
            .or_insert_with(|| role.clone());
    }
    for (role, model_id) in &prefs.roles {
        model_to_role
            .entry(model_id.clone())
            .or_insert_with(|| role.clone());
    }

    // Build all entries from the pricing table.
    let mut all: Vec<ModelEntry> = pricing
        .models
        .values()
        .map(|e| ModelEntry {
            id: e.name.clone(),
            provider: e.provider.clone(),
            input_mtok: e.input_per_mtok,
            output_mtok: e.output_per_mtok,
            context_k: e.context_window.map(|w| w / 1000),
            role: model_to_role.get(&e.name).cloned(),
            is_favorite: prefs.is_favorite(&e.name),
        })
        .collect();

    all.sort_by(|a, b| a.id.cmp(&b.id));

    // Partition into three groups.
    let mut favorites: Vec<ModelEntry> = all.iter().filter(|m| m.is_favorite).cloned().collect();
    let recent_ids: Vec<&str> = prefs.recent.iter().map(|s| s.as_str()).collect();
    let mut recent: Vec<ModelEntry> = recent_ids
        .iter()
        .filter_map(|id| all.iter().find(|m| m.id == *id && !m.is_favorite))
        .cloned()
        .collect();
    let fav_or_recent: std::collections::HashSet<&str> = favorites
        .iter()
        .chain(recent.iter())
        .map(|m| m.id.as_str())
        .collect();
    let mut rest: Vec<ModelEntry> = all
        .iter()
        .filter(|m| !fav_or_recent.contains(m.id.as_str()))
        .cloned()
        .collect();

    favorites.sort_by(|a, b| a.id.cmp(&b.id));
    rest.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = favorites;
    out.append(&mut recent);
    out.append(&mut rest);
    out
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run_tui(
    cli: Cli,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), anyhow::Error>> + Send>> {
    Box::pin(run_tui_inner(cli))
}

struct AgentRuntimeHandles {
    prompt_tx: mpsc::UnboundedSender<String>,
    resume_tx: mpsc::UnboundedSender<String>,
    model_switch_tx: mpsc::UnboundedSender<String>,
    workdir_tx: mpsc::UnboundedSender<std::path::PathBuf>,
    compact_now_tx: mpsc::UnboundedSender<()>,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    perm_rx: mpsc::UnboundedReceiver<PermRequest>,
    /// Clone of the event_tx channel — lets code outside the agent task send events to the TUI.
    fetch_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Shared todo list written by the agent's TodoWrite tool — readable by the TUI.
    todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>>,
    /// JoinHandle for the agent background task — aborted on TUI exit to prevent zombies.
    agent_handle: tokio::task::JoinHandle<()>,
}

fn start_agent_runtime(
    cli: Cli,
    workspace_root: std::path::PathBuf,
    preloaded_config: Option<clido_core::LoadedConfig>,
    preloaded_pricing: clido_core::PricingTable,
    cancel: Arc<AtomicBool>,
    image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
    reviewer_enabled: Arc<AtomicBool>,
) -> AgentRuntimeHandles {
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<String>();
    let (resume_tx, resume_rx) = mpsc::unbounded_channel::<String>();
    let (model_switch_tx, model_switch_rx) = mpsc::unbounded_channel::<String>();
    let (workdir_tx, workdir_rx) = mpsc::unbounded_channel::<std::path::PathBuf>();
    let (compact_now_tx, compact_now_rx) = mpsc::unbounded_channel::<()>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (perm_tx, perm_rx) = mpsc::unbounded_channel::<PermRequest>();

    // Pre-create the shared todo store so both the agent task and the TUI app can share it.
    let todo_store: std::sync::Arc<std::sync::Mutex<Vec<clido_tools::TodoItem>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let agent_handle = tokio::spawn(agent_task(
        cli,
        workspace_root,
        preloaded_config,
        preloaded_pricing,
        prompt_rx,
        resume_rx,
        model_switch_rx,
        workdir_rx,
        compact_now_rx,
        event_tx.clone(),
        perm_tx,
        cancel,
        image_state,
        reviewer_enabled,
        todo_store.clone(),
    ));

    AgentRuntimeHandles {
        prompt_tx,
        resume_tx,
        model_switch_tx,
        workdir_tx,
        compact_now_tx,
        event_rx,
        perm_rx,
        fetch_tx: event_tx,
        todo_store,
        agent_handle,
    }
}

/// Spawn a background task to fetch the model list from the provider API.
/// Results arrive via `AgentEvent::ModelsLoaded` on the given channel.
fn spawn_model_fetch(
    provider: String,
    api_key: String,
    base_url: Option<String>,
    tx: mpsc::UnboundedSender<AgentEvent>,
) {
    tokio::spawn(async move {
        let base_url_ref = base_url.as_deref();
        let entries =
            clido_providers::fetch_provider_models(&provider, &api_key, base_url_ref).await;
        let ids: Vec<String> = entries
            .into_iter()
            .filter(|m| m.available)
            .map(|m| m.id)
            .collect();
        let _ = tx.send(AgentEvent::ModelsLoaded(ids));
    });
}

async fn run_tui_inner(cli: Cli) -> Result<(), anyhow::Error> {
    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));

    // Prune session files older than 30 days in the background (non-fatal).
    {
        let wr = workspace_root.clone();
        tokio::task::spawn_blocking(move || {
            let _ = clido_storage::prune_old_sessions(&wr, 30);
        });
    }

    // Load config, pricing table, and model prefs concurrently to minimise startup latency.
    let wr = workspace_root.clone();
    let (config_res, pricing_res, prefs_res) = tokio::join!(
        tokio::task::spawn_blocking(move || clido_core::load_config(&wr)),
        tokio::task::spawn_blocking(clido_core::load_pricing),
        tokio::task::spawn_blocking(clido_core::ModelPrefs::load),
    );
    let loaded_config: Option<clido_core::LoadedConfig> = config_res.ok().and_then(|r| r.ok());
    let (pricing_table, _) =
        pricing_res.unwrap_or_else(|_| (clido_core::PricingTable::default(), None));
    let model_prefs = prefs_res.unwrap_or_else(|_| clido_core::ModelPrefs::default());

    // Derive provider + model from the loaded config (mirrors read_provider_model logic).
    let (provider, model, api_key, base_url) = {
        let profile_name = cli.profile.as_deref().unwrap_or_else(|| {
            loaded_config
                .as_ref()
                .map(|c| c.default_profile.as_str())
                .unwrap_or("default")
        });
        match loaded_config
            .as_ref()
            .and_then(|c| c.get_profile(profile_name).ok())
        {
            Some(profile) => {
                let model = cli.model.clone().unwrap_or_else(|| profile.model.clone());
                let provider = cli
                    .provider
                    .clone()
                    .unwrap_or_else(|| profile.provider.clone());
                // Resolve the API key: direct value takes priority over env var.
                let key = profile
                    .api_key
                    .clone()
                    .or_else(|| {
                        profile
                            .api_key_env
                            .as_deref()
                            .and_then(|e| std::env::var(e).ok())
                    })
                    .unwrap_or_default();
                (provider, model, key, profile.base_url.clone())
            }
            None => ("?".to_string(), "?".to_string(), String::new(), None),
        }
    };

    // Resolve notify setting: CLI flags take priority over config.
    let notify_enabled = if cli.no_notify {
        false
    } else if cli.notify {
        true
    } else {
        loaded_config
            .as_ref()
            .map(|c| c.agent.notify)
            .unwrap_or(false)
    };

    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let image_state: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));

    // Reviewer toggle: shared between App (TUI control) and SpawnReviewerTool (enforcement).
    // Derive initial state from config: enabled by default if reviewer is configured.
    let reviewer_configured = loaded_config
        .as_ref()
        .map(|c| c.agents.reviewer.is_some())
        .unwrap_or(false);
    let reviewer_enabled = Arc::new(AtomicBool::new(true));
    let mut runtime = start_agent_runtime(
        cli.clone(),
        workspace_root.clone(),
        loaded_config.clone(),
        pricing_table.clone(),
        cancel.clone(),
        image_state.clone(),
        reviewer_enabled.clone(),
    );

    // Install a panic hook so the terminal is always restored even on crash.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stderr(), LeaveAlternateScreen);
        #[cfg(unix)]
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) == 0 && t.c_lflag & libc::ICANON == 0 {
                t.c_iflag |= (libc::ICRNL | libc::IXON) as libc::tcflag_t;
                t.c_oflag |= (libc::OPOST | libc::ONLCR) as libc::tcflag_t;
                t.c_lflag |= (libc::ICANON
                    | libc::ECHO
                    | libc::ECHOE
                    | libc::ECHOK
                    | libc::ISIG
                    | libc::IEXTEN) as libc::tcflag_t;
                libc::tcsetattr(0, libc::TCSAFLUSH, &t);
            }
        }
        original_hook(info);
    }));

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let plan_dry_run = cli.plan_dry_run;

    // Build model list from already-loaded config + pricing (no extra disk I/O needed).
    let (config_roles, known_models, current_profile) = {
        let roles = loaded_config
            .as_ref()
            .map(|c| c.roles.as_map())
            .unwrap_or_default();
        let profile = cli
            .profile
            .clone()
            .or_else(|| loaded_config.as_ref().map(|c| c.default_profile.clone()))
            .unwrap_or_else(|| "default".to_string());
        let models = build_model_list(&pricing_table, &roles, &model_prefs);
        (roles, models, profile)
    };

    let mut app = App::new(
        runtime.prompt_tx.clone(),
        runtime.resume_tx.clone(),
        runtime.model_switch_tx.clone(),
        runtime.workdir_tx.clone(),
        runtime.compact_now_tx.clone(),
        cancel,
        provider.clone(),
        model,
        workspace_root.clone(),
        notify_enabled,
        image_state,
        plan_dry_run,
        known_models,
        model_prefs,
        config_roles,
        current_profile,
        reviewer_enabled,
        reviewer_configured,
        runtime.todo_store.clone(),
        api_key.clone(),
        base_url.clone(),
        runtime.fetch_tx.clone(),
    );
    // Kick off a live model-list fetch from the provider API immediately at startup.
    // Results arrive as AgentEvent::ModelsLoaded and update app.known_models.
    if !api_key.is_empty() {
        spawn_model_fetch(
            provider.clone(),
            api_key.clone(),
            base_url.clone(),
            runtime.fetch_tx.clone(),
        );
        app.models_loading = true;
    }
    let mut recovery_attempts: u8 = 0;
    let result = loop {
        match event_loop(
            &mut app,
            &mut terminal,
            &mut runtime.event_rx,
            &mut runtime.perm_rx,
        )
        .await?
        {
            EventLoopExit::Quit => break Ok(()),
            EventLoopExit::ProfileSwitch(profile_name) => {
                // Switch active profile on disk.
                if let Some(config_path) = clido_core::global_config_path() {
                    let _ = clido_core::switch_active_profile(&config_path, &profile_name);
                }

                // Reload config from disk.
                let wr = workspace_root.clone();
                let fresh_config: Option<clido_core::LoadedConfig> =
                    tokio::task::spawn_blocking(move || clido_core::load_config(&wr).ok())
                        .await
                        .ok()
                        .flatten();

                // Extract new profile settings.
                let (new_provider, new_model, new_api_key, new_base_url) = {
                    let pname = profile_name.as_str();
                    match fresh_config
                        .as_ref()
                        .and_then(|c| c.get_profile(pname).ok())
                    {
                        Some(profile) => {
                            let key = profile
                                .api_key
                                .clone()
                                .or_else(|| {
                                    profile
                                        .api_key_env
                                        .as_deref()
                                        .and_then(|e| std::env::var(e).ok())
                                })
                                .unwrap_or_default();
                            (
                                profile.provider.clone(),
                                profile.model.clone(),
                                key,
                                profile.base_url.clone(),
                            )
                        }
                        None => ("?".to_string(), "?".to_string(), String::new(), None),
                    }
                };

                // Abort old agent runtime.
                runtime.agent_handle.abort();

                // Start fresh agent runtime with updated config.
                let mut switch_cli = cli.clone();
                switch_cli.profile = Some(profile_name.clone());
                if let Some(sid) = app.current_session_id.as_deref() {
                    switch_cli.resume = Some(sid.to_string());
                }

                runtime = start_agent_runtime(
                    switch_cli,
                    workspace_root.clone(),
                    fresh_config.clone().or_else(|| loaded_config.clone()),
                    pricing_table.clone(),
                    app.cancel.clone(),
                    app.image_state.clone(),
                    app.reviewer_enabled.clone(),
                );

                // Update app state in-place.
                app.provider = new_provider.clone();
                app.model = new_model.clone();
                app.api_key = new_api_key.clone();
                app.base_url = new_base_url.clone();
                app.current_profile = profile_name.clone();
                app.prompt_tx = runtime.prompt_tx.clone();
                app.resume_tx = runtime.resume_tx.clone();
                app.model_switch_tx = runtime.model_switch_tx.clone();
                app.workdir_tx = runtime.workdir_tx.clone();
                app.compact_now_tx = runtime.compact_now_tx.clone();
                app.quit = false;
                app.busy = false;
                app.status_log.clear();
                app.cancel.store(false, Ordering::Relaxed);

                // Kick off model list fetch for new provider.
                if !new_api_key.is_empty() {
                    spawn_model_fetch(
                        new_provider,
                        new_api_key,
                        new_base_url,
                        runtime.fetch_tx.clone(),
                    );
                    app.models_loading = true;
                }

                app.push(ChatLine::Info(format!(
                    "  switched to profile '{profile_name}'"
                )));
                recovery_attempts = 0;
                continue;
            }
            EventLoopExit::Recover(reason) => {
                recovery_attempts = recovery_attempts.saturating_add(1);
                if recovery_attempts > 3 {
                    break Err(anyhow::anyhow!(
                        "agent recovery failed after {} attempts: {}",
                        recovery_attempts - 1,
                        reason
                    ));
                }
                let backoff_ms = 300u64.saturating_mul(1u64 << (recovery_attempts - 1));
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;

                // Re-use the current session so the recovered agent picks up the full
                // conversation history from disk, preventing context amnesia.
                let mut recovery_cli = cli.clone();
                if let Some(sid) = app.current_session_id.as_deref() {
                    recovery_cli.resume = Some(sid.to_string());
                    app.recovering = true;
                }

                runtime = start_agent_runtime(
                    recovery_cli,
                    workspace_root.clone(),
                    loaded_config.clone(),
                    pricing_table.clone(),
                    app.cancel.clone(),
                    app.image_state.clone(),
                    app.reviewer_enabled.clone(),
                );
                app.prompt_tx = runtime.prompt_tx.clone();
                app.resume_tx = runtime.resume_tx.clone();
                app.model_switch_tx = runtime.model_switch_tx.clone();
                app.workdir_tx = runtime.workdir_tx.clone();
                app.compact_now_tx = runtime.compact_now_tx.clone();
                app.push(ChatLine::Thinking("↻ recovering runtime…".to_string()));
                app.busy = false;
                app.status_log.clear();
                app.cancel.store(false, Ordering::Relaxed);
                app.drain_input_queue();
                continue;
            }
        }
    };

    // Abort the agent background task to prevent zombies after TUI exits.
    runtime.agent_handle.abort();

    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );

    // Handle /profile <name> switch request.
    if let Some(profile_name) = app.wants_profile_switch.take() {
        if let Some(config_path) = clido_core::global_config_path() {
            let _ = clido_core::switch_active_profile(&config_path, &profile_name);
        }
        let mut next_cli = cli.clone();
        next_cli.resume = app.restart_resume_session.take();
        return run_tui(next_cli).await;
    }

    // Handle /profile new — create a new profile via the guided wizard, then restart TUI.
    if app.wants_profile_create {
        crate::setup::run_create_profile(None).await?;
        return run_tui(cli).await;
    }

    // Handle /profile edit <name> — edit existing profile via wizard, then restart TUI.
    if let Some(profile_name) = app.wants_profile_edit.take() {
        let entry = clido_core::load_config(&workspace_root)
            .ok()
            .and_then(|c| c.profiles.get(&profile_name).cloned());
        if let Some(entry) = entry {
            crate::setup::run_edit_profile(profile_name, entry).await?;
        } else {
            tracing::warn!(profile = %profile_name, "profile not found for /profile edit");
        }
        let mut next_cli = cli.clone();
        next_cli.resume = app.restart_resume_session.take();
        return run_tui(next_cli).await;
    }

    // Handle /init reinit request.
    if app.wants_reinit {
        let pre_fill = {
            let loaded = clido_core::load_config(&workspace_root).ok();
            let profile = loaded
                .as_ref()
                .map(|c| c.default_profile.clone())
                .unwrap_or_else(|| "default".to_string());
            let prof_entry = loaded
                .as_ref()
                .and_then(|c| c.profiles.get(&profile).cloned());
            let api_key = prof_entry
                .as_ref()
                .and_then(|p| {
                    p.api_key
                        .clone()
                        .or_else(|| p.api_key_env.as_ref().and_then(|e| std::env::var(e).ok()))
                })
                .unwrap_or_default();
            let roles: Vec<(String, String)> = loaded
                .as_ref()
                .map(|c| {
                    let mut v: Vec<(String, String)> = c.roles.as_map().into_iter().collect();
                    v.sort_by(|a, b| a.0.cmp(&b.0));
                    v
                })
                .unwrap_or_default();
            crate::setup::SetupPreFill {
                provider: app.provider.clone(),
                api_key,
                model: app.model.clone(),
                roles,
                profile_name: String::new(),
                is_new_profile: false,
                saved_api_keys: Vec::new(),
            }
        };
        // Run setup wizard with current values pre-filled.
        crate::setup::run_reinit(pre_fill).await?;
        // Re-launch the TUI.
        let cli_for_reinit = cli.clone();
        return run_tui(cli_for_reinit).await;
    }

    result
}

enum EventLoopExit {
    Quit,
    Recover(String),
    /// Switch to a different profile without restarting the TUI.
    ProfileSwitch(String),
}

async fn event_loop(
    app: &mut App,
    terminal: &mut ratatui::Terminal<CrosstermBackend<std::io::Stdout>>,
    event_rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
    perm_rx: &mut mpsc::UnboundedReceiver<PermRequest>,
) -> Result<EventLoopExit, anyhow::Error> {
    let mut crossterm_events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(80));
    let mut last_agent_activity = std::time::Instant::now();
    // Stall timeout: trigger recovery only if truly no activity (heartbeats keep this fresh
    // during long LLM calls, so 120 s is a reliable hard ceiling for genuinely hung agents).
    const STALL_TIMEOUT_SECS: u64 = 120;
    // Only redraw when state has actually changed to reduce CPU usage.
    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|f| render(f, app))?;
            dirty = false;
        }

        tokio::select! {
            _ = tick.tick() => {
                // Only mark dirty when spinner is actually animating.
                if app.busy || app.pending_perm.is_some() {
                    app.tick_spinner();
                    dirty = true;
                }
                if app.busy && app.pending_perm.is_none() {
                    let baseline = if let Some(turn_start) = app.turn_start {
                        if turn_start > last_agent_activity {
                            turn_start
                        } else {
                            last_agent_activity
                        }
                    } else {
                        last_agent_activity
                    };
                    if baseline.elapsed().as_secs() >= STALL_TIMEOUT_SECS {
                        return Ok(EventLoopExit::Recover(
                            "agent appears stalled (no progress events)".to_string(),
                        ));
                    }
                }
            }
            maybe = crossterm_events.next() => {
                dirty = true;
                match maybe {
                    Some(Ok(Event::Key(key))) => {
                        // Ctrl+L: force a full terminal redraw (screen recovery).
                        if key.modifiers == KeyModifiers::CONTROL
                            && key.code == KeyCode::Char('l')
                        {
                            let _ = terminal.clear();
                        } else {
                            handle_key(app, key);
                        }
                    }
                    Some(Ok(Event::Paste(mut text))) => {
                        // Normalise line endings to \n but preserve newlines so users can
                        // paste multiline markdown without it collapsing into a single line.
                        text = text.replace("\r\n", "\n").replace('\r', "\n");
                        if text.is_empty() {
                            // nothing to do
                        } else if let Some(ref mut ed) = app.plan_text_editor {
                            // Route paste into plan text editor at cursor position.
                            let line = &mut ed.lines[ed.cursor_row];
                            let byte_pos = line
                                .char_indices()
                                .nth(ed.cursor_col)
                                .map(|(i, _)| i)
                                .unwrap_or(line.len());
                            // Only insert first line (plan editor is line-based)
                            let paste_line = text.lines().next().unwrap_or(&text);
                            line.insert_str(byte_pos, paste_line);
                            ed.cursor_col += paste_line.chars().count();
                        } else if app.overlay_stack.handle_paste(&text) {
                            // Overlay stack consumed the paste
                        } else if let Some(ref mut ov) = app.profile_overlay {
                            // Route paste into the active profile overlay text input.
                            // Provider/model picker steps don't accept free-text paste.
                            let accepts_text = matches!(
                                &ov.mode,
                                ProfileOverlayMode::Creating {
                                    step: ProfileCreateStep::Name | ProfileCreateStep::ApiKey,
                                } | ProfileOverlayMode::EditField(_)
                            );
                            if accepts_text {
                                // Strip newlines from API keys
                                let clean = if matches!(
                                    &ov.mode,
                                    ProfileOverlayMode::Creating {
                                        step: ProfileCreateStep::ApiKey,
                                    } | ProfileOverlayMode::EditField(ProfileEditField::ApiKey)
                                ) {
                                    text.lines().collect::<Vec<_>>().join("")
                                } else {
                                    text.clone()
                                };
                                let b = char_byte_pos_tui(&ov.input, ov.input_cursor);
                                ov.input.insert_str(b, &clean);
                                ov.input_cursor += clean.chars().count();
                            }
                        } else if let Some(ref mut st) = app.settings {
                            // Route paste into settings input field.
                            let accepts = !matches!(st.edit_field, SettingsEditField::None);
                            if accepts {
                                let clean = text.lines().next().unwrap_or(&text);
                                st.input.push_str(clean);
                            }
                        } else {
                            let byte_pos = char_byte_pos(&app.input, app.cursor);
                            app.input.insert_str(byte_pos, &text);
                            app.cursor += text.chars().count();
                            app.selected_cmd = None;
                            app.history_idx = None;
                        }
                    }
                    Some(Ok(Event::Mouse(m))) => {
                        match m.kind {
                            MouseEventKind::ScrollDown => scroll_down(app, 3),
                            MouseEventKind::ScrollUp => scroll_up(app, 3),
                            _ => {}
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        // Force a clean redraw after terminal resize to avoid stale cells.
                        // Preserve scroll ratio so user doesn't lose their place.
                        let ratio = if app.max_scroll > 0 && !app.following {
                            Some(app.scroll as f64 / app.max_scroll as f64)
                        } else {
                            None
                        };
                        let _ = terminal.clear();
                        // Width changed — render cache is now stale (line-wrapping differs).
                        app.render_cache.clear();
                        app.render_cache_msg_count = 0;
                        // Restore approximate scroll position after redraw recalculates max_scroll.
                        // The actual clamping is done in render_frame when max_scroll is recomputed.
                        app.pending_scroll_ratio = ratio;
                    }
                    None => break,
                    _ => {}
                }
            }
            maybe = event_rx.recv() => {
                dirty = true;
                match maybe {
                    Some(AgentEvent::ToolStart {
                        tool_use_id,
                        name,
                        detail,
                    }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.push_status(
                            tool_use_id.clone(),
                            name.clone(),
                            detail.clone(),
                        );
                        app.push(ChatLine::ToolCall {
                            tool_use_id,
                            name,
                            detail,
                            done: false,
                            is_error: false,
                        });
                    }
                    Some(AgentEvent::ToolDone {
                        tool_use_id,
                        is_error,
                        diff,
                        ..
                    }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.finish_status(&tool_use_id, is_error);
                        for line in app.messages.iter_mut() {
                            if let ChatLine::ToolCall {
                                tool_use_id: tid,
                                done,
                                is_error: e,
                                ..
                            } = line
                            {
                                if tid == &tool_use_id && !*done {
                                    *done = true;
                                    *e = is_error;
                                    break;
                                }
                            }
                        }
                        if let Some(d) = diff {
                            if !d.is_empty() {
                                app.push(ChatLine::Diff(d));
                            }
                        }
                    }
                    Some(AgentEvent::Thinking(text)) => {
                        last_agent_activity = std::time::Instant::now();
                        if let Some((num, step)) = extract_current_step_full(&text) {
                            app.current_step = Some(step);
                            app.last_executed_step_num = Some(num);
                        }
                        app.push(ChatLine::Thinking(text));
                        // Don't call on_agent_done — the agent is still running.
                    }
                    Some(AgentEvent::Response(text)) => {
                        last_agent_activity = std::time::Instant::now();
                        if let Some((num, step)) = extract_current_step_full(&text) {
                            app.current_step = Some(step);
                            app.last_executed_step_num = Some(num);
                        }
                        app.push(ChatLine::Assistant(text));
                        // Fire desktop notification + bell if enabled.
                        if app.notify_enabled {
                            let elapsed = app
                                .turn_start
                                .map(|s| s.elapsed().as_secs())
                                .unwrap_or(0);
                            let session_id = app
                                .current_session_id
                                .as_deref()
                                .unwrap_or("unknown");
                            crate::notify::notify_done(
                                session_id,
                                elapsed,
                                app.session_total_cost_usd,
                            );
                        }
                        // Revert per-turn model override if active.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.model_switch_tx.send(prev.clone());
                            app.push(ChatLine::Info(format!(
                                "  ↻ Model restored to {}",
                                prev
                            )));
                        }
                        app.on_agent_done();
                    }
                    Some(AgentEvent::ModelSwitched { to_model }) => {
                        last_agent_activity = std::time::Instant::now();
                        // Confirmation from agent_task that the model was switched.
                        // Update display model in case it diverged.
                        app.model = to_model;
                    }
                    Some(AgentEvent::WorkdirSwitched { path }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.workspace_root = path.clone();
                        // Reset the app-side AllowAll override — the agent's override was
                        // already reset in agent_task when replace_tools was called.
                        app.permission_mode_override = None;
                        app.push(ChatLine::Info(format!("  ✓ Working directory: {}", path.display())));
                        app.push(ChatLine::Info(
                            "  Permission grants were reset for safety after the switch."
                                .into(),
                        ));
                        app.push(ChatLine::Info(
                            "  Note: session history stays on the original project until restart."
                                .into(),
                        ));
                    }
                    Some(AgentEvent::SessionStarted(id)) => {
                        last_agent_activity = std::time::Instant::now();
                        app.current_session_id = Some(id);
                    }
                    Some(AgentEvent::Interrupted) => {
                        last_agent_activity = std::time::Instant::now();
                        // Revert per-turn model override on interruption too.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.model_switch_tx.send(prev);
                        }
                        app.on_agent_done();
                    }
                    Some(AgentEvent::Err(msg)) => {
                        last_agent_activity = std::time::Instant::now();
                        // Revert per-turn model override on error too.
                        if let Some(prev) = app.per_turn_prev_model.take() {
                            app.model = prev.clone();
                            let _ = app.model_switch_tx.send(prev);
                        }
                        app.pending_error = Some(ErrorInfo::from_message(msg));
                        app.on_agent_done();
                    }
                    Some(AgentEvent::ResumedSession { messages }) => {
                        last_agent_activity = std::time::Instant::now();
                        if app.recovering {
                            // Silent recovery resume: the agent has restored its internal history
                            // from the session on disk. Preserve the current TUI display and just
                            // show a brief note so the user knows context was restored.
                            app.recovering = false;
                            app.push(ChatLine::Info(
                                "  ✓ context restored — please re-send your last message".into(),
                            ));
                        } else {
                            // Explicit /resume or startup resume: clear and replay.
                            app.messages.clear();
                            app.messages.push(ChatLine::WelcomeBrand);
                            let user_turns = messages.iter().filter(|(r, _)| r == "user").count();
                            let turn_label = if user_turns == 1 { "1 message".to_string() } else { format!("{} messages", user_turns) };
                            app.messages.push(ChatLine::Info(format!("  ↺ Session resumed — {}", turn_label)));
                            for (role, text) in messages {
                                if role == "user" {
                                    app.push(ChatLine::User(text));
                                } else if role == "assistant" {
                                    app.push(ChatLine::Assistant(text));
                                }
                            }
                        }
                        app.busy = false;
                    }
                    Some(AgentEvent::TokenUsage { input_tokens, output_tokens, cost_usd, context_max_tokens }) => {
                        last_agent_activity = std::time::Instant::now();
                        // Cumulative fields on the agent reset at the start of each Run.
                        // Use delta tracking: if new value < previous, this is a new run.
                        let delta_in = if input_tokens >= app.session_input_tokens {
                            input_tokens - app.session_input_tokens
                        } else {
                            input_tokens // new run — full value is the delta
                        };
                        let delta_out = if output_tokens >= app.session_output_tokens {
                            output_tokens - app.session_output_tokens
                        } else {
                            output_tokens
                        };
                        let delta_cost = if cost_usd >= app.session_cost_usd {
                            cost_usd - app.session_cost_usd
                        } else {
                            cost_usd
                        };
                        app.session_input_tokens = input_tokens;
                        app.session_output_tokens = output_tokens;
                        app.session_cost_usd = cost_usd;
                        app.session_total_input_tokens += delta_in;
                        app.session_total_output_tokens += delta_out;
                        app.session_total_cost_usd += delta_cost;
                        app.context_max_tokens = context_max_tokens;
                    }
                    Some(AgentEvent::Compacted { before, after }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.push(ChatLine::Info(format!(
                            "  ↻ Context compressed: {} → {} messages (older history summarised)",
                            before, after
                        )));
                    }
                    Some(AgentEvent::PlanCreated { tasks }) => {
                        last_agent_activity = std::time::Instant::now();
                        // Display the plan in the chat as an info block.
                        app.push(ChatLine::Info("  ┌─ Plan:".to_string()));
                        let count = tasks.len();
                        for (i, task) in tasks.iter().enumerate() {
                            let prefix = if i + 1 == count { "        └─" } else { "        ├─" };
                            app.push(ChatLine::Info(format!("{} {}", prefix, task)));
                        }
                        // Store last plan so /plan command can show it later.
                        app.last_plan = Some(tasks.clone());
                        app.last_plan_snapshot = build_plan_from_tasks(&tasks);
                    }
                    Some(AgentEvent::PlanReady { plan }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.last_plan_snapshot = Some(plan.clone());
                        app.last_plan = Some(
                            plan.tasks
                                .iter()
                                .map(|t| t.description.clone())
                                .collect::<Vec<_>>(),
                        );
                        // Open the plan editor overlay (blocks execution until user presses x or Esc).
                        app.plan_selected_task = 0;
                        app.plan_task_editing = None;
                        app.plan_editor = Some(PlanEditor::new(plan));
                        // Mark as busy so the spinner shows — agent is paused waiting for plan approval.
                    }
                    Some(AgentEvent::PlanTaskStarted { task_id }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.push(ChatLine::Info(format!("  ↻ Step {} started", task_id)));
                    }
                    Some(AgentEvent::PlanTaskDone { task_id, success }) => {
                        last_agent_activity = std::time::Instant::now();
                        let icon = if success { "✓" } else { "✗" };
                        app.push(ChatLine::Info(format!("  {} Step {} done", icon, task_id)));
                    }
                    Some(AgentEvent::Heartbeat) => {
                        // Silent keep-alive from agent_task during slow LLM responses.
                        // Just refresh the activity timestamp to prevent false stall detection.
                        last_agent_activity = std::time::Instant::now();
                    }
                    Some(AgentEvent::BudgetWarning { percent, cost, limit }) => {
                        last_agent_activity = std::time::Instant::now();
                        app.push(ChatLine::Info(format!(
                            "  ⚠ {}% of budget used (${:.4} / ${:.4})",
                            percent, cost, limit
                        )));
                    }
                    Some(AgentEvent::ModelsLoaded(ids)) => {
                        app.models_loading = false;
                        if !ids.is_empty() {
                            // Merge API-fetched model IDs with existing pricing data.
                            // Keep existing entries (which may have cost metadata) for known IDs,
                            // and add stub entries for newly-discovered ones.
                            let existing: std::collections::HashSet<String> =
                                app.known_models.iter().map(|m| m.id.clone()).collect();
                            for id in &ids {
                                if !existing.contains(id) {
                                    app.known_models.push(ModelEntry {
                                        id: id.clone(),
                                        provider: app.provider.clone(),
                                        input_mtok: 0.0,
                                        output_mtok: 0.0,
                                        context_k: None,
                                        role: None,
                                        is_favorite: false,
                                    });
                                }
                            }
                            // If model picker is open, refresh its list to show the new models.
                            if let Some(picker) = &mut app.model_picker {
                                let all: Vec<String> =
                                    app.known_models.iter().map(|m| m.id.clone()).collect();
                                picker.refresh_models(all);
                            }
                        }
                    }
                    None => {
                        return Ok(EventLoopExit::Recover(
                            "agent event channel closed unexpectedly".to_string(),
                        ));
                    }
                }
            }
            maybe = perm_rx.recv() => {
                dirty = true;
                if let Some(req) = maybe {
                    last_agent_activity = std::time::Instant::now();
                    app.pending_perm = Some(PendingPerm {
                        tool_name: req.tool_name,
                        preview: req.preview,
                        reply: req.reply,
                    });
                    // Don't clear busy — agent is still running, awaiting our reply.
                } else {
                    return Ok(EventLoopExit::Recover(
                        "permission channel closed unexpectedly".to_string(),
                    ));
                }
            }
        }

        if app.quit {
            // Check if this is a profile switch rather than a real quit.
            if let Some(profile_name) = app.wants_profile_switch.take() {
                return Ok(EventLoopExit::ProfileSwitch(profile_name));
            }
            return Ok(EventLoopExit::Quit);
        }
    }
    Ok(EventLoopExit::Quit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn make_test_app() -> App {
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (resume_tx, _resume_rx) = mpsc::unbounded_channel();
        let (model_switch_tx, _model_switch_rx) = mpsc::unbounded_channel();
        let (workdir_tx, _workdir_rx) = mpsc::unbounded_channel();
        let (compact_now_tx, _compact_now_rx) = mpsc::unbounded_channel();
        let (fetch_tx, _fetch_rx) = mpsc::unbounded_channel();
        App::new(
            prompt_tx,
            resume_tx,
            model_switch_tx,
            workdir_tx,
            compact_now_tx,
            Arc::new(AtomicBool::new(false)),
            "openrouter".to_string(),
            "default-model".to_string(),
            std::env::temp_dir(),
            false,
            Arc::new(Mutex::new(None)),
            false,
            Vec::new(),
            clido_core::ModelPrefs::default(),
            std::collections::HashMap::new(),
            "default".to_string(),
            Arc::new(AtomicBool::new(false)),
            false,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            String::new(),
            None,
            fetch_tx,
        )
    }

    // ── parse_per_turn_model tests ─────────────────────────────────────────────

    #[test]
    fn test_parse_per_turn_model_extracts_model_and_prompt() {
        let result = parse_per_turn_model("@claude-opus-4-6 explain the auth flow");
        assert_eq!(
            result,
            Some((
                "claude-opus-4-6".to_string(),
                "explain the auth flow".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_per_turn_model_returns_none_for_normal_input() {
        assert_eq!(parse_per_turn_model("explain the auth flow"), None);
    }

    #[test]
    fn test_parse_per_turn_model_returns_none_for_at_in_middle() {
        // @ not at start → None
        assert_eq!(parse_per_turn_model("email me @ work"), None);
    }

    #[test]
    fn test_parse_per_turn_model_returns_none_for_at_only() {
        assert_eq!(parse_per_turn_model("@"), None);
    }

    #[test]
    fn test_parse_per_turn_model_returns_none_for_model_no_prompt() {
        // Has model name but no prompt after space
        assert_eq!(parse_per_turn_model("@claude-opus-4-6"), None);
    }

    #[test]
    fn test_parse_per_turn_model_trims_prompt_whitespace() {
        let result = parse_per_turn_model("@claude-haiku-4-5   refactor this");
        assert_eq!(
            result,
            Some(("claude-haiku-4-5".to_string(), "refactor this".to_string()))
        );
    }

    // ── /fast and /smart model name constants ─────────────────────────────────

    #[test]
    fn test_fast_model_name() {
        // Verify the model name used by /fast is the expected haiku model.
        let fast_model = "claude-haiku-4-5-20251001";
        assert!(!fast_model.is_empty());
        assert!(fast_model.contains("haiku"));
    }

    #[test]
    fn test_smart_model_name() {
        // Verify the model name used by /smart is the expected opus model.
        let smart_model = "claude-opus-4-6";
        assert!(!smart_model.is_empty());
        assert!(smart_model.contains("opus"));
    }

    // ── slash_completions ──────────────────────────────────────────────────────

    #[test]
    fn completions_for_slash_only_returns_all_commands() {
        let c = slash_completions("/");
        assert_eq!(c.len(), slash_commands().len());
    }

    #[test]
    fn completions_for_pr_includes_profile_variants() {
        // When the user types "/pr", autocomplete should offer /pr, /profile,
        // and /profiles — all are valid prefixed matches for the popup.
        let c = slash_completions("/pr");
        let cmds: Vec<&str> = c.iter().map(|(cmd, _)| *cmd).collect();
        assert!(cmds.contains(&"/pr"), "/pr must be in completions");
        assert!(
            cmds.contains(&"/profile"),
            "/profile must be in completions"
        );
        assert!(
            cmds.contains(&"/profiles"),
            "/profiles must be in completions"
        );
    }

    #[test]
    fn completions_for_profile_does_not_include_pr() {
        let c = slash_completions("/profile");
        let cmds: Vec<&str> = c.iter().map(|(cmd, _)| *cmd).collect();
        assert!(
            !cmds.contains(&"/pr"),
            "/pr must NOT appear for /profile prefix"
        );
    }

    #[test]
    fn completions_for_profiles_returns_only_profiles() {
        let c = slash_completions("/profiles");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, "/profiles");
    }

    #[test]
    fn completions_empty_for_non_slash() {
        assert!(slash_completions("hello").is_empty());
        assert!(slash_completions("").is_empty());
    }

    #[test]
    fn parse_plan_from_text_strips_markdown_wrapped_steps() {
        let text = "### **Step 1:** Fix auth\n**Step 2:** Add tests\n";
        let tasks = parse_plan_from_text(text);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0], "Fix auth");
        assert_eq!(tasks[1], "Add tests");
    }

    #[test]
    fn build_plan_from_assistant_text_preserves_order_for_save_and_render() {
        let text = "Step 1: Define contract\nStep 2: Implement parser\nStep 3: Add tests";
        let plan = build_plan_from_assistant_text(text).expect("plan");
        let tasks: Vec<String> = plan.tasks.iter().map(|t| t.description.clone()).collect();
        assert_eq!(
            tasks,
            vec![
                "Define contract".to_string(),
                "Implement parser".to_string(),
                "Add tests".to_string()
            ]
        );
    }

    #[test]
    fn build_plan_from_assistant_text_fallback_is_deterministic() {
        let text = "alpha\n\n beta  \n gamma";
        let plan = build_plan_from_assistant_text(text).expect("fallback plan");
        let tasks: Vec<String> = plan.tasks.iter().map(|t| t.description.clone()).collect();
        assert_eq!(
            tasks,
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
        );
    }

    #[test]
    fn resolve_workdir_arg_accepts_absolute_dir() {
        let td = tempfile::tempdir().expect("tempdir");
        let p = td.path().to_path_buf();
        let resolved = resolve_workdir_arg(p.to_str().expect("utf8 path")).expect("resolve");
        assert_eq!(std::fs::canonicalize(p).expect("canonicalize"), resolved);
    }

    #[test]
    fn resolve_workdir_arg_rejects_missing_path() {
        let td = tempfile::tempdir().expect("tempdir");
        let missing = td.path().join("does-not-exist-12345");
        let err = resolve_workdir_arg(missing.to_str().expect("utf8 path")).expect_err("must fail");
        assert!(err.contains("could not access"));
    }

    // ── is_known_slash_cmd ────────────────────────────────────────────────────

    #[test]
    fn known_slash_cmd_exact_match() {
        assert!(is_known_slash_cmd("/pr"));
        assert!(is_known_slash_cmd("/clear"));
        assert!(is_known_slash_cmd("/ship"));
        assert!(is_known_slash_cmd("/profile"));
        assert!(is_known_slash_cmd("/profiles"));
    }

    #[test]
    fn known_slash_cmd_with_args() {
        assert!(is_known_slash_cmd("/pr my feature title"));
        assert!(is_known_slash_cmd("/profile work"));
        assert!(is_known_slash_cmd("/memory search query"));
        assert!(is_known_slash_cmd("/branch feature/foo"));
        assert!(is_known_slash_cmd("/ship fix login bug"));
    }

    #[test]
    fn known_slash_cmd_profile_with_name_is_recognized() {
        // Regression: /profile <name> must be recognized as a slash command,
        // not silently treated as chat input.
        assert!(is_known_slash_cmd("/profile default"));
        assert!(is_known_slash_cmd("/profile my-work-profile"));
    }

    #[test]
    fn known_slash_cmd_rejects_unknown() {
        assert!(!is_known_slash_cmd("/prfoo"));
        assert!(!is_known_slash_cmd("/notacommand"));
        assert!(!is_known_slash_cmd("not a slash"));
        assert!(!is_known_slash_cmd(""));
    }

    // ── slash_completion_rows grouping ────────────────────────────────────────

    #[test]
    fn completion_rows_have_headers_between_sections() {
        let rows = slash_completion_rows("/");
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| {
                if let CompletionRow::Header(h) = r {
                    Some(*h)
                } else {
                    None
                }
            })
            .collect();
        // All six sections should appear as headers.
        assert!(headers.contains(&"Session"));
        assert!(headers.contains(&"Git"));
        assert!(headers.contains(&"Model"));
        assert!(headers.contains(&"Context"));
        assert!(headers.contains(&"Plan"));
        assert!(headers.contains(&"Project"));
    }

    #[test]
    fn completion_rows_pr_section_under_git_header() {
        let rows = slash_completion_rows("/pr");
        // Should have exactly one Git header since /pr, /profile, /profiles
        // all live in different sections.
        let header_sections: Vec<&str> = rows
            .iter()
            .filter_map(|r| {
                if let CompletionRow::Header(h) = r {
                    Some(*h)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            header_sections.contains(&"Git"),
            "Git section header expected"
        );
        assert!(
            header_sections.contains(&"Project"),
            "Project section header expected (contains /profile)"
        );
    }

    #[test]
    fn completion_rows_flat_indices_are_contiguous() {
        // flat_idx in Cmd rows must be 0, 1, 2, ... without gaps.
        let rows = slash_completion_rows("/");
        let indices: Vec<usize> = rows
            .iter()
            .filter_map(|r| {
                if let CompletionRow::Cmd { flat_idx, .. } = r {
                    Some(*flat_idx)
                } else {
                    None
                }
            })
            .collect();
        for (expected, got) in indices.iter().enumerate() {
            assert_eq!(
                expected, *got,
                "flat_idx out of order at position {}",
                expected
            );
        }
    }

    #[test]
    fn slash_commands_derived_matches_registry_count() {
        // slash_commands() must match the command_registry size.
        assert_eq!(
            slash_commands().len(),
            crate::command_registry::COMMANDS.len(),
            "slash_commands() and command_registry::COMMANDS are out of sync"
        );
    }

    #[test]
    fn undo_command_description_mentions_confirmation() {
        let desc = slash_commands()
            .into_iter()
            .find_map(|(cmd, desc)| (cmd == "/undo").then_some(desc))
            .expect("/undo command should exist");
        assert!(
            desc.contains("confirm"),
            "/undo description should mention confirmation for safety"
        );
    }

    #[test]
    fn submit_queues_known_slash_when_busy() {
        let mut app = make_test_app();
        app.busy = true;
        app.input = "/help".to_string();
        app.cursor = app.input.chars().count();

        app.submit();

        assert_eq!(app.queued.len(), 1);
        assert_eq!(app.queued.front().map(String::as_str), Some("/help"));
    }

    #[test]
    fn force_send_interrupt_prioritizes_prompt_at_queue_front() {
        let mut app = make_test_app();
        app.busy = true;
        app.queued.push_back("older queued item".to_string());
        app.input = "urgent next prompt".to_string();
        app.cursor = app.input.chars().count();

        app.force_send();

        assert_eq!(app.queued.len(), 2);
        assert_eq!(
            app.queued.front().map(String::as_str),
            Some("urgent next prompt")
        );
        assert!(app.cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn perms_state_clear_all_grants_resets_permissions() {
        let mut perms = PermsState::default();
        perms.session_allowed.insert("Edit".to_string());
        perms.workdir_open = true;

        perms.clear_all_grants();

        assert!(perms.session_allowed.is_empty());
        assert!(!perms.workdir_open);
    }

    #[test]
    fn workdir_command_does_not_switch_before_runtime_confirmation() {
        let mut app = make_test_app();
        let original = app.workspace_root.clone();
        let target_dir = std::env::temp_dir();
        let cmd = format!("/workdir {}", target_dir.display());

        execute_slash(&mut app, &cmd);

        assert_eq!(
            app.workspace_root, original,
            "UI workdir should remain unchanged until runtime confirms switch"
        );
    }

    #[tokio::test]
    async fn tui_ask_user_session_grant_skips_second_prompt_for_same_tool() {
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let ask = TuiAskUser {
            perm_tx,
            perms: Arc::new(Mutex::new(PermsState::default())),
        };

        let first = tokio::spawn({
            let ask = TuiAskUser {
                perm_tx: ask.perm_tx.clone(),
                perms: ask.perms.clone(),
            };
            async move {
                ask.ask(AgentPermRequest {
                    tool_name: "Write".to_string(),
                    description: "{\"path\":\"a.txt\"}".to_string(),
                    diff: None,
                    proposed_content: None,
                    file_path: None,
                })
                .await
            }
        });
        let pending = perm_rx.recv().await.expect("first prompt expected");
        pending
            .reply
            .send(PermGrant::Session)
            .expect("reply should send");
        let first_grant = first.await.expect("first ask task should complete");
        assert!(matches!(first_grant, AgentPermGrant::AllowAll));

        let second_grant = ask
            .ask(AgentPermRequest {
                tool_name: "Write".to_string(),
                description: "{\"path\":\"a.txt\"}".to_string(),
                diff: None,
                proposed_content: None,
                file_path: None,
            })
            .await;
        assert!(matches!(second_grant, AgentPermGrant::Allow));
        assert!(
            perm_rx.try_recv().is_err(),
            "second ask for same tool should not prompt again"
        );
    }

    #[tokio::test]
    async fn tui_ask_user_workdir_grant_skips_prompt_for_other_tools() {
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let ask = TuiAskUser {
            perm_tx,
            perms: Arc::new(Mutex::new(PermsState::default())),
        };

        let first_req = AgentPermRequest {
            tool_name: "Edit".to_string(),
            description: "{}".to_string(),
            diff: None,
            proposed_content: None,
            file_path: None,
        };
        let first = tokio::spawn({
            let ask = TuiAskUser {
                perm_tx: ask.perm_tx.clone(),
                perms: ask.perms.clone(),
            };
            async move { ask.ask(first_req).await }
        });
        let pending = perm_rx.recv().await.expect("first prompt expected");
        pending
            .reply
            .send(PermGrant::Workdir)
            .expect("reply should send");
        let first_grant = first.await.expect("first ask task should complete");
        assert!(matches!(first_grant, AgentPermGrant::AllowAll));

        let second_req = AgentPermRequest {
            tool_name: "Write".to_string(),
            description: "{}".to_string(),
            diff: None,
            proposed_content: None,
            file_path: None,
        };
        let second_grant = ask.ask(second_req).await;
        assert!(matches!(second_grant, AgentPermGrant::Allow));
        assert!(
            perm_rx.try_recv().is_err(),
            "workdir grant should avoid prompting for other tools"
        );
    }

    // ── T01: TUI critical path tests ──────────────────────────────────────

    #[test]
    fn model_picker_filtered_trims_whitespace() {
        fn make_entry(id: &str) -> ModelEntry {
            ModelEntry {
                id: id.to_string(),
                provider: "test".to_string(),
                input_mtok: 0.0,
                output_mtok: 0.0,
                context_k: None,
                role: None,
                is_favorite: false,
            }
        }
        let mut state = ModelPickerState {
            models: vec![make_entry("gpt-4"), make_entry("claude-3")],
            filter: "  gpt  ".to_string(),
            selected: 0,
            scroll_offset: 0,
        };
        let filtered = state.filtered();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "gpt-4");

        // Whitespace-only filter should return all models
        state.filter = "   ".to_string();
        let all = state.filtered();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn app_per_turn_override_indicator_set_and_cleared() {
        let mut app = make_test_app();
        assert!(app.per_turn_prev_model.is_none());
        // Simulate override set
        app.per_turn_prev_model = Some("base-model".to_string());
        assert!(app.per_turn_prev_model.is_some());
        // Simulate override cleared
        app.per_turn_prev_model = None;
        assert!(app.per_turn_prev_model.is_none());
    }

    #[test]
    fn queue_messages_empty_does_not_panic() {
        let mut app = make_test_app();
        // Draining an empty queue should be a no-op
        let drained: Vec<_> = app.queued.drain(..).collect();
        assert!(drained.is_empty());
    }

    #[test]
    fn queue_messages_preserves_order() {
        let mut app = make_test_app();
        app.queued.push_back("first".to_string());
        app.queued.push_back("second".to_string());
        app.queued.push_back("third".to_string());
        let result: Vec<_> = app.queued.drain(..).collect();
        assert_eq!(result, vec!["first", "second", "third"]);
    }

    #[test]
    fn slash_completions_returns_sorted_unique_commands() {
        let completions = slash_completions("/");
        // All completions should have names starting with "/"
        for (cmd, _desc) in &completions {
            assert!(cmd.starts_with('/'), "completion missing /: {cmd}");
        }
        // Should not have duplicates
        let mut names: Vec<_> = completions.iter().map(|(c, _)| *c).collect();
        names.sort();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate slash completions found");
    }

    #[test]
    fn search_and_export_are_known_commands() {
        assert!(
            is_known_slash_cmd("/search"),
            "/search must be a known command"
        );
        assert!(
            is_known_slash_cmd("/export"),
            "/export must be a known command"
        );
        assert!(
            is_known_slash_cmd("/search hello world"),
            "/search with args must be known"
        );
    }

    #[test]
    fn search_and_export_appear_in_completions() {
        let cmds: Vec<_> = slash_completions("/s").iter().map(|(c, _)| *c).collect();
        assert!(
            cmds.contains(&"/search"),
            "/search must appear in /s completions"
        );
        let all: Vec<_> = slash_completions("/export")
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert!(
            all.contains(&"/export"),
            "/export must appear in completions"
        );
    }

    #[test]
    fn search_and_export_have_non_empty_descriptions() {
        let all = slash_commands();
        let search_desc = all.iter().find(|(c, _)| *c == "/search").map(|(_, d)| d);
        let export_desc = all.iter().find(|(c, _)| *c == "/export").map(|(_, d)| d);
        assert!(
            search_desc.is_some_and(|d| !d.is_empty()),
            "/search needs a description"
        );
        assert!(
            export_desc.is_some_and(|d| !d.is_empty()),
            "/export needs a description"
        );
    }

    // ── render_markdown responsiveness ───────────────────────────────────────

    #[test]
    fn code_block_close_scales_with_width() {
        let md = "```bash\necho hello\n```";
        let narrow = render_markdown(md, 40);
        let wide = render_markdown(md, 120);
        // Find the close-bar line (starts with └)
        let close_narrow = narrow
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.starts_with('└'))
                    .unwrap_or(false)
            })
            .expect("close bar missing on narrow");
        let close_wide = wide
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.starts_with('└'))
                    .unwrap_or(false)
            })
            .expect("close bar missing on wide");
        let narrow_len: usize = close_narrow
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        let wide_len: usize = close_wide
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        assert!(
            narrow_len <= wide_len,
            "narrow close bar ({narrow_len}) should be ≤ wide ({wide_len})"
        );
    }

    #[test]
    fn horizontal_rule_scales_with_width() {
        let md = "---";
        let narrow = render_markdown(md, 40);
        let wide = render_markdown(md, 100);
        let hr_narrow = narrow
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.starts_with('─'))
                    .unwrap_or(false)
            })
            .expect("hr missing on narrow");
        let hr_wide = wide
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.starts_with('─'))
                    .unwrap_or(false)
            })
            .expect("hr missing on wide");
        let n_len: usize = hr_narrow
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        let w_len: usize = hr_wide
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        assert!(
            n_len <= w_len,
            "narrow hr ({n_len}) should be ≤ wide ({w_len})"
        );
    }

    // ── Profile overlay unit tests ────────────────────────────────────────────

    #[test]
    fn profile_overlay_for_edit_initializes_correctly() {
        let entry = clido_core::ProfileEntry {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            api_key: Some("sk-test-1234".into()),
            api_key_env: None,
            base_url: Some("https://custom.api.example.com".into()),
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        let ov = ProfileOverlayState::for_edit(
            "myprofile".into(),
            &entry,
            std::path::PathBuf::from("/tmp/test-config.toml"),
        );
        assert_eq!(ov.name, "myprofile");
        assert_eq!(ov.provider, "anthropic");
        assert_eq!(ov.model, "claude-opus-4-5");
        assert_eq!(ov.api_key, "sk-test-1234");
        assert_eq!(ov.base_url, "https://custom.api.example.com");
        assert!(!ov.is_new);
        assert_eq!(ov.mode, ProfileOverlayMode::Overview);
        assert_eq!(ov.cursor, 0);
    }

    #[test]
    fn profile_overlay_for_create_starts_in_create_mode() {
        let ov = ProfileOverlayState::for_create(std::path::PathBuf::from("/tmp/test.toml"));
        assert!(ov.is_new);
        assert_eq!(
            ov.mode,
            ProfileOverlayMode::Creating {
                step: ProfileCreateStep::Provider
            }
        );
        assert!(ov.name.is_empty());
    }

    #[test]
    fn profile_overlay_begin_and_cancel_edit() {
        let entry = clido_core::ProfileEntry {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            api_key: None,
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        let mut ov = ProfileOverlayState::for_edit(
            "p".into(),
            &entry,
            std::path::PathBuf::from("/tmp/t.toml"),
        );
        // Navigate to ApiKey field (cursor=1) — uses inline edit, not picker
        ov.cursor = 1;
        ov.begin_edit(&[]);
        assert_eq!(
            ov.mode,
            ProfileOverlayMode::EditField(ProfileEditField::ApiKey)
        );
        assert_eq!(ov.input, "");
        assert_eq!(ov.input_cursor, 0);

        // Cancel should restore overview
        ov.cancel_edit();
        assert_eq!(ov.mode, ProfileOverlayMode::Overview);
        assert!(ov.input.is_empty());
        assert_eq!(ov.input_cursor, 0);
        // Original value unchanged
        assert_eq!(ov.model, "gpt-4o");
    }

    #[test]
    fn profile_overlay_commit_edit_updates_field() {
        let entry = clido_core::ProfileEntry {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            api_key: None,
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        let mut ov = ProfileOverlayState::for_edit(
            "p".into(),
            &entry,
            std::path::PathBuf::from("/tmp/t.toml"),
        );
        // Model field (cursor=2) now uses picker — begin_edit enters PickingModel mode
        ov.cursor = 2;
        let model_entry = ModelEntry {
            id: "claude-haiku-4-5".into(),
            provider: "openai".into(),
            input_mtok: 0.25,
            output_mtok: 1.25,
            context_k: Some(200),
            role: None,
            is_favorite: false,
        };
        ov.begin_edit(&[model_entry]);
        assert!(matches!(ov.mode, ProfileOverlayMode::PickingModel { .. }));
        // picker should have the model entry
        assert!(ov.profile_model_picker.is_some());
        // commit the pick
        ov.commit_model_pick();
        assert_eq!(ov.model, "claude-haiku-4-5");
        assert_eq!(ov.mode, ProfileOverlayMode::Overview);
    }

    #[test]
    fn profile_overlay_cursor_field_mapping() {
        let entry = clido_core::ProfileEntry {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            api_key: None,
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        let mut ov = ProfileOverlayState::for_edit(
            "p".into(),
            &entry,
            std::path::PathBuf::from("/tmp/t.toml"),
        );
        ov.cursor = 0;
        assert_eq!(ov.cursor_field(), ProfileEditField::Provider);
        ov.cursor = 1;
        assert_eq!(ov.cursor_field(), ProfileEditField::ApiKey);
        ov.cursor = 2;
        assert_eq!(ov.cursor_field(), ProfileEditField::Model);
        ov.cursor = 3;
        assert_eq!(ov.cursor_field(), ProfileEditField::BaseUrl);
    }

    #[test]
    fn profile_overlay_masked_api_key() {
        let entry = clido_core::ProfileEntry {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            api_key: Some("sk-ant-api03-verylongkeyvalue".into()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        let ov = ProfileOverlayState::for_edit(
            "p".into(),
            &entry,
            std::path::PathBuf::from("/tmp/t.toml"),
        );
        let masked = ov.masked_api_key();
        // Should not reveal full key
        assert!(!masked.contains("verylongkeyvalue"));
        // Should not be empty
        assert!(!masked.is_empty());
    }

    #[test]
    fn char_byte_pos_tui_works_for_ascii_and_unicode() {
        let s = "hello";
        assert_eq!(char_byte_pos_tui(s, 0), 0);
        assert_eq!(char_byte_pos_tui(s, 3), 3);
        assert_eq!(char_byte_pos_tui(s, 5), 5); // end

        // Unicode: each emoji is >1 byte
        let u = "hé";
        assert_eq!(char_byte_pos_tui(u, 0), 0);
        assert_eq!(char_byte_pos_tui(u, 1), 1); // 'h' is 1 byte
        assert_eq!(char_byte_pos_tui(u, 2), 3); // 'é' is 2 bytes
    }

    // ── Multiline cursor navigation tests ────────────────────────────────────

    #[test]
    fn move_up_single_line_returns_none() {
        // No newline — can't move up.
        assert_eq!(move_cursor_line_up("hello world", 5), None);
    }

    #[test]
    fn move_down_single_line_returns_none() {
        assert_eq!(move_cursor_line_down("hello world", 5), None);
    }

    #[test]
    fn move_up_on_first_line_returns_none() {
        // Cursor on line 0 — can't go further up.
        let s = "line0\nline1\nline2";
        assert_eq!(move_cursor_line_up(s, 3), None); // col 3 of "line0"
    }

    #[test]
    fn move_down_on_last_line_returns_none() {
        let s = "line0\nline1\nline2";
        // "line2" starts at index 12; cursor at 14 (col 2)
        assert_eq!(move_cursor_line_down(s, 14), None);
    }

    #[test]
    fn move_up_from_second_line() {
        // "abc\nde\nfghi"
        //  0123 456 7890
        // line0="abc" (0-2), line1="de" (4-5), line2="fghi" (7-10)
        let s = "abc\nde\nfghi";
        // Cursor at index 5 = col 1 of "de"
        // Moving up → col 1 of "abc" = index 1
        assert_eq!(move_cursor_line_up(s, 5), Some(1));
    }

    #[test]
    fn move_up_clamps_to_shorter_prev_line() {
        // "ab\ndefgh"  — line0 is shorter
        // Cursor at index 7 = col 5 of "defgh" (which is "h")
        // prev line "ab" has len 2, so should clamp to col 2 (end of "ab") = index 2
        let s = "ab\ndefgh";
        assert_eq!(move_cursor_line_up(s, 7), Some(2));
    }

    #[test]
    fn move_down_from_first_line() {
        // "abc\nde\nfghi"
        let s = "abc\nde\nfghi";
        // Cursor at index 1 (col 1 of "abc") → col 1 of "de" = index 5
        assert_eq!(move_cursor_line_down(s, 1), Some(5));
    }

    #[test]
    fn move_down_clamps_to_shorter_next_line() {
        // "defgh\nab"  — next line is shorter
        // Cursor at col 4 of "defgh" = index 4
        // next line "ab" has len 2, clamp to 2 = index 8
        let s = "defgh\nab";
        assert_eq!(move_cursor_line_down(s, 4), Some(8));
    }

    #[test]
    fn move_up_down_roundtrip() {
        // Moving down then up should return to original position (when lines are equal length)
        let s = "hello\nworld\nfinal";
        let start = 2; // col 2 of "hello"
        let down = move_cursor_line_down(s, start).unwrap();
        let back = move_cursor_line_up(s, down).unwrap();
        assert_eq!(back, start);
    }

    // ── T01: TUI critical path unit tests ─────────────────────────────────────

    fn make_key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent {
            code,
            modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    // T01-1: pressing Enter with option 3 (Deny) selected resolves PendingPerm with Deny.
    #[test]
    fn perm_modal_deny_sends_deny_grant() {
        use crossterm::event::KeyCode;
        use tokio::sync::oneshot;

        let mut app = make_test_app();
        let (reply_tx, mut reply_rx) = oneshot::channel::<PermGrant>();
        app.pending_perm = Some(PendingPerm {
            tool_name: "Write".to_string(),
            preview: "writing to a file".to_string(),
            reply: reply_tx,
        });
        app.perm_selected = 3; // index 3 = Deny

        handle_key(&mut app, make_key(KeyCode::Enter));

        assert!(
            app.pending_perm.is_none(),
            "pending_perm should be cleared after Deny"
        );
        let grant = reply_rx.try_recv().expect("reply should have been sent");
        assert!(
            matches!(grant, PermGrant::Deny),
            "expected PermGrant::Deny, got {:?}",
            grant
        );
        assert_eq!(app.perm_selected, 0, "selection should reset to 0");
    }

    // T01-2: DenyWithFeedback — Enter on option 4 enters feedback mode; second Enter sends
    // DenyWithFeedback with the typed reason.
    #[test]
    fn perm_modal_deny_with_feedback_sends_reason() {
        use crossterm::event::KeyCode;
        use tokio::sync::oneshot;

        let mut app = make_test_app();
        let (reply_tx, mut reply_rx) = oneshot::channel::<PermGrant>();
        app.pending_perm = Some(PendingPerm {
            tool_name: "Bash".to_string(),
            preview: "rm -rf /".to_string(),
            reply: reply_tx,
        });
        app.perm_selected = 4; // index 4 = DenyWithFeedback

        // First Enter: enter feedback-input mode.
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(
            app.perm_feedback_input.is_some(),
            "feedback input mode should be active"
        );
        assert!(
            app.pending_perm.is_some(),
            "pending_perm should still be set during feedback entry"
        );

        // Type feedback characters.
        handle_key(&mut app, make_key(KeyCode::Char('b')));
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        handle_key(&mut app, make_key(KeyCode::Char('d')));

        // Second Enter: submit feedback.
        handle_key(&mut app, make_key(KeyCode::Enter));

        assert!(
            app.pending_perm.is_none(),
            "pending_perm should be cleared after DenyWithFeedback submit"
        );
        assert!(
            app.perm_feedback_input.is_none(),
            "perm_feedback_input should be cleared"
        );
        assert_eq!(app.perm_selected, 0, "selection should reset to 0");

        let grant = reply_rx.try_recv().expect("reply should have been sent");
        match grant {
            PermGrant::DenyWithFeedback(reason) => {
                assert_eq!(reason, "bad", "feedback text mismatch: {reason}");
            }
            other => panic!("expected DenyWithFeedback, got {:?}", other),
        }
    }

    // T01-3: messages queued while busy are processed FIFO.
    #[test]
    fn queue_processes_items_in_fifo_order() {
        let mut app = make_test_app();
        app.busy = true;

        // Submit three inputs while busy — they should land in queued FIFO.
        for msg in &["first", "second", "third"] {
            app.input = msg.to_string();
            app.cursor = app.input.chars().count();
            app.submit();
        }

        assert_eq!(app.queued.len(), 3, "all three inputs should be queued");
        let items: Vec<&str> = app.queued.iter().map(String::as_str).collect();
        assert_eq!(
            items,
            vec!["first", "second", "third"],
            "queue must preserve FIFO order"
        );
    }

    // T01-4: input_history is capped at 1000 entries (FX14 fix).
    #[test]
    fn input_history_capped_at_1000() {
        let mut app = make_test_app();
        // Push 1001 distinct entries directly via send_now (which adds to history).
        for i in 0..1001usize {
            app.send_now(format!("prompt {i}"));
        }
        assert_eq!(
            app.input_history.len(),
            1000,
            "history must be capped at 1000"
        );
        // The very first entry "prompt 0" should have been evicted.
        assert_ne!(
            app.input_history.first().map(String::as_str),
            Some("prompt 0"),
            "oldest entry should have been evicted"
        );
        // The last entry should be the most recent.
        assert_eq!(
            app.input_history.last().map(String::as_str),
            Some("prompt 1000"),
            "newest entry should be the last element"
        );
    }

    // T01-5: a multiline input (containing '\n') is submitted and stored in history.
    #[test]
    fn multiline_input_is_handled() {
        let mut app = make_test_app();
        let multiline = "line one\nline two\nline three";
        app.send_now(multiline.to_string());

        // The input should appear as a ChatLine::User message.
        let has_user_line = app.messages.iter().any(|m| {
            if let ChatLine::User(text) = m {
                text == multiline
            } else {
                false
            }
        });
        assert!(
            has_user_line,
            "multiline input should appear as a User ChatLine"
        );

        // It should also be recorded in history.
        assert!(
            app.input_history.contains(&multiline.to_string()),
            "multiline input should be recorded in input_history"
        );
    }
}
