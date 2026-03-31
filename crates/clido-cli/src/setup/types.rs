//! Setup types, state, and picker helpers.

use crate::list_picker::{ListPicker, PickerItem};
use crate::text_input::TextInput;

use clido_providers::registry::PROVIDER_REGISTRY;
use clido_providers::ModelEntry;

/// Current config values passed to the setup wizard when re-running from TUI (/init).
/// Each field is optional — empty string = nothing known.
pub struct SetupPreFill {
    /// Current provider ID (e.g. "anthropic", "openrouter").
    pub provider: String,
    /// Current API key (shown masked, Enter to keep).
    pub api_key: String,
    /// Current model ID (pre-selected in list).
    pub model: String,
    /// Current roles (pre-populated in roles step).
    pub roles: Vec<(String, String)>,
    /// Profile name (used in profile-create / profile-edit flows).
    pub profile_name: String,
    /// True when creating a brand-new named profile (shows ProfileName step first).
    pub is_new_profile: bool,
    /// Plaintext API keys from other profiles (profile create / reuse flow).
    pub saved_api_keys: Vec<SavedApiKeyOffer>,
}

/// A stored API key offered for reuse (same provider only in the UI).
#[derive(Debug, Clone)]
pub struct SavedApiKeyOffer {
    pub source_profile: String,
    pub provider_id: String,
    pub api_key: String,
}

/// Build reuse offers from profiles with plaintext `api_key` (deduped by key value).
pub(crate) fn build_saved_key_catalog(
    loaded: &clido_core::LoadedConfig,
    exclude_profile: Option<&str>,
) -> Vec<SavedApiKeyOffer> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let mut keys: Vec<&String> = loaded.profiles.keys().collect();
    keys.sort();
    for name in keys {
        if exclude_profile == Some(name.as_str()) {
            continue;
        }
        let entry = &loaded.profiles[name];
        let Some(ref k) = entry.api_key else {
            continue;
        };
        if k.is_empty() {
            continue;
        }
        if !seen.insert(k.clone()) {
            continue;
        }
        out.push(SavedApiKeyOffer {
            source_profile: name.clone(),
            provider_id: entry.provider.clone(),
            api_key: k.clone(),
        });
    }
    out
}

// ── ListPicker wrapper types ──────────────────────────────────────────────────

/// Wraps a provider-registry index for use in [`ListPicker`].
#[derive(Debug, Clone)]
pub(super) struct ProviderEntry(pub usize);

impl PickerItem for ProviderEntry {
    fn filter_text(&self) -> String {
        PROVIDER_REGISTRY[self.0].name.to_string()
    }
    fn filter_text_secondary(&self) -> Option<String> {
        Some(PROVIDER_REGISTRY[self.0].description.to_string())
    }
}

/// A model-picker item: either a fetched model or the "Custom…" sentinel.
#[derive(Debug, Clone)]
pub(super) enum ModelOption {
    Entry(ModelEntry),
    Custom,
}

impl PickerItem for ModelOption {
    fn filter_text(&self) -> String {
        match self {
            ModelOption::Entry(m) => m.id.clone(),
            ModelOption::Custom => "Custom\u{2026}".to_string(),
        }
    }
}

pub(super) fn make_provider_picker() -> ListPicker<ProviderEntry> {
    let items: Vec<ProviderEntry> = (0..PROVIDER_REGISTRY.len()).map(ProviderEntry).collect();
    ListPicker::without_filter(items, 20)
}

pub(super) fn make_provider_picker_at(selected: usize) -> ListPicker<ProviderEntry> {
    let mut picker = make_provider_picker();
    picker.selected = selected.min(PROVIDER_REGISTRY.len().saturating_sub(1));
    picker
}

pub(super) fn make_model_picker(models: &[ModelEntry]) -> ListPicker<ModelOption> {
    let items: Vec<ModelOption> = models
        .iter()
        .cloned()
        .map(ModelOption::Entry)
        .chain(std::iter::once(ModelOption::Custom))
        .collect();
    ListPicker::new(items, 10)
}

// ── TUI setup state ───────────────────────────────────────────────────────────

