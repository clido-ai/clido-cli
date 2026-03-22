//! Load and merge config.toml from CLIDO_CONFIG, global, and project paths.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::{AgentConfig, HooksConfig, PermissionMode};
use crate::{ClidoError, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileEntry {
    pub provider: String,
    pub model: String,
    /// API key stored directly in config (takes priority over api_key_env).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Name of the environment variable that holds the API key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AgentSection {
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_max_budget")]
    pub max_budget_usd: Option<f64>,
    /// Also accepted as `max-parallel-tools` (CLI name). Config key is `max-concurrent-tools`.
    #[serde(default, alias = "max-parallel-tools")]
    pub max_concurrent_tools: Option<u32>,
    /// Suppress spinner, tool lifecycle output, and cost footer.
    /// Can be set persistently here; `--quiet` / `-q` CLI flag also sets this.
    #[serde(default)]
    pub quiet: bool,
    /// Skip all CLIDO.md / rules file injection.
    #[serde(default)]
    pub no_rules: bool,
    /// Use a specific rules file instead of the standard hierarchical lookup.
    #[serde(default)]
    pub rules_file: Option<String>,
    /// Send desktop notification + terminal bell when a task completes (requires
    /// the `desktop-notify` feature to be compiled in for the OS notification;
    /// the terminal bell fires regardless).
    #[serde(default)]
    pub notify: bool,
    /// Enable automatic checkpoint before file-mutating agent turns. Default: true.
    #[serde(default = "default_true")]
    pub auto_checkpoint: bool,
    /// Maximum number of checkpoints retained per session (0 = unlimited). Default: 50.
    #[serde(default = "default_max_checkpoints")]
    pub max_checkpoints_per_session: usize,
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            max_turns: default_max_turns(),
            max_budget_usd: default_max_budget(),
            max_concurrent_tools: None,
            quiet: false,
            no_rules: false,
            rules_file: None,
            notify: false,
            auto_checkpoint: true,
            max_checkpoints_per_session: 50,
        }
    }
}

