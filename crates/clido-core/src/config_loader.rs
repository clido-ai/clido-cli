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
    /// Optional fast/cheap provider for utility tasks (summarization, title, commit, sub-agents).
    /// If not set, the main provider is used for everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<crate::config::FastProviderConfig>,
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
    /// Enable harness mode: structured `.clido/harness/` tasks + `HarnessControl` tool + strict protocol.
    #[serde(default)]
    pub harness: bool,
    /// Wall-clock seconds for one user turn (0 = unlimited).
    #[serde(default)]
    pub max_wall_time_per_turn_sec: Option<u64>,
    #[serde(default)]
    pub max_tool_calls_per_turn: Option<u32>,
    #[serde(default)]
    pub stall_threshold: Option<u32>,
    #[serde(default)]
    pub doom_consecutive_same_error: Option<usize>,
    #[serde(default)]
    pub doom_same_args_window: Option<usize>,
    #[serde(default)]
    pub doom_same_args_min: Option<usize>,
    /// Alias: `tool-retries` for backward compatibility.
    #[serde(default, alias = "tool-retries")]
    pub max_tool_retries: Option<u32>,
    #[serde(default)]
    pub retry_backoff_max_ms: Option<u64>,
    #[serde(default)]
    pub retry_jitter_numerator: Option<u8>,
    #[serde(default)]
    pub provider_min_request_interval_ms: Option<u32>,
    #[serde(default)]
    pub stream_model_completion: bool,
    #[serde(default)]
    pub tool_timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_tool_output_bytes: Option<usize>,
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
            harness: false,
            max_wall_time_per_turn_sec: None,
            max_tool_calls_per_turn: None,
            stall_threshold: None,
            doom_consecutive_same_error: None,
            doom_same_args_window: None,
            doom_same_args_min: None,
            max_tool_retries: None,
            retry_backoff_max_ms: None,
            retry_jitter_numerator: None,
            provider_min_request_interval_ms: None,
            stream_model_completion: false,
            tool_timeout_secs: None,
            max_tool_output_bytes: None,
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
    0.58
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

/// Legacy `[roles]` section — kept only for backwards-compatible parsing.
/// Ignored at runtime; use `[profiles.<name>.fast]` instead.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RolesSection {
    #[serde(default)]
    pub fast: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, String>,
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

/// User preferences for Skills discovery and activation (`clido_core::skills`).
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct SkillsSection {
    /// Skill ids to hide from the agent (even if present on disk).
    #[serde(default)]
    pub disabled: Vec<String>,
    /// If non-empty, only these ids are active (whitelist). Off-disk ids are ignored.
    #[serde(default)]
    pub enabled: Vec<String>,
    /// Extra directories to scan (absolute or relative to workspace root). `~/` expanded.
    #[serde(default)]
    pub extra_paths: Vec<String>,
    /// When true, skip all skill injection.
    #[serde(default)]
    pub no_skills: bool,
    /// When true (default), the system prompt encourages suggesting matching skills.
    #[serde(default)]
    pub auto_suggest: Option<bool>,
    /// Reserved for remote registries / marketplace (not fetched in this version).
    #[serde(default)]
    pub registry_urls: Vec<String>,
}

fn merge_string_vecs_union(a: Vec<String>, b: Vec<String>) -> Vec<String> {
    let mut v: Vec<String> = a.into_iter().chain(b).collect();
    v.sort();
    v.dedup();
    v
}