/// Setup steps: [ProfileName →] Provider → Credential → FetchModels → Model → SubAgentIntro → [Worker] → [Reviewer] → Roles → Done.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum SetupStep {
    /// New named profile: ask for a profile name first.
    ProfileName,
    // Main agent
    Provider,
    Credential,
    FetchingModels,
    Model,
    // Sub-agents (optional)
    SubAgentIntro,
    WorkerProvider,
    WorkerCredential,
    FetchingWorkerModels,
    WorkerModel,
    ReviewerProvider,
    ReviewerCredential,
    FetchingReviewerModels,
    ReviewerModel,
    // Roles (optional)
    Roles,
}

pub(super) struct SetupState {
    pub step: SetupStep,
    pub needs_fetch: bool,
    /// Profile name when in profile-create / profile-edit mode.
    pub profile_name: String,
    pub provider_picker: ListPicker<ProviderEntry>,
    pub model_picker: ListPicker<ModelOption>,
    pub custom_model: bool,
    pub provider: usize,
    pub credential: String,
    pub model: String,
    pub text_input: TextInput,
    pub fetched_models: Vec<ModelEntry>,
    // ── Roles step ────────────────────────────────────────────
    pub roles: Vec<(String, String)>, // (role_name, model_id)
    pub role_cursor: usize,
    pub role_edit_field: RoleEditField,
    pub role_input: String, // text being typed in a role field
    // ── Sub-agent configuration ────────────────────────────────
    pub subagent_intro_cursor: usize,
    pub configure_worker: bool,
    pub configure_reviewer: bool,
    pub worker_provider: usize,
    pub worker_provider_picker: ListPicker<ProviderEntry>,
    pub worker_credential: String,
    pub worker_model: String,
    pub worker_fetched_models: Vec<ModelEntry>,
    pub worker_model_picker: ListPicker<ModelOption>,
    pub worker_custom_model: bool,
    pub reviewer_provider: usize,
    pub reviewer_provider_picker: ListPicker<ProviderEntry>,
    pub reviewer_credential: String,
    pub reviewer_model: String,
    pub reviewer_fetched_models: Vec<ModelEntry>,
    pub reviewer_model_picker: ListPicker<ModelOption>,
    pub reviewer_custom_model: bool,
    pub worker_needs_fetch: bool,
    pub reviewer_needs_fetch: bool,
    // ──────────────────────────────────────────────────────────
    pub error: Option<String>,
    /// Stored credential from pre-fill (kept so user can press Enter to keep it).
    pub current_credential: Option<String>,
    /// Current model ID from pre-fill (used to pre-select after model fetch).
    pub current_model: String,
    /// True when wizard began with the "new profile name" step (`/profile new`).
    pub started_with_profile_name: bool,
    /// Saved keys from config (profile create); filter by provider in credential step.
    pub saved_api_keys: Vec<SavedApiKeyOffer>,
    /// When true, credential step shows ↑↓ picker for `saved_api_keys` (non-local).
    pub credential_pick_active: bool,
    pub credential_pick_index: usize,
}

/// Result of the interactive setup TUI: finished configuration or user cancelled.
pub(super) enum SetupOutcome {
    Cancelled,
    Finished(Box<SetupState>),
}

/// Which field is being edited in the roles step.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum RoleEditField {
    None,
    Name(usize),  // editing role name at index (usize::MAX = new)
    Model(usize), // editing model id at index
}

impl SetupState {
    pub fn new() -> Self {
        Self {
            step: SetupStep::Provider,
            needs_fetch: false,
            profile_name: String::new(),
            provider_picker: make_provider_picker(),
            model_picker: make_model_picker(&[]),
            custom_model: false,
            provider: 0,
            credential: String::new(),
            model: String::new(),
            text_input: TextInput::new(),
            fetched_models: Vec::new(),
            roles: Vec::new(),
            role_cursor: 0,
            role_edit_field: RoleEditField::None,
            role_input: String::new(),
            subagent_intro_cursor: 0,
            configure_worker: false,
            configure_reviewer: false,
            worker_provider: 0,
            worker_provider_picker: make_provider_picker(),
            worker_credential: String::new(),
            worker_model: String::new(),
            worker_fetched_models: Vec::new(),
            worker_model_picker: make_model_picker(&[]),
            worker_custom_model: false,
            reviewer_provider: 0,
            reviewer_provider_picker: make_provider_picker(),
            reviewer_credential: String::new(),
            reviewer_model: String::new(),
            reviewer_fetched_models: Vec::new(),
            reviewer_model_picker: make_model_picker(&[]),
            reviewer_custom_model: false,
            worker_needs_fetch: false,
            reviewer_needs_fetch: false,
            error: None,
            current_credential: None,
            current_model: String::new(),
            started_with_profile_name: false,
            saved_api_keys: Vec::new(),
            credential_pick_active: false,
            credential_pick_index: 0,
        }
    }

