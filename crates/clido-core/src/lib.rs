//! Shared types, errors, and config for Clido.

pub mod config;
pub mod config_loader;
pub mod error;
pub mod model_prefs;
pub mod pricing;
pub mod skills;
pub mod tool_failure;
pub mod types;

pub use config::{
    evaluate_rules, AgentConfig, FastProviderConfig, HooksConfig, PermissionMode, PermissionRule,
    ProviderConfig, ProviderType, RuleAction, ExplorationConfig,
};
pub use config_loader::{
    agent_config_from_loaded, config_file_exists, delete_profile_from_config, global_config_dir,
    global_config_path, load_config, set_skill_disabled_in_project, switch_active_profile,
    upsert_profile_in_config, LoadedConfig, ProfileEntry, SkillsSection,
};
pub use error::{ClidoError, Result};
pub use model_prefs::ModelPrefs;
pub use pricing::{compute_cost_usd, load_pricing, ModelPricingEntry, PricingTable};
pub use tool_failure::ToolFailureKind;
pub use types::{ContentBlock, Message, ModelResponse, Role, StopReason, ToolSchema, Usage};

/// Number of consecutive identical tool failures before doom-loop detection triggers.
/// Exported so ClidoError::DoomLoop can reference it in its Display impl.
pub const DOOM_LOOP_THRESHOLD_DISPLAY: usize = 3;
