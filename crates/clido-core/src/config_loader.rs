//! Load and merge config.toml from CLIDO_CONFIG, global, and project paths.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::{AgentConfig, HooksConfig, PermissionMode};
use crate::{ClidoError, Result};

/// Write `contents` to `path` atomically: write to a `.tmp` sibling, then rename.
/// Prevents partial writes from corrupting the config on crash or I/O error.
fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)
        .map_err(|e| ClidoError::Config(format!("Cannot write tmp config: {}", e)))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| ClidoError::Config(format!("Cannot rename tmp config: {}", e)))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub provider: String,
    pub model: String,
    /// API key stored directly in config (takes priority over api_key_env).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Name of the environment variable that holds the API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// HTTP User-Agent override for API requests. Defaults to `"clido/<version>"`.
    /// Some providers (e.g. Kimi Code) restrict access by User-Agent — set this to
    /// a compatible client string such as `"RooCode/3.0.0"` to gain access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Per-profile worker sub-agent slot. Overrides global [agents.worker].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<crate::config::AgentSlotConfig>,
    /// Per-profile reviewer sub-agent slot. Overrides global [agents.reviewer].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<crate::config::AgentSlotConfig>,
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
    /// Maximum tokens the model may produce per response. None = provider default (8192).
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
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
            max_output_tokens: None,
        }
    }
}

fn default_max_turns() -> u32 {
    200
}
fn default_max_budget() -> Option<f64> {
    None
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
    /// Accepts both `default-profile` (canonical) and legacy `default_profile` (underscore).
    #[serde(default = "default_default_profile", alias = "default_profile")]
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
    #[serde(default)]
    pub agents: crate::config::AgentsConfig,
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
    pub agents: crate::config::AgentsConfig,
}

impl LoadedConfig {
    /// Resolve the effective provider/model for an agent slot.
    /// Falls back: agents.main → profile.default
    pub fn effective_slot(&self, role: &str) -> Option<&crate::config::AgentSlotConfig> {
        match role {
            "worker" => self.agents.worker.as_ref().or(self.agents.main.as_ref()),
            "reviewer" => self.agents.reviewer.as_ref().or(self.agents.main.as_ref()),
            _ => self.agents.main.as_ref(),
        }
    }

    /// Resolve the effective slot for a named profile's role.
    /// Per-profile slots take priority over global [agents.*] slots.
    pub fn effective_slot_for_profile(
        &self,
        role: &str,
        profile_name: &str,
    ) -> Option<&crate::config::AgentSlotConfig> {
        if let Some(profile) = self.profiles.get(profile_name) {
            match role {
                "worker" => {
                    if let Some(ref w) = profile.worker {
                        return Some(w);
                    }
                }
                "reviewer" => {
                    if let Some(ref r) = profile.reviewer {
                        return Some(r);
                    }
                }
                _ => {}
            }
        }
        self.effective_slot(role)
    }

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
        let valid = [
            "anthropic",
            "openrouter",
            "openai",
            "mistral",
            "minimax",
            "kimi",
            "kimi-code",
            "local",
            "alibabacloud",
            "deepseek",
            "groq",
            "cerebras",
            "togetherai",
            "fireworks",
            "xai",
            "perplexity",
            "gemini",
        ];
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
        max_output_tokens: later
            .agent
            .max_output_tokens
            .or(base.agent.max_output_tokens),
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
    let agents = crate::config::AgentsConfig {
        main: later.agents.main.or(base.agents.main),
        worker: later.agents.worker.or(base.agents.worker),
        reviewer: later.agents.reviewer.or(base.agents.reviewer),
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
        agents,
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
        agents: crate::config::AgentsConfig::default(),
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
        return Err(ClidoError::Config(
            "No provider profile found. Run 'clido init' to set up a provider.".into(),
        ));
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
        agents: merged.agents,
    })
}

