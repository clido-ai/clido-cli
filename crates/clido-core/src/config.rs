//! Agent and provider configuration types (from config.toml / CLI).

use serde::{Deserialize, Serialize};

/// Permission mode for state-changing tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    #[default]
    Default,
    AcceptAll,
    PlanOnly,
    /// Show a diff preview modal before every Write and Edit operation.
    DiffReview,
}

/// Agent-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub max_turns: u32,
    pub max_budget_usd: Option<f64>,
    pub model: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub use_planner: bool,
    #[serde(default)]
    pub use_index: bool,
    /// Max context tokens (from config or pricing). None = use default in context engine (e.g. 200000).
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
    /// Compact when context_tokens > max_context_tokens * compaction_threshold. Default 0.75.
    #[serde(default)]
    pub compaction_threshold: Option<f64>,
    /// Suppress spinner, tool lifecycle output, and cost footer.
    #[serde(default)]
    pub quiet: bool,
    /// Max parallel tool calls for read-only tools (bounded concurrency). Default 4.
    /// Config key is `max_concurrent_tools` (per spec); CLI flag/env use `max_parallel_tools`.
    /// Both names are accepted for compatibility.
    #[serde(default = "default_max_parallel_tools", alias = "max_concurrent_tools")]
    pub max_parallel_tools: u32,
    /// Skip all CLIDO.md / rules file injection.
    #[serde(default)]
    pub no_rules: bool,
    /// Use a specific rules file instead of the standard hierarchical lookup.
    #[serde(default)]
    pub rules_file: Option<String>,
    /// Maximum tokens the model may produce per response. None = provider default (8192).
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

fn default_max_parallel_tools() -> u32 {
    4
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: 200,
            max_budget_usd: None,
            model: String::new(),
            system_prompt: None,
            permission_mode: PermissionMode::Default,
            use_planner: false,
            use_index: false,
            max_context_tokens: None,
            compaction_threshold: None,
            quiet: false,
            max_parallel_tools: 4,
            no_rules: false,
            rules_file: None,
            max_output_tokens: None,
        }
    }
}

/// Provider type (canonical names from config spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    OpenRouter,
    MiniMax,
    Alibaba,
    Local,
}

/// Hooks configuration: shell commands run before/after each tool call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Shell command to run before each tool use. Env: CLIDO_TOOL_NAME, CLIDO_TOOL_INPUT.
    pub pre_tool_use: Option<String>,
    /// Shell command to run after each tool use. Env: CLIDO_TOOL_NAME, CLIDO_TOOL_INPUT, CLIDO_TOOL_OUTPUT, CLIDO_TOOL_IS_ERROR, CLIDO_TOOL_DURATION_MS.
    pub post_tool_use: Option<String>,
}

/// Provider-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: String,
}

/// Configuration for a single agent slot (main, worker, or reviewer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSlotConfig {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Tiered agents config: main (required for use), worker + reviewer (optional, fall back to main).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentsConfig {
    pub main: Option<AgentSlotConfig>,
    pub worker: Option<AgentSlotConfig>,
    pub reviewer: Option<AgentSlotConfig>,
}

/// Named role → model ID mapping (from `[roles]` in config.toml).
/// Built-in roles: fast, reasoning, critic, planner. Arbitrary user-defined roles are allowed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RolesConfig {
    /// Model for the "fast" role (e.g. a cheaper/quicker model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<String>,
    /// Model for the "reasoning" role (e.g. a thinking/extended model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Model for the "critic" role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critic: Option<String>,
    /// Model for the "planner" role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner: Option<String>,
    /// Arbitrary user-defined roles. Key is role name, value is model ID.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, String>,
}

impl RolesConfig {
    /// Look up a model ID for a named role. Checks built-in fields first, then extra.
    pub fn resolve(&self, role: &str) -> Option<&str> {
        match role {
            "fast" => self.fast.as_deref(),
            "reasoning" | "smart" => self.reasoning.as_deref(),
            "critic" => self.critic.as_deref(),
            "planner" => self.planner.as_deref(),
            other => self.extra.get(other).map(|s| s.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_config_from_json() {
        let json = r#"{
            "max_turns": 20,
            "max_budget_usd": 1.0,
            "model": "claude-3-5-sonnet",
            "permission_mode": "plan-only",
            "use_planner": false,
            "use_index": false
        }"#;
        let c: AgentConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.max_turns, 20);
        assert_eq!(c.max_budget_usd, Some(1.0));
        assert_eq!(c.model, "claude-3-5-sonnet");
        assert_eq!(c.permission_mode, PermissionMode::PlanOnly);
    }

    #[test]
    fn agent_config_defaults() {
        let c = AgentConfig::default();
        assert_eq!(c.max_turns, 200);
        assert_eq!(c.max_budget_usd, None);
        assert_eq!(c.permission_mode, PermissionMode::Default);
        assert!(!c.use_planner);
        assert!(!c.use_index);
        assert!(!c.quiet);
        assert_eq!(c.max_parallel_tools, 4);
        assert!(!c.no_rules);
        assert!(c.system_prompt.is_none());
        assert!(c.max_context_tokens.is_none());
        assert!(c.compaction_threshold.is_none());
        assert!(c.rules_file.is_none());
    }

    #[test]
    fn permission_mode_serialization() {
        assert_eq!(
            serde_json::to_string(&PermissionMode::Default).unwrap(),
            "\"default\""
        );
        assert_eq!(
            serde_json::to_string(&PermissionMode::AcceptAll).unwrap(),
            "\"accept-all\""
        );
        assert_eq!(
            serde_json::to_string(&PermissionMode::PlanOnly).unwrap(),
            "\"plan-only\""
        );
        assert_eq!(
            serde_json::to_string(&PermissionMode::DiffReview).unwrap(),
            "\"diff-review\""
        );
    }

    #[test]
    fn permission_mode_default_is_default() {
        assert_eq!(PermissionMode::default(), PermissionMode::Default);
    }

    #[test]
    fn permission_mode_roundtrip() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptAll,
            PermissionMode::PlanOnly,
            PermissionMode::DiffReview,
        ] {
            let s = serde_json::to_string(&mode).unwrap();
            let back: PermissionMode = serde_json::from_str(&s).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn hooks_config_default_is_empty() {
        let h = HooksConfig::default();
        assert!(h.pre_tool_use.is_none());
        assert!(h.post_tool_use.is_none());
    }

    #[test]
    fn agent_config_json_with_all_fields() {
        let json = r#"{
            "max_turns": 50,
            "max_budget_usd": 2.5,
            "model": "gpt-4o",
            "system_prompt": "You are an expert.",
            "permission_mode": "accept-all",
            "use_planner": true,
            "use_index": true,
            "max_context_tokens": 100000,
            "compaction_threshold": 0.8,
            "quiet": true,
            "max_parallel_tools": 8,
            "no_rules": true,
            "rules_file": "RULES.md"
        }"#;
        let c: AgentConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.max_turns, 50);
        assert_eq!(c.permission_mode, PermissionMode::AcceptAll);
        assert!(c.use_planner);
        assert!(c.use_index);
        assert_eq!(c.max_context_tokens, Some(100000));
        assert_eq!(c.compaction_threshold, Some(0.8));
        assert!(c.quiet);
        assert_eq!(c.max_parallel_tools, 8);
        assert!(c.no_rules);
        assert_eq!(c.rules_file.as_deref(), Some("RULES.md"));
        assert_eq!(c.system_prompt.as_deref(), Some("You are an expert."));
    }
}
