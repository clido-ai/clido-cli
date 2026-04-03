use clido_planner::{Complexity, Plan, PlanEditor};
use ratatui::style::Color;
use tokio::sync::{mpsc, oneshot};

use crate::list_picker::{ListPicker, PickerItem};

use super::render::parse_plan_from_text;
use super::{AgentEvent, PermGrant};

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
    #[allow(dead_code)]
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
}

impl Default for LayoutInfo {
    fn default() -> Self {
        Self {
            chat_area_y: (0, 0),
            chat_area_width: 120,
            max_scroll: 0,
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

/// mpsc senders used to communicate with the background agent task.
pub(crate) struct AgentChannels {
    pub(crate) prompt_tx: mpsc::UnboundedSender<String>,
    /// Channel to request session resume in agent_task.
    pub(crate) resume_tx: mpsc::UnboundedSender<String>,
    /// Channel to switch the session model in agent_task.
    pub(crate) model_switch_tx: mpsc::UnboundedSender<String>,
    /// Channel to update tool workspace in agent_task.
    pub(crate) workdir_tx: mpsc::UnboundedSender<std::path::PathBuf>,
    /// Channel to trigger immediate context compaction in agent_task.
    pub(crate) compact_now_tx: mpsc::UnboundedSender<()>,
    /// Channel to send AgentEvents from background tasks (e.g. model fetch) to the TUI loop.
    pub(crate) fetch_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Channel to force abort the agent task immediately (for /stop command).
    pub(crate) kill_tx: mpsc::UnboundedSender<()>,
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
        self.title
            .as_deref()
            .unwrap_or(&self.session_id)
            .to_string()
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

// ── Session picker popup state ────────────────────────────────────────────────

pub(crate) struct SessionPickerState {
    pub(crate) picker: ListPicker<clido_storage::SessionSummary>,
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
}

/// Steps for the in-TUI new profile wizard.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProfileCreateStep {
    Name,
    Provider,
    ApiKey,
    Model,
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
        let fast_provider = entry
            .fast
            .as_ref()
            .map(|f| f.provider.clone())
            .unwrap_or_default();
        let fast_api_key = entry
            .fast
            .as_ref()
            .and_then(|f| {
                f.api_key
                    .clone()
                    .or_else(|| f.api_key_env.as_ref().and_then(|e| std::env::var(e).ok()))
            })
            .unwrap_or_default();
        let fast_model = entry
            .fast
            .as_ref()
            .map(|f| f.model.clone())
            .unwrap_or_default();
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
        }
    }

    /// Open a blank state for creating a new profile.
    pub(crate) fn for_create(config_path: std::path::PathBuf) -> Self {
        Self {
            name: String::new(),
            provider: String::new(),
            api_key: String::new(),
            model: String::new(),
            base_url: String::new(),
            fast_provider: String::new(),
            fast_api_key: String::new(),
            fast_model: String::new(),
            cursor: 0,
            mode: ProfileOverlayMode::Creating {
                step: ProfileCreateStep::Name,
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
                let mut picker = ModelPickerState {
                    models: known_models.to_vec(),
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
            if let Err(e) = crate::setup::upsert_credential(
                &self.config_path,
                &self.provider,
                &self.api_key,
            ) {
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
