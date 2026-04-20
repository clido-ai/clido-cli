//! Setup types, state, and picker helpers.

use crate::list_picker::{ListPicker, PickerItem};
use crate::text_input::TextInput;

use clido_providers::registry::PROVIDER_REGISTRY;
use clido_providers::{ModelEntry, ModelMetadata};

/// Current config values passed to the setup wizard when re-running from TUI (/init).
/// Each field is optional — empty string = nothing known.
pub struct SetupPreFill {
    /// Current provider ID (e.g. "anthropic", "openrouter").
    pub provider: String,
    /// Current API key (shown masked, Enter to keep).
    pub api_key: String,
    /// Current model ID (pre-selected in list).
    pub model: String,
    /// Profile name (Used in profile-create / profile-edit flows).
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

/// Build reuse offers from profiles by checking credentials file, env vars, and
/// inline api_key (deduped by key value).
pub(crate) fn build_saved_key_catalog(
    loaded: &clido_core::LoadedConfig,
    config_path: &std::path::Path,
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

        // Try credentials file first
        let api_key = crate::setup::read_credential(config_path, &entry.provider)
            .or_else(|| {
                // Fall back to env var
                entry
                    .api_key_env
                    .as_ref()
                    .and_then(|e| std::env::var(e).ok())
            })
            .or_else(|| {
                // Last resort: inline key (legacy)
                entry.api_key.clone()
            });

        let Some(k) = api_key else {
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
            api_key: k,
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

/// A model-picker item: either a fetched model with metadata or the "Custom…" sentinel.
#[derive(Debug, Clone)]
pub(super) enum ModelOption {
    Metadata(ModelMetadata),
    Custom,
}

impl ModelOption {
    /// Backward compat: create from a simple ModelEntry.
    #[allow(dead_code)]
    pub fn from_entry(entry: ModelEntry) -> Self {
        Self::Metadata(ModelMetadata {
            id: entry.id,
            name: None,
            context_window: None,
            pricing: None,
            capabilities: clido_providers::ModelCapabilities::default(),
            status: clido_providers::ModelStatus::Active,
            release_date: None,
            available: entry.available,
        })
    }
}

impl PickerItem for ModelOption {
    fn filter_text(&self) -> String {
        match self {
            ModelOption::Metadata(m) => m.name.clone().unwrap_or_else(|| m.id.clone()),
            ModelOption::Custom => "Custom\u{2026}".to_string(),
        }
    }
    fn filter_text_secondary(&self) -> Option<String> {
        match self {
            ModelOption::Metadata(m) => {
                let mut parts = Vec::new();
                if m.name.is_some() {
                    parts.push(m.id.clone());
                }
                if let Some(ctx) = m.context_window {
                    parts.push(format_context(ctx));
                }
                if let Some(ref pricing) = m.pricing {
                    parts.push(format_pricing(pricing));
                }
                if m.capabilities.reasoning {
                    parts.push("🧠".to_string());
                }
                if !parts.is_empty() {
                    Some(parts.join("  "))
                } else {
                    None
                }
            }
            ModelOption::Custom => None,
        }
    }
}

fn format_context(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{}K", tokens / 1000)
    } else {
        format!("{} tokens", tokens)
    }
}

fn format_pricing(p: &clido_providers::ModelPricing) -> String {
    format!("${:.2}/${:.2}", p.input_per_mtok, p.output_per_mtok)
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

pub(super) fn make_model_picker(models: &[ModelMetadata]) -> ListPicker<ModelOption> {
    let items: Vec<ModelOption> = models
        .iter()
        .cloned()
        .map(ModelOption::Metadata)
        .chain(std::iter::once(ModelOption::Custom))
        .collect();
    ListPicker::new(items, 10)
}

// ── TUI setup state ───────────────────────────────────────────────────────────

/// Setup steps: [ProfileName →] Provider → Credential → FetchModels → Model → FastProviderIntro → [Fast] → Done.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum SetupStep {
    /// New named profile: ask for a profile name first.
    ProfileName,
    // Main agent
    Provider,
    Credential,
    FetchingModels,
    Model,
    // Fast provider (optional)
    FastProviderIntro,
    FastProvider,
    FastCredential,
    FetchingFastModels,
    FastModel,
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
    pub fetched_models: Vec<ModelMetadata>,
    // ── Fast provider configuration ───────────────────────────
    pub fast_intro_cursor: usize,
    pub configure_fast: bool,
    pub fast_provider_idx: usize,
    pub fast_provider_picker: ListPicker<ProviderEntry>,
    pub fast_credential: String,
    pub fast_model: String,
    pub fast_fetched_models: Vec<ModelMetadata>,
    pub fast_model_picker: ListPicker<ModelOption>,
    pub fast_custom_model: bool,
    pub fast_needs_fetch: bool,
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
    /// Saved credential for fast agent (displayed partially masked).
    pub current_fast_credential: Option<String>,
}

/// Result of the interactive setup TUI: finished configuration or user cancelled.
pub(super) enum SetupOutcome {
    Cancelled,
    Finished(Box<SetupState>),
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
            fast_intro_cursor: 0,
            configure_fast: false,
            fast_provider_idx: 0,
            fast_provider_picker: make_provider_picker(),
            fast_credential: String::new(),
            fast_model: String::new(),
            fast_fetched_models: Vec::new(),
            fast_model_picker: make_model_picker(&[]),
            fast_custom_model: false,
            fast_needs_fetch: false,
            error: None,
            current_credential: None,
            current_model: String::new(),
            started_with_profile_name: false,
            saved_api_keys: Vec::new(),
            credential_pick_active: false,
            credential_pick_index: 0,
            current_fast_credential: None,
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
            fast_intro_cursor: 0,
            configure_fast: false,
            fast_provider_idx: provider_idx,
            fast_provider_picker: make_provider_picker_at(provider_idx),
            fast_credential: String::new(),
            fast_model: String::new(),
            fast_fetched_models: Vec::new(),
            fast_model_picker: make_model_picker(&[]),
            fast_custom_model: false,
            fast_needs_fetch: false,
            error: None,
            current_credential,
            current_fast_credential: None, // Will be loaded when fast provider is selected
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

    pub fn saved_keys_for_fast_provider(&self) -> Vec<&SavedApiKeyOffer> {
        let pid = PROVIDER_REGISTRY[self.fast_provider_idx].id;
        self.saved_api_keys
            .iter()
            .filter(|o| o.provider_id == pid)
            .collect()
    }

    pub fn init_fast_credential_step(&mut self) {
        self.fast_credential.clear();
        if self.is_fast_local() {
            self.credential_pick_active = false;
            self.current_fast_credential = None;
            return;
        }
        // Collect saved keys first to avoid borrow issues
        let saved: Vec<SavedApiKeyOffer> = self
            .saved_keys_for_fast_provider()
            .into_iter()
            .cloned()
            .collect();
        self.credential_pick_active = !saved.is_empty();
        self.credential_pick_index = 0;
        // Load first saved key as current_fast_credential if available
        self.current_fast_credential = saved.first().map(|k| k.api_key.clone());
    }

    pub fn is_fast_local(&self) -> bool {
        PROVIDER_REGISTRY[self.fast_provider_idx].is_local
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

    pub fn fast_model_list_mode(&self) -> bool {
        !self.fast_fetched_models.is_empty() && !self.fast_custom_model
    }
}