/// Switch the active profile by writing `default_profile = "<name>"` to the config file.
/// The file is read, mutated via `toml::Value`, and written back atomically.
pub fn switch_active_profile(config_path: &Path, name: &str) -> Result<()> {
    let src = std::fs::read_to_string(config_path).map_err(|e| {
        ClidoError::Config(format!(
            "Cannot read config {}: {}",
            config_path.display(),
            e
        ))
    })?;
    let mut root: toml::Value =
        toml::from_str(&src).map_err(|e| ClidoError::Config(format!("Invalid config: {}", e)))?;
    if let toml::Value::Table(ref mut t) = root {
        // Preserve the existing key format (underscore legacy or hyphen canonical).
        let key = if t.contains_key("default_profile") {
            "default_profile"
        } else {
            "default-profile"
        };
        t.insert(key.to_string(), toml::Value::String(name.to_string()));
    }
    let out = toml::to_string_pretty(&root)
        .map_err(|e| ClidoError::Config(format!("Serialize error: {}", e)))?;
    atomic_write(config_path, &out)?;
    Ok(())
}

/// Remove a named profile (and its sub-tables) from the config file.
/// Returns an error if the profile is currently the active default.
pub fn delete_profile_from_config(config_path: &Path, name: &str) -> Result<()> {
    let src = std::fs::read_to_string(config_path).map_err(|e| {
        ClidoError::Config(format!(
            "Cannot read config {}: {}",
            config_path.display(),
            e
        ))
    })?;
    let mut root: toml::Value =
        toml::from_str(&src).map_err(|e| ClidoError::Config(format!("Invalid config: {}", e)))?;
    // Guard: cannot delete the active default profile.
    if let toml::Value::Table(ref t) = root {
        if let Some(toml::Value::String(default)) = t.get("default_profile") {
            if default == name {
                return Err(ClidoError::Config(format!(
                    "Cannot delete the active profile '{}'. Switch to another profile first.",
                    name
                )));
            }
        }
    }
    if let toml::Value::Table(ref mut t) = root {
        if let Some(toml::Value::Table(ref mut profiles)) = t.get_mut("profile") {
            profiles.remove(name);
        }
    }
    let out = toml::to_string_pretty(&root)
        .map_err(|e| ClidoError::Config(format!("Serialize error: {}", e)))?;
    atomic_write(config_path, &out)?;
    Ok(())
}