fn default_max_turns() -> u32 {
    200
}
fn default_max_budget() -> Option<f64> {
    Some(5.0)
}
fn default_true() -> bool {
    true
}
fn default_max_checkpoints() -> usize {
    50
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ToolsSection {
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub disallowed: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ContextSection {
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: f64,
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
}

fn default_compaction_threshold() -> f64 {
    0.75
}

impl Default for ContextSection {
    fn default() -> Self {
        Self {
            compaction_threshold: default_compaction_threshold(),
            max_context_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct WorkflowsSection {
    #[serde(default = "default_workflows_directory")]
    pub directory: String,
}

/// Maps role names to model IDs. Built-in roles: fast, reasoning, critic, planner.
/// Arbitrary extra roles can be added freely.
///
/// Example in config.toml:
/// ```toml
/// [roles]
/// fast      = "claude-haiku-4-5-20251001"
/// reasoning = "claude-opus-4-6"
/// critic    = "claude-opus-4-6"
/// planner   = "claude-sonnet-4-6"
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct RolesSection {
    /// Fast, cheap model for quick tasks.
    #[serde(default)]
    pub fast: Option<String>,
    /// High-quality reasoning model.
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Evaluation / critique model.
    #[serde(default)]
    pub critic: Option<String>,
    /// Task decomposition / planning model.
    #[serde(default)]
    pub planner: Option<String>,
    /// Arbitrary user-defined roles.
    #[serde(flatten)]
    pub extra: HashMap<String, String>,
}

impl RolesSection {
    /// Return all roles as a flat map of name → model ID.
    pub fn as_map(&self) -> HashMap<String, String> {
        let mut map = self.extra.clone();
        if let Some(m) = &self.fast {
            map.insert("fast".into(), m.clone());
        }
        if let Some(m) = &self.reasoning {
            map.insert("reasoning".into(), m.clone());
        }
        if let Some(m) = &self.critic {
            map.insert("critic".into(), m.clone());
        }
        if let Some(m) = &self.planner {
            map.insert("planner".into(), m.clone());
        }
        map
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct IndexSection {
    /// Glob patterns to exclude when building the index (e.g. `["*.lock", "vendor/**"]`).
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
    /// When true, bypass .gitignore rules and index all files including build artifacts.
    #[serde(default)]
    pub include_ignored: bool,
}

fn default_workflows_directory() -> String {
    ".clido/workflows".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigFile {
    #[serde(default = "default_default_profile")]
    pub default_profile: String,
    #[serde(default)]
    pub profile: HashMap<String, ProfileEntry>,
    #[serde(default)]
    pub agent: AgentSection,
    #[serde(default)]
    pub tools: ToolsSection,
    #[serde(default)]
    pub context: ContextSection,
    #[serde(default)]
    pub workflows: WorkflowsSection,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub index: IndexSection,
    #[serde(default)]
    pub roles: RolesSection,
}

fn default_default_profile() -> String {
    "default".to_string()
}

/// Merged config from all sources (for use by CLI).
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub default_profile: String,
    pub profiles: HashMap<String, ProfileEntry>,
    pub agent: AgentSection,
    pub tools: ToolsSection,
    pub context: ContextSection,
    pub workflows: WorkflowsSection,
    pub hooks: HooksConfig,
    pub index: IndexSection,
    pub roles: RolesSection,
}

impl LoadedConfig {
    /// Resolve profile by name. Returns error if profile not found or provider unknown.
    pub fn get_profile(&self, name: &str) -> Result<&ProfileEntry> {
        self.profiles.get(name).ok_or_else(|| {
            ClidoError::Config(format!(
                "Profile '{}' not found. Check default_profile in config.",
                name
            ))
        })
    }

    /// Validate provider name.
    pub fn validate_provider(provider: &str) -> Result<()> {
        let valid = ["anthropic", "openrouter", "local"];
        if valid.contains(&provider) {
            Ok(())
        } else {
            Err(ClidoError::Config(format!(
                "Unknown provider '{}'. Valid: {}.",
                provider,
                valid.join(", ")
            )))
        }
    }
}

/// Path to the global config file (CLIDO_CONFIG or platform config dir). Used for first-run detection.
pub fn global_config_path() -> Option<PathBuf> {
    if let Ok(path_str) = std::env::var("CLIDO_CONFIG") {
        let p = PathBuf::from(&path_str);
        return Some(if p.is_absolute() {
            p
        } else {
            std::env::current_dir().ok()?.join(path_str)
        });
    }
    directories::ProjectDirs::from("", "", "clido")
        .map(|d: directories::ProjectDirs| d.config_dir().join("config.toml"))
}

/// True if any config file exists (global or project). Used to decide first-run vs normal run.
pub fn config_file_exists(cwd: &Path) -> bool {
    if global_config_path().map(|p| p.exists()).unwrap_or(false) {
        return true;
    }
    find_project_config(cwd).is_some()
}

fn find_project_config(cwd: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    let mut dir = cwd.to_path_buf();
    loop {
        let candidate = dir.join(".clido").join("config.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if let Some(ref h) = home {
            if dir == *h {
                break;
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn load_toml(path: &Path) -> Result<ConfigFile> {
    let s = std::fs::read_to_string(path).map_err(|e| {
        ClidoError::Config(format!("Failed to read config {}: {}", path.display(), e))
    })?;
    toml::from_str(&s)
        .map_err(|e| ClidoError::Config(format!("Invalid config {}: {}", path.display(), e)))
}

/// Merge two config files; `later` overrides `base` (shallow for profile tables).
fn merge(base: ConfigFile, later: ConfigFile) -> ConfigFile {
    let default_profile = later.default_profile.clone();
    let mut profile = base.profile;
    for (k, v) in later.profile {
        profile.insert(k, v);
    }
    let agent = AgentSection {
        max_turns: later.agent.max_turns,
        max_budget_usd: later.agent.max_budget_usd.or(base.agent.max_budget_usd),
        max_concurrent_tools: later
            .agent
            .max_concurrent_tools
            .or(base.agent.max_concurrent_tools),
        quiet: later.agent.quiet || base.agent.quiet,
        no_rules: later.agent.no_rules || base.agent.no_rules,
        rules_file: later.agent.rules_file.or(base.agent.rules_file),
        notify: later.agent.notify || base.agent.notify,
        auto_checkpoint: later.agent.auto_checkpoint,
        max_checkpoints_per_session: if later.agent.max_checkpoints_per_session != 50 {
            later.agent.max_checkpoints_per_session
        } else {
            base.agent.max_checkpoints_per_session
        },
    };
    let tools = ToolsSection {
        allowed: if later.tools.allowed.is_empty() {
            base.tools.allowed
        } else {
            later.tools.allowed
        },
        disallowed: if later.tools.disallowed.is_empty() {
            base.tools.disallowed
        } else {
            later.tools.disallowed
        },
    };
    let context = ContextSection {
        compaction_threshold: later.context.compaction_threshold,
        max_context_tokens: later
            .context
            .max_context_tokens
            .or(base.context.max_context_tokens),
    };
    let workflows = WorkflowsSection {
        directory: if later.workflows.directory != default_workflows_directory() {
            later.workflows.directory
        } else {
            base.workflows.directory
        },
    };
    let hooks = HooksConfig {
        pre_tool_use: later.hooks.pre_tool_use.or(base.hooks.pre_tool_use),
        post_tool_use: later.hooks.post_tool_use.or(base.hooks.post_tool_use),
    };
    let index = IndexSection {
        exclude_patterns: if later.index.exclude_patterns.is_empty() {
            base.index.exclude_patterns
        } else {
            later.index.exclude_patterns
        },
        include_ignored: later.index.include_ignored || base.index.include_ignored,
    };
    ConfigFile {
        default_profile,
        profile,
        agent,
        tools,
        context,
        workflows,
        hooks,
        index,
        roles: later.roles,
    }
}

/// Load config: CLIDO_CONFIG (if set) or global then project. Returns defaults if no files.
pub fn load_config(cwd: &Path) -> Result<LoadedConfig> {
    let mut merged = ConfigFile {
        default_profile: default_default_profile(),
        profile: HashMap::new(),
        agent: AgentSection::default(),
        tools: ToolsSection::default(),
        context: ContextSection::default(),
        workflows: WorkflowsSection::default(),
        hooks: HooksConfig::default(),
        index: IndexSection::default(),
        roles: RolesSection::default(),
    };

    if let Some(path) = global_config_path() {
        if path.exists() {
            merged = load_toml(&path)?;
        }
    }

    if let Some(path) = find_project_config(cwd) {
        let proj = load_toml(&path)?;
        merged = merge(merged, proj);
    }

    if merged.profile.is_empty() {
        merged.profile.insert(
            "default".to_string(),
            ProfileEntry {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet-20241022".to_string(),
                api_key: None,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                base_url: None,
            },
        );
        merged.default_profile = "default".to_string();
    }

    Ok(LoadedConfig {
        default_profile: merged.default_profile.clone(),
        profiles: merged.profile,
        agent: merged.agent,
        tools: merged.tools,
        context: merged.context,
        workflows: merged.workflows,
        hooks: merged.hooks,
        index: merged.index,
        roles: merged.roles,
    })
}

/// Build AgentConfig from LoadedConfig, profile name, and CLI overrides.
#[allow(clippy::too_many_arguments)]
pub fn agent_config_from_loaded(
    loaded: &LoadedConfig,
    profile_name: &str,
    cli_max_turns: Option<u32>,
    cli_max_budget_usd: Option<f64>,
    cli_model: Option<String>,
    cli_system_prompt: Option<String>,
    cli_permission_mode: Option<PermissionMode>,
    cli_quiet: bool,
    cli_max_parallel_tools: Option<u32>,
) -> Result<AgentConfig> {
    let profile = loaded.get_profile(profile_name)?;
    LoadedConfig::validate_provider(&profile.provider)?;
    let model = cli_model.clone().unwrap_or_else(|| profile.model.clone());
    // Config key is `max-concurrent-tools`; CLI flag is `--max-parallel-tools`.
    // Both refer to the same bounded-concurrency cap. CLI wins when provided.
    let max_parallel_tools = cli_max_parallel_tools
        .or(loaded.agent.max_concurrent_tools)
        .unwrap_or(4);
    Ok(AgentConfig {
        max_turns: cli_max_turns.unwrap_or(loaded.agent.max_turns),
        max_budget_usd: cli_max_budget_usd.or(loaded.agent.max_budget_usd),
        model: model.clone(),
        system_prompt: cli_system_prompt,
        permission_mode: cli_permission_mode.unwrap_or_default(),
        use_planner: false,
        use_index: false,
        max_context_tokens: loaded.context.max_context_tokens,
        compaction_threshold: Some(loaded.context.compaction_threshold),
        quiet: cli_quiet || loaded.agent.quiet,
        max_parallel_tools,
        no_rules: loaded.agent.no_rules,
        rules_file: loaded.agent.rules_file.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_config_returns_config_with_default_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let loaded = load_config(cwd).unwrap();
        assert!(!loaded.profiles.is_empty());
        assert!(loaded.profiles.contains_key("default"));
    }

    #[test]
    fn validate_provider_rejects_unknown() {
        assert!(LoadedConfig::validate_provider("unknown").is_err());
        assert!(LoadedConfig::validate_provider("anthropic").is_ok());
    }

    #[test]
    fn config_file_exists_true_when_project_config_present() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let clido_dir = cwd.join(".clido");
        std::fs::create_dir_all(&clido_dir).unwrap();
        std::fs::write(clido_dir.join("config.toml"), "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"x\"\n").unwrap();
        assert!(config_file_exists(cwd));
    }
}