fn merge_skills_section(base: &SkillsSection, later: &SkillsSection) -> SkillsSection {
    SkillsSection {
        disabled: merge_string_vecs_union(base.disabled.clone(), later.disabled.clone()),
        enabled: if !later.enabled.is_empty() {
            later.enabled.clone()
        } else {
            base.enabled.clone()
        },
        extra_paths: merge_string_vecs_union(base.extra_paths.clone(), later.extra_paths.clone()),
        no_skills: base.no_skills || later.no_skills,
        auto_suggest: later.auto_suggest.or(base.auto_suggest),
        registry_urls: merge_string_vecs_union(
            base.registry_urls.clone(),
            later.registry_urls.clone(),
        ),
    }
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
    pub skills: SkillsSection,
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
    pub skills: SkillsSection,
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

/// Path to the global config directory (parent of `global_config_path()`).
/// Returns `None` if the directory cannot be determined.
pub fn global_config_dir() -> Option<PathBuf> {
    global_config_path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
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
        harness: later.agent.harness || base.agent.harness,
        max_wall_time_per_turn_sec: later
            .agent
            .max_wall_time_per_turn_sec
            .or(base.agent.max_wall_time_per_turn_sec),
        max_tool_calls_per_turn: later
            .agent
            .max_tool_calls_per_turn
            .or(base.agent.max_tool_calls_per_turn),
        stall_threshold: later
            .agent
            .stall_threshold
            .or(base.agent.stall_threshold),
        doom_consecutive_same_error: later
            .agent
            .doom_consecutive_same_error
            .or(base.agent.doom_consecutive_same_error),
        doom_same_args_window: later
            .agent
            .doom_same_args_window
            .or(base.agent.doom_same_args_window),
        doom_same_args_min: later
            .agent
            .doom_same_args_min
            .or(base.agent.doom_same_args_min),
        max_tool_retries: later
            .agent
            .max_tool_retries
            .or(base.agent.max_tool_retries),
        retry_backoff_max_ms: later
            .agent
            .retry_backoff_max_ms
            .or(base.agent.retry_backoff_max_ms),
        retry_jitter_numerator: later
            .agent
            .retry_jitter_numerator
            .or(base.agent.retry_jitter_numerator),
        provider_min_request_interval_ms: later
            .agent
            .provider_min_request_interval_ms
            .or(base.agent.provider_min_request_interval_ms),
        stream_model_completion: later.agent.stream_model_completion || base.agent.stream_model_completion,
        tool_timeout_secs: later
            .agent
            .tool_timeout_secs
            .or(base.agent.tool_timeout_secs),
        max_tool_output_bytes: later
            .agent
            .max_tool_output_bytes
            .or(base.agent.max_tool_output_bytes),
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
    let skills = merge_skills_section(&base.skills, &later.skills);
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
        skills,
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
        skills: SkillsSection::default(),
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
        skills: merged.skills,
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

/// Update `<workspace>/.clido/config.toml` so `[skills].disabled` includes or excludes `skill_id`.
/// Creates the file and parent dirs if needed. Other keys are preserved.
pub fn set_skill_disabled_in_project(
    workspace_root: &Path,
    skill_id: &str,
    disabled: bool,
) -> Result<()> {
    let path = workspace_root.join(".clido").join("config.toml");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ClidoError::Config(format!("Cannot create {}: {}", parent.display(), e))
        })?;
    }
    let src = if path.exists() {
        std::fs::read_to_string(&path)
            .map_err(|e| ClidoError::Config(format!("Cannot read {}: {}", path.display(), e)))?
    } else {
        String::new()
    };
    let mut root: toml::Value = if src.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&src)
            .map_err(|e| ClidoError::Config(format!("Invalid config {}: {}", path.display(), e)))?
    };
    let table = root.as_table_mut().ok_or_else(|| {
        ClidoError::Config(format!("Config root must be a table: {}", path.display()))
    })?;
    let skills_val = table
        .entry("skills")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let skills_table = skills_val
        .as_table_mut()
        .ok_or_else(|| ClidoError::Config("[skills] must be a table".to_string()))?;
    let disabled_val = skills_table
        .entry("disabled")
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    let arr = disabled_val
        .as_array_mut()
        .ok_or_else(|| ClidoError::Config("[skills].disabled must be an array".to_string()))?;
    if disabled {
        if !arr.iter().any(|v| v.as_str() == Some(skill_id)) {
            arr.push(toml::Value::String(skill_id.to_string()));
        }
        if let Some(toml::Value::Array(en)) = skills_table.get_mut("enabled") {
            en.retain(|v| v.as_str() != Some(skill_id));
        }
    } else {
        arr.retain(|v| v.as_str() != Some(skill_id));
    }
    let out = toml::to_string_pretty(&root)
        .map_err(|e| ClidoError::Config(format!("Serialize error: {}", e)))?;
    atomic_write(&path, &out)?;
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
    let model = cli_model.unwrap_or_else(|| profile.model.clone());
    // Config key is `max-concurrent-tools`; CLI flag is `--max-parallel-tools`.
    // Both refer to the same bounded-concurrency cap. CLI wins when provided.
    let max_parallel_tools = cli_max_parallel_tools
        .or(loaded.agent.max_concurrent_tools)
        .unwrap_or(4);
    let def = AgentConfig::default();
    Ok(AgentConfig {
        max_turns: cli_max_turns.unwrap_or(loaded.agent.max_turns),
        max_budget_usd: cli_max_budget_usd.or(loaded.agent.max_budget_usd),
        model,
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
        max_wall_time_per_turn_sec: loaded
            .agent
            .max_wall_time_per_turn_sec
            .unwrap_or(def.max_wall_time_per_turn_sec),
        max_tool_calls_per_turn: loaded
            .agent
            .max_tool_calls_per_turn
            .unwrap_or(def.max_tool_calls_per_turn),
        stall_threshold: loaded
            .agent
            .stall_threshold
            .unwrap_or(def.stall_threshold),
        doom_consecutive_same_error: loaded
            .agent
            .doom_consecutive_same_error
            .unwrap_or(def.doom_consecutive_same_error),
        doom_same_args_window: loaded
            .agent
            .doom_same_args_window
            .unwrap_or(def.doom_same_args_window),
        doom_same_args_min: loaded
            .agent
            .doom_same_args_min
            .unwrap_or(def.doom_same_args_min),
        max_tool_retries: loaded
            .agent
            .max_tool_retries
            .unwrap_or(def.max_tool_retries),
        retry_backoff_max_ms: loaded
            .agent
            .retry_backoff_max_ms
            .unwrap_or(def.retry_backoff_max_ms),
        retry_jitter_numerator: loaded
            .agent
            .retry_jitter_numerator
            .unwrap_or(def.retry_jitter_numerator),
        provider_min_request_interval_ms: loaded
            .agent
            .provider_min_request_interval_ms
            .unwrap_or(def.provider_min_request_interval_ms),
        stream_model_completion: loaded.agent.stream_model_completion,
        tool_timeout_secs: loaded
            .agent
            .tool_timeout_secs
            .unwrap_or(def.tool_timeout_secs),
        max_tool_output_bytes: loaded
            .agent
            .max_tool_output_bytes
            .unwrap_or(def.max_tool_output_bytes),
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
        assert!((c.compaction_threshold - 0.58).abs() < f64::EPSILON);
        assert!(c.max_context_tokens.is_none());
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
            skills: SkillsSection::default(),
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
            fast: None,
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
            fast: None,
        };
        upsert_profile_in_config(&cfg_path, "default", &entry).unwrap();
        let src = std::fs::read_to_string(&cfg_path).unwrap();
        let cf: ConfigFile = toml::from_str(&src).unwrap();
        assert_eq!(cf.profile["default"].model, "new-model");
    }

    // ── ProfileEntry with fast provider serialization ─────────────────

    #[test]
    fn profile_entry_with_fast_toml_roundtrip() {
        let entry = crate::ProfileEntry {
            provider: "anthropic".to_string(),
            model: "claude-opus-4-6".to_string(),
            api_key: Some("sk-ant".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: Some(crate::config::FastProviderConfig {
                provider: "openrouter".to_string(),
                model: "google/gemini-2.0-flash".to_string(),
                api_key: Some("sk-or-test".to_string()),
                api_key_env: None,
                base_url: None,
                user_agent: None,
            }),
        };
        let serialized = toml::to_string(&entry).unwrap();
        let deserialized: crate::ProfileEntry = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.model, "claude-opus-4-6");
        let f = deserialized.fast.expect("fast should round-trip");
        assert_eq!(f.provider, "openrouter");
        assert_eq!(f.model, "google/gemini-2.0-flash");
    }

    #[test]
    fn profile_entry_without_fast_serializes_cleanly() {
        let entry = crate::ProfileEntry {
            provider: "anthropic".to_string(),
            model: "m".to_string(),
            api_key: Some("k".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        let s = toml::to_string(&entry).unwrap();
        assert!(!s.contains("fast"), "fast=None should not appear: {}", s);
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
            fast: None,
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
            fast: None,
        };
        upsert_profile_in_config(&cfg, "coding", &entry).unwrap();
        let content = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            content.contains("default_profile"),
            "should set default_profile: {}",
            content
        );
    }

    // ── upsert_profile: additional coverage ──────────────────────────────

    #[test]
    fn upsert_profile_to_nonexistent_file_creates_parents_and_file() {
        let temp = tempfile::tempdir().unwrap();
        // Target a path with a non-existent parent directory
        let cfg_path = temp.path().join("subdir").join("config.toml");
        assert!(!cfg_path.exists());
        let entry = ProfileEntry {
            provider: "anthropic".to_string(),
            model: "claude-sonnet".to_string(),
            api_key: Some("sk-ant-new".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        upsert_profile_in_config(&cfg_path, "fresh", &entry).unwrap();
        assert!(cfg_path.exists());
        // Verify the written file is valid TOML with the profile
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.profile["fresh"].provider, "anthropic");
        assert_eq!(cf.profile["fresh"].model, "claude-sonnet");
        // First profile becomes the default
        assert_eq!(cf.default_profile, "fresh");
    }

    #[test]
    fn upsert_profile_preserves_other_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"alpha\"\n\
             [profile.alpha]\nprovider = \"anthropic\"\nmodel = \"m1\"\napi_key = \"k1\"\n\
             [profile.beta]\nprovider = \"openai\"\nmodel = \"m2\"\napi_key = \"k2\"\n",
        )
        .unwrap();
        let entry = ProfileEntry {
            provider: "deepseek".to_string(),
            model: "deepseek-v3".to_string(),
            api_key: Some("sk-ds".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        upsert_profile_in_config(&cfg_path, "gamma", &entry).unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.profile.len(), 3);
        assert!(cf.profile.contains_key("alpha"));
        assert!(cf.profile.contains_key("beta"));
        assert!(cf.profile.contains_key("gamma"));
        // default_profile unchanged since it already existed
        assert_eq!(cf.default_profile, "alpha");
    }

    #[test]
    fn upsert_profile_update_changes_only_target_profile() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n\
             [profile.default]\nprovider = \"anthropic\"\nmodel = \"old\"\napi_key = \"old-key\"\n\
             [profile.work]\nprovider = \"openai\"\nmodel = \"gpt-4o\"\napi_key = \"sk-work\"\n",
        )
        .unwrap();
        let updated = ProfileEntry {
            provider: "anthropic".to_string(),
            model: "updated-model".to_string(),
            api_key: Some("updated-key".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        upsert_profile_in_config(&cfg_path, "default", &updated).unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.profile["default"].model, "updated-model");
        // Other profile is untouched
        assert_eq!(cf.profile["work"].model, "gpt-4o");
    }

    #[test]
    fn upsert_profile_with_optional_fields() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        let entry = ProfileEntry {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            api_key: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            base_url: Some("https://custom.endpoint.com".to_string()),
            user_agent: Some("CustomAgent/1.0".to_string()),
            fast: None,
        };
        upsert_profile_in_config(&cfg_path, "custom", &entry).unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        let p = &cf.profile["custom"];
        assert!(p.api_key.is_none());
        assert_eq!(p.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(p.base_url.as_deref(), Some("https://custom.endpoint.com"));
        assert_eq!(p.user_agent.as_deref(), Some("CustomAgent/1.0"));
    }

    // ── delete_profile: additional coverage ───────────────────────────────

    #[test]
    fn delete_profile_nonexistent_profile_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n\
             [profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n",
        )
        .unwrap();
        // Deleting a profile that doesn't exist should succeed silently
        let result = delete_profile_from_config(&cfg_path, "nonexistent");
        assert!(result.is_ok());
        // Original profile still there
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert!(cf.profile.contains_key("default"));
    }

    #[test]
    fn delete_profile_last_non_default_leaves_default_only() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n\
             [profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n\
             [profile.extra]\nprovider = \"openai\"\nmodel = \"gpt\"\napi_key = \"sk\"\n",
        )
        .unwrap();
        delete_profile_from_config(&cfg_path, "extra").unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.profile.len(), 1);
        assert!(cf.profile.contains_key("default"));
    }

    #[test]
    fn delete_profile_multiple_profiles_removes_only_target() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"alpha\"\n\
             [profile.alpha]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n\
             [profile.beta]\nprovider = \"openai\"\nmodel = \"m2\"\napi_key = \"k2\"\n\
             [profile.gamma]\nprovider = \"deepseek\"\nmodel = \"m3\"\napi_key = \"k3\"\n",
        )
        .unwrap();
        delete_profile_from_config(&cfg_path, "beta").unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.profile.len(), 2);
        assert!(cf.profile.contains_key("alpha"));
        assert!(!cf.profile.contains_key("beta"));
        assert!(cf.profile.contains_key("gamma"));
    }

    // ── switch_active_profile: additional coverage ────────────────────────

    #[test]
    fn switch_active_profile_preserves_all_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n\
             [profile.default]\nprovider = \"anthropic\"\nmodel = \"m1\"\napi_key = \"k1\"\n\
             [profile.work]\nprovider = \"openai\"\nmodel = \"m2\"\napi_key = \"k2\"\n",
        )
        .unwrap();
        switch_active_profile(&cfg_path, "work").unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.default_profile, "work");
        assert!(cf.profile.contains_key("default"));
        assert!(cf.profile.contains_key("work"));
        assert_eq!(cf.profile["default"].model, "m1");
    }

    #[test]
    fn switch_active_profile_to_nonexistent_profile_succeeds() {
        // The function just writes the string; it doesn't validate the profile exists.
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "default_profile = \"default\"\n\
             [profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n",
        )
        .unwrap();
        let result = switch_active_profile(&cfg_path, "nonexistent");
        assert!(result.is_ok());
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.default_profile, "nonexistent");
    }

    #[test]
    fn switch_active_profile_preserves_hyphen_format_key() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        // Config written with hyphen-style key
        std::fs::write(
            &cfg_path,
            "default-profile = \"default\"\n\
             [profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n\
             [profile.alt]\nprovider = \"openai\"\nmodel = \"m2\"\napi_key = \"k2\"\n",
        )
        .unwrap();
        switch_active_profile(&cfg_path, "alt").unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.default_profile, "alt");
    }

    #[test]
    fn switch_active_profile_on_missing_file_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("does_not_exist.toml");
        let result = switch_active_profile(&cfg_path, "anything");
        assert!(result.is_err());
    }

    // ── load_config: additional coverage ──────────────────────────────────

    #[test]
    fn load_config_finds_config_in_parent_directory() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        // Place config in parent's .clido/config.toml
        let parent = temp.path();
        let clido_dir = parent.join(".clido");
        std::fs::create_dir_all(&clido_dir).unwrap();
        std::fs::write(
            clido_dir.join("config.toml"),
            "default_profile = \"default\"\n\
             [profile.default]\nprovider = \"anthropic\"\nmodel = \"from-parent\"\napi_key = \"k\"\n",
        )
        .unwrap();
        // Create a child directory with no config
        let child = parent.join("subproject");
        std::fs::create_dir_all(&child).unwrap();

        // Point CLIDO_CONFIG to a non-existent path to skip global config
        let nonexistent = temp.path().join("no_global.toml");
        unsafe { std::env::set_var("CLIDO_CONFIG", &nonexistent) };
        let loaded = load_config(&child).unwrap();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        assert_eq!(loaded.profiles["default"].model, "from-parent");
    }

    #[test]
    fn load_config_no_config_anywhere_returns_error() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        // Override CLIDO_CONFIG to skip global config
        let nonexistent = temp.path().join("no_global.toml");
        unsafe { std::env::set_var("CLIDO_CONFIG", &nonexistent) };
        let result = load_config(temp.path());
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("clido init"), "expected init hint in: {}", msg);
    }

    #[test]
    fn load_config_project_config_overrides_global_agent_settings() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        // Global config with agent settings
        let global_cfg = temp.path().join("global.toml");
        std::fs::write(
            &global_cfg,
            "default_profile = \"default\"\n\
             [profile.default]\nprovider = \"anthropic\"\nmodel = \"m\"\napi_key = \"k\"\n\
             [agent]\nmax-turns = 50\nquiet = true\n",
        )
        .unwrap();
        // Project config overrides max-turns
        let project = temp.path().join("project");
        std::fs::create_dir_all(project.join(".clido")).unwrap();
        std::fs::write(
            project.join(".clido").join("config.toml"),
            "[agent]\nmax-turns = 300\n",
        )
        .unwrap();
        unsafe { std::env::set_var("CLIDO_CONFIG", &global_cfg) };
        let loaded = load_config(&project).unwrap();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        assert_eq!(loaded.agent.max_turns, 300);
        // quiet is OR'd: global true || project false(default) = true
        assert!(loaded.agent.quiet);
    }

    #[test]
    fn load_config_merges_profiles_from_global_and_project() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let global_cfg = temp.path().join("global.toml");
        std::fs::write(
            &global_cfg,
            "default_profile = \"global-prof\"\n\
             [profile.global-prof]\nprovider = \"anthropic\"\nmodel = \"m-global\"\napi_key = \"k1\"\n",
        )
        .unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(project.join(".clido")).unwrap();
        std::fs::write(
            project.join(".clido").join("config.toml"),
            "default_profile = \"local-prof\"\n\
             [profile.local-prof]\nprovider = \"openai\"\nmodel = \"m-local\"\napi_key = \"k2\"\n",
        )
        .unwrap();
        unsafe { std::env::set_var("CLIDO_CONFIG", &global_cfg) };
        let loaded = load_config(&project).unwrap();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        // Both profiles available
        assert!(loaded.profiles.contains_key("global-prof"));
        assert!(loaded.profiles.contains_key("local-prof"));
        // Project default wins
        assert_eq!(loaded.default_profile, "local-prof");
    }

    // ── agent_config_from_loaded: additional coverage ─────────────────────

    #[test]
    fn agent_config_from_loaded_partial_cli_overrides() {
        // Only some CLI args provided, rest should come from config
        let mut profiles = HashMap::new();
        profiles.insert(
            "default".to_string(),
            ProfileEntry {
                provider: "anthropic".to_string(),
                model: "config-model".to_string(),
                api_key: Some("k".to_string()),
                api_key_env: None,
                base_url: None,
                user_agent: None,
                fast: None,
            },
        );
        let loaded = LoadedConfig {
            default_profile: "default".to_string(),
            profiles,
            agent: AgentSection {
                max_turns: 150,
                max_budget_usd: Some(5.0),
                max_concurrent_tools: Some(6),
                quiet: true,
                ..Default::default()
            },
            tools: ToolsSection::default(),
            context: ContextSection::default(),
            workflows: WorkflowsSection::default(),
            hooks: HooksConfig::default(),
            index: IndexSection::default(),
            skills: SkillsSection::default(),
        };
        // Override only max_turns via CLI
        let cfg = agent_config_from_loaded(
            &loaded,
            "default",
            Some(10), // CLI override
            None,     // config's max_budget_usd should be used
            None,     // config's model should be used
            None,
            None,
            false, // CLI quiet=false, but config quiet=true → OR'd = true
            None,  // config's max_concurrent_tools should be used
        )
        .unwrap();
        assert_eq!(cfg.max_turns, 10);
        assert_eq!(cfg.max_budget_usd, Some(5.0));
        assert_eq!(cfg.model, "config-model");
        assert!(cfg.quiet);
        assert_eq!(cfg.max_parallel_tools, 6);
    }

    #[test]
    fn agent_config_from_loaded_with_config_agent_settings() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "default".to_string(),
            ProfileEntry {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                api_key: Some("sk".to_string()),
                api_key_env: None,
                base_url: None,
                user_agent: None,
                fast: None,
            },
        );
        let loaded = LoadedConfig {
            default_profile: "default".to_string(),
            profiles,
            agent: AgentSection {
                max_turns: 75,
                max_budget_usd: Some(10.0),
                max_concurrent_tools: Some(12),
                quiet: false,
                no_rules: true,
                rules_file: Some("custom-rules.md".to_string()),
                max_output_tokens: Some(4096),
                ..Default::default()
            },
            tools: ToolsSection::default(),
            context: ContextSection {
                compaction_threshold: 0.5,
                max_context_tokens: Some(100_000),
            },
            workflows: WorkflowsSection::default(),
            hooks: HooksConfig::default(),
            index: IndexSection::default(),
            skills: SkillsSection::default(),
        };
        let cfg = agent_config_from_loaded(
            &loaded, "default", None, None, None, None, None, false, None,
        )
        .unwrap();
        assert_eq!(cfg.max_turns, 75);
        assert_eq!(cfg.max_budget_usd, Some(10.0));
        assert_eq!(cfg.max_parallel_tools, 12);
        assert!(cfg.no_rules);
        assert_eq!(cfg.rules_file.as_deref(), Some("custom-rules.md"));
        assert_eq!(cfg.max_output_tokens, Some(4096));
        assert_eq!(cfg.max_context_tokens, Some(100_000));
        assert!((cfg.compaction_threshold.unwrap() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn agent_config_from_loaded_cli_quiet_overrides_config() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "default".to_string(),
            ProfileEntry {
                provider: "anthropic".to_string(),
                model: "m".to_string(),
                api_key: Some("k".to_string()),
                api_key_env: None,
                base_url: None,
                user_agent: None,
                fast: None,
            },
        );
        let loaded = LoadedConfig {
            default_profile: "default".to_string(),
            profiles,
            agent: AgentSection {
                quiet: false,
                ..Default::default()
            },
            tools: ToolsSection::default(),
            context: ContextSection::default(),
            workflows: WorkflowsSection::default(),
            hooks: HooksConfig::default(),
            index: IndexSection::default(),
            skills: SkillsSection::default(),
        };
        let cfg =
            agent_config_from_loaded(&loaded, "default", None, None, None, None, None, true, None)
                .unwrap();
        assert!(cfg.quiet);
    }

    #[test]
    fn agent_config_from_loaded_default_parallel_tools_when_none_set() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "default".to_string(),
            ProfileEntry {
                provider: "anthropic".to_string(),
                model: "m".to_string(),
                api_key: Some("k".to_string()),
                api_key_env: None,
                base_url: None,
                user_agent: None,
                fast: None,
            },
        );
        let loaded = LoadedConfig {
            default_profile: "default".to_string(),
            profiles,
            agent: AgentSection {
                max_concurrent_tools: None,
                ..Default::default()
            },
            tools: ToolsSection::default(),
            context: ContextSection::default(),
            workflows: WorkflowsSection::default(),
            hooks: HooksConfig::default(),
            index: IndexSection::default(),
            skills: SkillsSection::default(),
        };
        let cfg = agent_config_from_loaded(
            &loaded, "default", None, None, None, None, None, false, None,
        )
        .unwrap();
        // Default is 4 when neither CLI nor config specifies
        assert_eq!(cfg.max_parallel_tools, 4);
    }

    // ── roundtrip: upsert then load ──────────────────────────────────────

    #[test]
    fn upsert_then_load_roundtrip() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let clido_dir = temp.path().join(".clido");
        std::fs::create_dir_all(&clido_dir).unwrap();
        let cfg_path = clido_dir.join("config.toml");
        let entry = ProfileEntry {
            provider: "anthropic".to_string(),
            model: "claude-haiku".to_string(),
            api_key: Some("sk-test".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        upsert_profile_in_config(&cfg_path, "myprofile", &entry).unwrap();
        // Point CLIDO_CONFIG away so only the project config is found
        let nonexistent = temp.path().join("no_global.toml");
        unsafe { std::env::set_var("CLIDO_CONFIG", &nonexistent) };
        let loaded = load_config(temp.path()).unwrap();
        unsafe { std::env::remove_var("CLIDO_CONFIG") };
        assert_eq!(loaded.profiles["myprofile"].provider, "anthropic");
        assert_eq!(loaded.profiles["myprofile"].model, "claude-haiku");
        assert_eq!(loaded.default_profile, "myprofile");
    }

    #[test]
    fn upsert_delete_switch_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let cfg_path = temp.path().join("config.toml");
        // Start with one profile
        let entry_a = ProfileEntry {
            provider: "anthropic".to_string(),
            model: "m-a".to_string(),
            api_key: Some("k-a".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        upsert_profile_in_config(&cfg_path, "alpha", &entry_a).unwrap();
        // Add second profile
        let entry_b = ProfileEntry {
            provider: "openai".to_string(),
            model: "m-b".to_string(),
            api_key: Some("k-b".to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        upsert_profile_in_config(&cfg_path, "beta", &entry_b).unwrap();
        // Switch default to beta
        switch_active_profile(&cfg_path, "beta").unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(cf.default_profile, "beta");
        // Delete alpha (non-default)
        delete_profile_from_config(&cfg_path, "alpha").unwrap();
        let cf: ConfigFile = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert!(!cf.profile.contains_key("alpha"));
        assert!(cf.profile.contains_key("beta"));
        assert_eq!(cf.default_profile, "beta");
    }

    #[test]
    fn set_skill_disabled_roundtrip_in_project_config() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        set_skill_disabled_in_project(root, "foo", true).unwrap();
        set_skill_disabled_in_project(root, "bar", true).unwrap();
        let s = std::fs::read_to_string(root.join(".clido/config.toml")).unwrap();
        let cf: ConfigFile = toml::from_str(&s).unwrap();
        assert!(cf.skills.disabled.contains(&"foo".to_string()));
        assert!(cf.skills.disabled.contains(&"bar".to_string()));
        set_skill_disabled_in_project(root, "foo", false).unwrap();
        let s2 = std::fs::read_to_string(root.join(".clido/config.toml")).unwrap();
        let cf2: ConfigFile = toml::from_str(&s2).unwrap();
        assert!(!cf2.skills.disabled.contains(&"foo".to_string()));
        assert!(cf2.skills.disabled.contains(&"bar".to_string()));
    }
}