/// Insert or replace a named profile in the config file.
/// `entry` is serialized and written under `[profile.<name>]`.
pub fn upsert_profile_in_config(
    config_path: &Path,
    name: &str,
    entry: &ProfileEntry,
) -> Result<()> {
    // Read existing config (or start empty if not found).
    let src = if config_path.exists() {
        std::fs::read_to_string(config_path).map_err(|e| {
            ClidoError::Config(format!(
                "Cannot read config {}: {}",
                config_path.display(),
                e
            ))
        })?
    } else {
        String::new()
    };
    let mut root: toml::Value = if src.is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&src).map_err(|e| ClidoError::Config(format!("Invalid config: {}", e)))?
    };

    let entry_value = toml::Value::try_from(entry)
        .map_err(|e| ClidoError::Config(format!("Serialize profile: {}", e)))?;

    if let toml::Value::Table(ref mut root_table) = root {
        let profiles = root_table
            .entry("profile".to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if let toml::Value::Table(ref mut pt) = profiles {
            pt.insert(name.to_string(), entry_value);
        }
        // If there's no default_profile yet, set this as the default.
        if !root_table.contains_key("default_profile") {
            root_table.insert(
                "default_profile".to_string(),
                toml::Value::String(name.to_string()),
            );
        }
    }

    let out = toml::to_string_pretty(&root)
        .map_err(|e| ClidoError::Config(format!("Serialize error: {}", e)))?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ClidoError::Config(format!("Cannot create config dir: {}", e)))?;
    }
    atomic_write(config_path, &out)?;
    Ok(())
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
        permission_rules: Vec::new(),
        use_planner: false,
        use_index: false,
        max_context_tokens: loaded.context.max_context_tokens,
        compaction_threshold: Some(loaded.context.compaction_threshold),
        quiet: cli_quiet || loaded.agent.quiet,
        max_parallel_tools,
        no_rules: loaded.agent.no_rules,
        rules_file: loaded.agent.rules_file.clone(),
        max_output_tokens: loaded.agent.max_output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn load_config_errors_when_no_profiles_in_config() {
        let temp = tempfile::tempdir().unwrap();
        // Point CLIDO_CONFIG at an empty config (no profiles) so we bypass the user's global config.
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(&cfg_path, "default_profile = \"default\"\n").unwrap();
        // SAFETY: only safe in single-threaded test contexts; cargo test runs each #[test] fn
        // in its own thread but env mutations can race. Acceptable for this unit test.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("CLIDO_CONFIG", &cfg_path);
        }
        let result = load_config(temp.path());
        unsafe {
            std::env::remove_var("CLIDO_CONFIG");
        }
        assert!(result.is_err(), "expected Err but got Ok");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("clido init"), "expected init hint in: {}", msg);
    }

    #[test]
    fn load_config_returns_profile_from_config_file() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let clido_dir = cwd.join(".clido");
        std::fs::create_dir_all(&clido_dir).unwrap();
        std::fs::write(
            clido_dir.join("config.toml"),
            "default_profile = \"default\"\n[profile.default]\nprovider = \"openrouter\"\nmodel = \"anthropic/claude-3-5-sonnet\"\napi_key = \"sk-or-test\"\n",
        ).unwrap();
        let loaded = load_config(cwd).unwrap();
        assert!(loaded.profiles.contains_key("default"));
        assert_eq!(loaded.profiles["default"].provider, "openrouter");
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

    #[test]
    fn config_file_exists_false_when_no_config() {
        // Use a totally isolated temp dir with no parent project config
        let temp = tempfile::tempdir().unwrap();
        // Override CLIDO_CONFIG to a non-existent path so global config is skipped
        let nonexistent = temp.path().join("no_such_config.toml");
        unsafe { std::env::set_var("CLIDO_CONFIG", &nonexistent) };
        let result = config_file_exists(temp.path());
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        assert!(!result);
    }

    #[test]
    fn agent_section_default_values() {
        let s = AgentSection::default();
        assert_eq!(s.max_turns, 200);
        assert_eq!(s.max_budget_usd, None);
        assert!(!s.quiet);
        assert!(!s.no_rules);
        assert!(s.rules_file.is_none());
        assert!(!s.notify);
        assert!(s.auto_checkpoint);
        assert_eq!(s.max_checkpoints_per_session, 50);
        assert!(s.max_concurrent_tools.is_none());
    }

    #[test]
    fn tools_section_default_is_empty() {
        let t = ToolsSection::default();
        assert!(t.allowed.is_empty());
        assert!(t.disallowed.is_empty());
    }

    #[test]
    fn context_section_default_values() {
        let c = ContextSection::default();
        assert!((c.compaction_threshold - 0.75).abs() < f64::EPSILON);
        assert!(c.max_context_tokens.is_none());
    }

    #[test]
    fn roles_section_as_map_includes_all_roles() {
        let mut r = RolesSection::default();
        r.fast = Some("fast-model".to_string());
        r.reasoning = Some("reasoning-model".to_string());
        r.critic = Some("critic-model".to_string());
        r.planner = Some("planner-model".to_string());
        r.extra
            .insert("custom".to_string(), "custom-model".to_string());

        let map = r.as_map();
        assert_eq!(map.get("fast").map(|s| s.as_str()), Some("fast-model"));
        assert_eq!(
            map.get("reasoning").map(|s| s.as_str()),
            Some("reasoning-model")
        );
        assert_eq!(map.get("critic").map(|s| s.as_str()), Some("critic-model"));
        assert_eq!(
            map.get("planner").map(|s| s.as_str()),
            Some("planner-model")
        );
        assert_eq!(map.get("custom").map(|s| s.as_str()), Some("custom-model"));
    }

    #[test]
    fn roles_section_as_map_empty_when_all_none() {
        let r = RolesSection::default();
        let map = r.as_map();
        assert!(map.is_empty());
    }

    #[test]
    fn get_profile_returns_error_for_missing_profile() {
        let loaded = LoadedConfig {
            default_profile: "default".to_string(),
            profiles: std::collections::HashMap::new(),
            agent: AgentSection::default(),
            tools: ToolsSection::default(),
            context: ContextSection::default(),
            workflows: WorkflowsSection::default(),
            hooks: crate::config::HooksConfig::default(),
            index: IndexSection::default(),
            roles: RolesSection::default(),
            agents: crate::config::AgentsConfig::default(),
        };
        let result = loaded.get_profile("nonexistent");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"));
    }

    #[test]
    fn validate_provider_accepts_all_valid_providers() {
        for p in &[
            "anthropic",
            "openrouter",
            "openai",
            "mistral",
            "minimax",
            "local",
            "alibabacloud",
            "deepseek",
            "groq",
            "cerebras",
            "togetherai",
            "fireworks",
            "xai",
            "perplexity",
            "gemini",
        ] {
            assert!(
                LoadedConfig::validate_provider(p).is_ok(),
                "should accept {}",
                p
            );
        }
    }

    #[test]
    fn merge_later_profile_overrides_base() {
        let temp = tempfile::tempdir().unwrap();
        // Create global config
        let global_cfg_path = temp.path().join("global.toml");
        std::fs::write(
            &global_cfg_path,
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"old-model\"\napi_key = \"old-key\"\n",
        ).unwrap();
        // Create project config that overrides model
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(cwd.join(".clido")).unwrap();
        std::fs::write(
            cwd.join(".clido").join("config.toml"),
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"new-model\"\napi_key = \"new-key\"\n",
        ).unwrap();
        unsafe { std::env::set_var("CLIDO_CONFIG", &global_cfg_path) };
        let loaded = load_config(&cwd).unwrap();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        assert_eq!(loaded.profiles["default"].model, "new-model");
    }

    #[test]
    fn agent_config_from_loaded_uses_cli_overrides() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let clido_dir = cwd.join(".clido");
        std::fs::create_dir_all(&clido_dir).unwrap();
        std::fs::write(
            clido_dir.join("config.toml"),
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet\"\napi_key = \"sk-ant-test\"\n",
        ).unwrap();
        // Override CLIDO_CONFIG so we don't pick up the developer's global config
        let nonexistent = temp.path().join("no_global.toml");
        unsafe { std::env::set_var("CLIDO_CONFIG", &nonexistent) };
        let loaded = load_config(cwd).unwrap();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };

        let cfg = agent_config_from_loaded(
            &loaded,
            "default",
            Some(10),
            Some(1.0),
            Some("overridden-model".to_string()),
            Some("Custom system prompt".to_string()),
            Some(crate::config::PermissionMode::AcceptAll),
            true,
            Some(8),
        )
        .unwrap();
        assert_eq!(cfg.max_turns, 10);
        assert_eq!(cfg.max_budget_usd, Some(1.0));
        assert_eq!(cfg.model, "overridden-model");
        assert_eq!(cfg.system_prompt.as_deref(), Some("Custom system prompt"));
        assert_eq!(
            cfg.permission_mode,
            crate::config::PermissionMode::AcceptAll
        );
        assert!(cfg.quiet);
        assert_eq!(cfg.max_parallel_tools, 8);
    }

    #[test]
    fn agent_config_from_loaded_uses_config_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let clido_dir = cwd.join(".clido");
        std::fs::create_dir_all(&clido_dir).unwrap();
        std::fs::write(
            clido_dir.join("config.toml"),
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"claude-haiku\"\napi_key = \"sk-ant-x\"\n",
        ).unwrap();
        let nonexistent = temp.path().join("no_global2.toml");
        unsafe { std::env::set_var("CLIDO_CONFIG", &nonexistent) };
        let loaded = load_config(cwd).unwrap();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };

        let cfg = agent_config_from_loaded(
            &loaded, "default", None, None, None, None, None, false, None,
        )
        .unwrap();
        assert_eq!(cfg.model, "claude-haiku");
        assert_eq!(cfg.max_turns, 200);
        assert_eq!(cfg.max_parallel_tools, 4);
        assert_eq!(cfg.permission_mode, crate::config::PermissionMode::Default);
        assert!(!cfg.quiet);
    }

    #[test]
    fn agent_config_from_loaded_fails_on_unknown_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let clido_dir = cwd.join(".clido");
        std::fs::create_dir_all(&clido_dir).unwrap();
        std::fs::write(
            clido_dir.join("config.toml"),
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n",
        ).unwrap();
        let nonexistent = temp.path().join("no_global3.toml");
        unsafe { std::env::set_var("CLIDO_CONFIG", &nonexistent) };
        let loaded = load_config(cwd).unwrap();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };

        let result = agent_config_from_loaded(
            &loaded, "missing", None, None, None, None, None, false, None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn global_config_path_reads_env_var() {
        let temp = tempfile::tempdir().unwrap();
        let custom_path = temp.path().join("custom_config.toml");
        unsafe { std::env::set_var("CLIDO_CONFIG", &custom_path) };
        let path = global_config_path();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        assert!(path.is_some());
        assert!(path.unwrap().ends_with("custom_config.toml"));
    }

    /// Line 264: CLIDO_CONFIG with a relative path gets joined to cwd.
    #[test]
    fn global_config_path_relative_env_var_joins_cwd() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("CLIDO_CONFIG", "relative_config.toml") };
        let path = global_config_path();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(
            p.is_absolute(),
            "relative path should be made absolute via cwd join"
        );
        assert!(p.to_string_lossy().ends_with("relative_config.toml"));
    }

    /// Lines 79-80, 82-83: default_true and default_max_checkpoints are used when
    /// deserializing TOML that doesn't specify those fields.
    #[test]
    fn agent_section_defaults_true_and_max_checkpoints() {
        let toml_str = r#"
[agent]
max-turns = 100
"#;
        let cf: ConfigFile = toml::from_str(toml_str).unwrap();
        // default_true is used for auto_checkpoint; default_max_checkpoints is 50
        assert_eq!(cf.agent.max_checkpoints_per_session, 50);
        assert!(
            cf.agent.auto_checkpoint,
            "default_true should give auto_checkpoint=true"
        );
    }

    /// Line 301: load_toml error path — file doesn't exist.
    #[test]
    fn load_toml_nonexistent_returns_error() {
        let result = load_toml(std::path::Path::new("/nonexistent/config.toml"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Failed to read config"), "got: {}", msg);
    }

    // ── AgentSlotConfig / AgentsConfig deserialization ────────────────────

    #[test]
    fn agent_slot_config_roundtrips_toml() {
        use crate::config::AgentSlotConfig;
        let slot = AgentSlotConfig {
            provider: "anthropic".to_string(),
            model: "claude-opus-4-6".to_string(),
            api_key: Some("sk-ant-test".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
        };
        let serialized = toml::to_string(&slot).unwrap();
        let deserialized: AgentSlotConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.provider, "anthropic");
        assert_eq!(deserialized.model, "claude-opus-4-6");
        assert_eq!(deserialized.api_key.as_deref(), Some("sk-ant-test"));
    }

    #[test]
    fn agents_config_default_has_no_slots() {
        use crate::config::AgentsConfig;
        let cfg = AgentsConfig::default();
        assert!(cfg.main.is_none());
        assert!(cfg.worker.is_none());
        assert!(cfg.reviewer.is_none());
    }

    #[test]
    fn agents_config_parsed_from_toml() {
        let toml_str = r#"
[agents.main]
provider = "anthropic"
model = "claude-sonnet-4-5"
api_key = "sk-ant-main"

[agents.worker]
provider = "openai"
model = "gpt-4o-mini"
api_key = "sk-openai-worker"
"#;
        let cf: ConfigFile = toml::from_str(toml_str).unwrap();
        let main = cf.agents.main.expect("main slot should be present");
        assert_eq!(main.provider, "anthropic");
        assert_eq!(main.model, "claude-sonnet-4-5");
        assert_eq!(main.api_key.as_deref(), Some("sk-ant-main"));
        let worker = cf.agents.worker.expect("worker slot should be present");
        assert_eq!(worker.provider, "openai");
        assert!(cf.agents.reviewer.is_none());
    }

    // ── effective_slot ────────────────────────────────────────────────────

    fn make_loaded_with_agents(
        main: Option<crate::config::AgentSlotConfig>,
        worker: Option<crate::config::AgentSlotConfig>,
        reviewer: Option<crate::config::AgentSlotConfig>,
    ) -> LoadedConfig {
        let mut profiles = std::collections::HashMap::new();
        profiles.insert(
            "default".to_string(),
            crate::ProfileEntry {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4-5".to_string(),
                api_key: Some("sk-ant".to_string()),
                api_key_env: None,
                base_url: None,
                user_agent: None,
                worker: None,
                reviewer: None,
            },
        );
        LoadedConfig {
            default_profile: "default".to_string(),
            profiles,
            agent: AgentSection::default(),
            tools: ToolsSection::default(),
            context: ContextSection::default(),
            workflows: WorkflowsSection::default(),
            hooks: crate::config::HooksConfig::default(),
            index: IndexSection::default(),
            roles: RolesSection::default(),
            agents: crate::config::AgentsConfig {
                main,
                worker,
                reviewer,
            },
        }
    }

    fn make_slot(provider: &str, model: &str) -> crate::config::AgentSlotConfig {
        crate::config::AgentSlotConfig {
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: None,
            api_key_env: None,
            base_url: None,
            user_agent: None,
        }
    }

    #[test]
    fn effective_slot_main_returns_main_when_set() {
        let loaded =
            make_loaded_with_agents(Some(make_slot("anthropic", "claude-opus-4-6")), None, None);
        let slot = loaded
            .effective_slot("main")
            .expect("should have main slot");
        assert_eq!(slot.model, "claude-opus-4-6");
    }

    #[test]
    fn effective_slot_worker_returns_worker_when_set() {
        let loaded = make_loaded_with_agents(
            Some(make_slot("anthropic", "main-model")),
            Some(make_slot("openai", "worker-model")),
            None,
        );
        let slot = loaded
            .effective_slot("worker")
            .expect("should have worker slot");
        assert_eq!(slot.model, "worker-model");
    }

    #[test]
    fn effective_slot_worker_falls_back_to_main() {
        let loaded =
            make_loaded_with_agents(Some(make_slot("anthropic", "main-model")), None, None);
        // No worker → falls back to main
        let slot = loaded
            .effective_slot("worker")
            .expect("should fall back to main");
        assert_eq!(slot.model, "main-model");
    }

    #[test]
    fn effective_slot_reviewer_returns_reviewer_when_set() {
        let loaded = make_loaded_with_agents(
            Some(make_slot("anthropic", "main-model")),
            None,
            Some(make_slot("anthropic", "reviewer-model")),
        );
        let slot = loaded
            .effective_slot("reviewer")
            .expect("should have reviewer slot");
        assert_eq!(slot.model, "reviewer-model");
    }

    #[test]
    fn effective_slot_reviewer_falls_back_to_main() {
        let loaded =
            make_loaded_with_agents(Some(make_slot("anthropic", "main-model")), None, None);
        let slot = loaded
            .effective_slot("reviewer")
            .expect("should fall back to main");
        assert_eq!(slot.model, "main-model");
    }

    #[test]
    fn effective_slot_returns_none_when_no_main_configured() {
        let loaded = make_loaded_with_agents(None, None, None);
        assert!(loaded.effective_slot("main").is_none());
        assert!(loaded.effective_slot("worker").is_none());
        assert!(loaded.effective_slot("reviewer").is_none());
    }

    #[test]
    fn effective_slot_unknown_role_returns_main() {
        let loaded =
            make_loaded_with_agents(Some(make_slot("anthropic", "main-model")), None, None);
        let slot = loaded
            .effective_slot("unknown_role")
            .expect("unknown role returns main");
        assert_eq!(slot.model, "main-model");
    }

    // ── effective_slot_for_profile ────────────────────────────────────────

    fn make_profile_entry_with_worker(
        worker: Option<crate::config::AgentSlotConfig>,
        reviewer: Option<crate::config::AgentSlotConfig>,
    ) -> crate::ProfileEntry {
        crate::ProfileEntry {
            provider: "anthropic".to_string(),
            model: "main-model".to_string(),
            api_key: None,
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker,
            reviewer,
        }
    }

    #[test]
    fn effective_slot_for_profile_per_profile_worker_overrides_global() {
        let mut profiles = std::collections::HashMap::new();
        let per_profile_worker = make_slot("openai", "per-profile-worker");
        profiles.insert(
            "work".to_string(),
            make_profile_entry_with_worker(Some(per_profile_worker), None),
        );
        let loaded = LoadedConfig {
            default_profile: "work".to_string(),
            profiles,
            agent: AgentSection::default(),
            tools: ToolsSection::default(),
            context: ContextSection::default(),
            workflows: WorkflowsSection::default(),
            hooks: crate::config::HooksConfig::default(),
            index: IndexSection::default(),
            roles: RolesSection::default(),
            agents: crate::config::AgentsConfig {
                main: Some(make_slot("anthropic", "global-main")),
                worker: Some(make_slot("anthropic", "global-worker")),
                reviewer: None,
            },
        };
        // Per-profile worker takes priority over global worker
        let slot = loaded.effective_slot_for_profile("worker", "work").unwrap();
        assert_eq!(slot.model, "per-profile-worker");
    }

    #[test]
    fn effective_slot_for_profile_falls_back_to_global_when_no_per_profile_slot() {
        let mut profiles = std::collections::HashMap::new();
        profiles.insert(
            "work".to_string(),
            make_profile_entry_with_worker(None, None),
        );
        let loaded = LoadedConfig {
            default_profile: "work".to_string(),
            profiles,
            agent: AgentSection::default(),
            tools: ToolsSection::default(),
            context: ContextSection::default(),
            workflows: WorkflowsSection::default(),
            hooks: crate::config::HooksConfig::default(),
            index: IndexSection::default(),
            roles: RolesSection::default(),
            agents: crate::config::AgentsConfig {
                main: Some(make_slot("anthropic", "global-main")),
                worker: Some(make_slot("anthropic", "global-worker")),
                reviewer: None,
            },
        };
        // No per-profile worker → falls back to global worker
        let slot = loaded.effective_slot_for_profile("worker", "work").unwrap();
        assert_eq!(slot.model, "global-worker");
    }

    #[test]
    fn effective_slot_for_profile_unknown_profile_falls_back_to_global() {
        let loaded = make_loaded_with_agents(
            Some(make_slot("anthropic", "global-main")),
            Some(make_slot("anthropic", "global-worker")),
            None,
        );
        // "nonexistent" profile → falls back to global worker
        let slot = loaded
            .effective_slot_for_profile("worker", "nonexistent")
            .unwrap();
        assert_eq!(slot.model, "global-worker");
    }

    // ── switch_active_profile ─────────────────────────────────────────────

    #[test]
    fn switch_active_profile_updates_default_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n[profile.work]\nprovider = \"openai\"\nmodel = \"gpt-4o\"\napi_key = \"sk\"\n",
        ).unwrap();
        switch_active_profile(&cfg_path, "work").unwrap();
        let src = std::fs::read_to_string(&cfg_path).unwrap();
        let cf: ConfigFile = toml::from_str(&src).unwrap();
        assert_eq!(cf.default_profile, "work");
    }

    // ── delete_profile_from_config ────────────────────────────────────────

    #[test]
    fn delete_profile_from_config_removes_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n[profile.old]\nprovider = \"openai\"\nmodel = \"gpt-4o\"\napi_key = \"sk\"\n",
        ).unwrap();
        delete_profile_from_config(&cfg_path, "old").unwrap();
        let src = std::fs::read_to_string(&cfg_path).unwrap();
        let cf: ConfigFile = toml::from_str(&src).unwrap();
        assert!(!cf.profile.contains_key("old"));
        assert!(cf.profile.contains_key("default"));
    }

    #[test]
    fn delete_profile_from_config_rejects_active_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n",
        ).unwrap();
        let result = delete_profile_from_config(&cfg_path, "default");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("active") || msg.contains("Cannot delete"),
            "got: {}",
            msg
        );
    }

    // ── upsert_profile_in_config ──────────────────────────────────────────

    #[test]
    fn upsert_profile_in_config_adds_new_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n",
        ).unwrap();
        let entry = crate::ProfileEntry {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            api_key: Some("sk-new".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        upsert_profile_in_config(&cfg_path, "work", &entry).unwrap();
        let src = std::fs::read_to_string(&cfg_path).unwrap();
        let cf: ConfigFile = toml::from_str(&src).unwrap();
        assert!(cf.profile.contains_key("work"));
        assert_eq!(cf.profile["work"].provider, "openai");
        assert_eq!(cf.profile["work"].model, "gpt-4o");
        // Default profile unchanged
        assert_eq!(cf.default_profile, "default");
    }

    #[test]
    fn upsert_profile_in_config_replaces_existing_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"old-model\"\napi_key = \"old-key\"\n",
        ).unwrap();
        let entry = crate::ProfileEntry {
            provider: "anthropic".to_string(),
            model: "new-model".to_string(),
            api_key: Some("new-key".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        upsert_profile_in_config(&cfg_path, "default", &entry).unwrap();
        let src = std::fs::read_to_string(&cfg_path).unwrap();
        let cf: ConfigFile = toml::from_str(&src).unwrap();
        assert_eq!(cf.profile["default"].model, "new-model");
    }

    // ── ProfileEntry with worker/reviewer serialization ───────────────────

    #[test]
    fn profile_entry_with_worker_toml_roundtrip() {
        let entry = crate::ProfileEntry {
            provider: "anthropic".to_string(),
            model: "claude-opus-4-6".to_string(),
            api_key: Some("sk-ant".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: Some(crate::config::AgentSlotConfig {
                provider: "openai".to_string(),
                model: "gpt-4o-mini".to_string(),
                api_key: Some("sk-openai".to_string()),
                api_key_env: None,
                base_url: None,
                user_agent: None,
            }),
            reviewer: None,
        };
        let serialized = toml::to_string(&entry).unwrap();
        let deserialized: crate::ProfileEntry = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.model, "claude-opus-4-6");
        let w = deserialized.worker.expect("worker should round-trip");
        assert_eq!(w.provider, "openai");
        assert_eq!(w.model, "gpt-4o-mini");
    }

    #[test]
    fn profile_entry_without_worker_serializes_cleanly() {
        let entry = crate::ProfileEntry {
            provider: "anthropic".to_string(),
            model: "m".to_string(),
            api_key: Some("k".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        let s = toml::to_string(&entry).unwrap();
        // Optional None fields should not appear in the serialized output
        assert!(
            !s.contains("worker"),
            "worker=None should not appear: {}",
            s
        );
        assert!(
            !s.contains("reviewer"),
            "reviewer=None should not appear: {}",
            s
        );
        assert!(
            !s.contains("api_key_env"),
            "api_key_env=None should not appear: {}",
            s
        );
    }

    // ── validate_provider exhaustive ─────────────────────────────────────────

    #[test]
    fn validate_provider_accepts_all_known_providers() {
        for p in &[
            "anthropic",
            "openrouter",
            "openai",
            "mistral",
            "minimax",
            "kimi",
            "kimi-code",
            "local",
            "alibabacloud",
            "deepseek",
            "groq",
            "cerebras",
            "togetherai",
            "fireworks",
            "xai",
            "perplexity",
            "gemini",
        ] {
            assert!(
                LoadedConfig::validate_provider(p).is_ok(),
                "expected Ok for '{}'",
                p
            );
        }
    }

    #[test]
    fn validate_provider_rejects_kimi_with_space() {
        assert!(LoadedConfig::validate_provider("kimi code").is_err());
        assert!(LoadedConfig::validate_provider("KIMI").is_err());
        assert!(LoadedConfig::validate_provider("moonshot").is_err());
    }

    // ── upsert_profile_in_config ─────────────────────────────────────────────

    #[test]
    fn upsert_profile_creates_new_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = temp.path().join("config.toml");
        let entry = crate::ProfileEntry {
            provider: "kimi".to_string(),
            model: "moonshot-v1-32k".to_string(),
            api_key: Some("sk-test".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        upsert_profile_in_config(&cfg, "kimi-profile", &entry).unwrap();
        let content = std::fs::read_to_string(&cfg).unwrap();
        assert!(content.contains("kimi-profile"));
        assert!(content.contains("moonshot-v1-32k"));
    }

    #[test]
    fn upsert_profile_sets_default_profile_when_absent() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = temp.path().join("config.toml");
        let entry = crate::ProfileEntry {
            provider: "kimi-code".to_string(),
            model: "kimi-for-coding".to_string(),
            api_key: Some("sk-kimi-code".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            worker: None,
            reviewer: None,
        };
        upsert_profile_in_config(&cfg, "coding", &entry).unwrap();
        let content = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            content.contains("default_profile"),
            "should set default_profile: {}",
            content
        );
    }
}