    pub fn new_with_prefill(pre_fill: SetupPreFill) -> Self {
        let provider_idx = PROVIDER_REGISTRY
            .iter()
            .position(|def| def.id == pre_fill.provider.as_str())
            .unwrap_or(0);
        let current_credential = if pre_fill.api_key.is_empty() {
            None
        } else {
            Some(pre_fill.api_key)
        };
        let initial_step = if pre_fill.is_new_profile {
            SetupStep::ProfileName
        } else {
            SetupStep::Provider
        };
        Self {
            step: initial_step,
            needs_fetch: false,
            profile_name: pre_fill.profile_name.clone(),
            provider_picker: make_provider_picker_at(provider_idx),
            model_picker: make_model_picker(&[]),
            custom_model: false,
            provider: provider_idx,
            credential: String::new(),
            model: pre_fill.model.clone(),
            text_input: TextInput::new(),
            fetched_models: Vec::new(),
            roles: pre_fill.roles,
            role_cursor: 0,
            role_edit_field: RoleEditField::None,
            role_input: String::new(),
            subagent_intro_cursor: 0,
            configure_worker: false,
            configure_reviewer: false,
            worker_provider: provider_idx,
            worker_provider_picker: make_provider_picker_at(provider_idx),
            worker_credential: String::new(),
            worker_model: String::new(),
            worker_fetched_models: Vec::new(),
            worker_model_picker: make_model_picker(&[]),
            worker_custom_model: false,
            reviewer_provider: provider_idx,
            reviewer_provider_picker: make_provider_picker_at(provider_idx),
            reviewer_credential: String::new(),
            reviewer_model: String::new(),
            reviewer_fetched_models: Vec::new(),
            reviewer_model_picker: make_model_picker(&[]),
            reviewer_custom_model: false,
            worker_needs_fetch: false,
            reviewer_needs_fetch: false,
            error: None,
            current_credential,
            current_model: pre_fill.model,
            started_with_profile_name: pre_fill.is_new_profile,
            saved_api_keys: pre_fill.saved_api_keys.clone(),
            credential_pick_active: false,
            credential_pick_index: 0,
        }
    }

    pub fn clear_typed_input(&mut self) {
        self.text_input.clear();
    }

    pub fn init_credential_step(&mut self) {
        self.clear_typed_input();
        if self.is_local() {
            self.credential_pick_active = false;
            return;
        }
        let n = self.saved_keys_for_current_provider().len();
        self.credential_pick_active = n > 0;
        self.credential_pick_index = 0;
    }

    pub fn saved_keys_for_current_provider(&self) -> Vec<&SavedApiKeyOffer> {
        let pid = PROVIDER_REGISTRY[self.provider].id;
        self.saved_api_keys
            .iter()
            .filter(|o| o.provider_id == pid)
            .collect()
    }

    pub fn is_local(&self) -> bool {
        PROVIDER_REGISTRY[self.provider].is_local
    }

    pub fn key_env(&self) -> &'static str {
        PROVIDER_REGISTRY[self.provider].api_key_env
    }

    pub fn model_list_mode(&self) -> bool {
        !self.fetched_models.is_empty() && !self.custom_model
    }

    pub fn worker_model_list_mode(&self) -> bool {
        !self.worker_fetched_models.is_empty() && !self.worker_custom_model
    }

    pub fn reviewer_model_list_mode(&self) -> bool {
        !self.reviewer_fetched_models.is_empty() && !self.reviewer_custom_model
    }
}
