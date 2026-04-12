//! Agent and provider configuration types (from config.toml / CLI).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Per-file permission rules
// ---------------------------------------------------------------------------

/// Action to take when a permission rule matches a tool's file argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleAction {
    /// Automatically allow without prompting.
    Allow,
    /// Automatically deny and return feedback to the model.
    Deny,
    /// Ask the user interactively.
    Ask,
}

/// A single glob-pattern-based permission rule.
///
/// Rules are evaluated in order; the first match wins.  If no rule matches,
/// the effective `PermissionMode` fallback is used.
///
/// Example TOML:
/// ```toml
/// [[permission_rules]]
/// pattern = "src/**"
/// action = "allow"
///
/// [[permission_rules]]
/// pattern = "tests/**"
/// action = "ask"
///
/// [[permission_rules]]
/// pattern = "**"
/// action = "deny"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Glob pattern matched against the tool's primary file argument (relative to workspace root).
    pub pattern: String,
    /// Action to take on match.
    pub action: RuleAction,
    /// Optional human-readable reason shown when denying.
    #[serde(default)]
    pub reason: Option<String>,
}

impl PermissionRule {
    /// Return `true` when `path` matches this rule's glob pattern.
    pub fn matches(&self, path: &str) -> bool {
        glob_match(&self.pattern, path)
    }
}

/// Evaluate an ordered rule list against `path`.  Returns the first matching
/// action, or `None` when no rule matches.
pub fn evaluate_rules(
    rules: &[PermissionRule],
    path: &str,
) -> Option<(RuleAction, Option<String>)> {
    for rule in rules {
        if rule.matches(path) {
            return Some((rule.action, rule.reason.clone()));
        }
    }
    None
}

/// Simple glob matcher supporting `*` (within segment) and `**` (multi-segment).
fn glob_match(pattern: &str, path: &str) -> bool {
    glob_match_parts(
        &pattern.split('/').collect::<Vec<_>>(),
        &path.split('/').collect::<Vec<_>>(),
    )
}

fn glob_match_parts(pat: &[&str], path: &[&str]) -> bool {
    match (pat.first(), path.first()) {
        (None, None) => true,
        (None, _) | (_, None) => {
            // Allow trailing **
            pat.iter().all(|p| *p == "**")
        }
        (Some(&"**"), _) => {
            // ** can consume zero or more path segments.
            if glob_match_parts(&pat[1..], path) {
                return true;
            }
            glob_match_parts(pat, &path[1..])
        }
        (Some(p), Some(s)) => {
            if segment_match(p, s) {
                glob_match_parts(&pat[1..], &path[1..])
            } else {
                false
            }
        }
    }
}

/// Match a single path segment against a pattern segment (supports `*`).
fn segment_match(pattern: &str, segment: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Simple * wildcard within segment.
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == segment;
    }
    let mut s = segment;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !s.starts_with(part) {
                return false;
            }
            s = &s[part.len()..];
        } else if i == parts.len() - 1 {
            if !s.ends_with(part) {
                return false;
            }
        } else {
            match s.find(part) {
                Some(pos) => s = &s[pos + part.len()..],
                None => return false,
            }
        }
    }
    true
}

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
    /// Total USD spend cap for the agent instance across all outer turns until history is replaced
    /// (e.g. session resume load).
    /// `None` = unlimited.
    pub max_budget_usd: Option<f64>,
    /// Optional cap on model spend **within one outer user turn** (one `completion_loop_run`).
    /// Checked after each provider completion. `None` = no per-turn cap (session cap still applies).
    #[serde(default)]
    pub max_budget_usd_per_turn: Option<f64>,
    pub model: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    /// Optional ordered list of glob-based per-file permission rules.
    /// Rules are evaluated before the `permission_mode` fallback.
    /// First matching rule wins.
    #[serde(default)]
    pub permission_rules: Vec<PermissionRule>,
    pub use_planner: bool,
    #[serde(default)]
    pub use_index: bool,
    /// Max context tokens (from config or pricing). None = use default in context engine (e.g. 200000).
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
    /// Compact when context_tokens > max_context_tokens * compaction_threshold. Default 0.58.
    #[serde(default)]
    pub compaction_threshold: Option<f64>,
    /// Suppress spinner, tool lifecycle output, and cost footer.
    #[serde(default)]
    pub quiet: bool,
    /// When the model requests multiple **read-only** tools in one turn, run up to this many
    /// concurrently (semaphore). Batches that include any write-capable tool run sequentially
    /// through the gated path (permissions, hooks).
    /// Config key is `max_concurrent_tools` (per spec); CLI flag/env use `max_parallel_tools`.
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
    /// Wall-clock seconds allowed for one user turn (entire completion loop). 0 = no limit.
    #[serde(default = "default_max_wall_time_per_turn_sec")]
    pub max_wall_time_per_turn_sec: u64,
    /// Maximum tool invocations (individual calls) per user turn.
    #[serde(default = "default_max_tool_calls_per_turn")]
    pub max_tool_calls_per_turn: u32,
    /// Stall score threshold before failing the turn (see agent loop stall tracker).
    #[serde(default = "default_stall_threshold")]
    pub stall_threshold: u32,
    /// Consecutive identical normalized tool errors to trigger doom loop.
    #[serde(default = "default_doom_consecutive")]
    pub doom_consecutive_same_error: usize,
    /// Sliding window size (entries) for doom-loop args repetition detection.
    #[serde(default = "default_doom_window")]
    pub doom_same_args_window: usize,
    /// Repeated identical `(tool, args_hash)` within the window triggers doom (minimum count).
    #[serde(default = "default_doom_same_args_min")]
    pub doom_same_args_min: usize,
    /// Auto-retries per tool call for transient failures (network, etc.).
    #[serde(default = "default_max_tool_retries")]
    pub max_tool_retries: u32,
    /// Max **retry scheduling events** (not counting the first attempt) summed across all tools in
    /// one outer user turn. Stops retry storms when combined with per-tool `max_tool_retries`.
    #[serde(default = "default_max_tool_retry_budget_per_turn")]
    pub max_tool_retry_budget_per_turn: u32,
    /// Upper bound on exponential backoff delay between retries (milliseconds).
    #[serde(default = "default_retry_backoff_max_ms")]
    pub retry_backoff_max_ms: u64,
    /// Jitter as a fraction of delay: delay * jitter_numerator / 100.
    #[serde(default = "default_retry_jitter_numerator")]
    pub retry_jitter_numerator: u8,
    /// Minimum spacing between provider `complete` calls (ms). 0 = disabled.
    #[serde(default)]
    pub provider_min_request_interval_ms: u32,
    /// When true, use `complete_stream` and aggregate to a full [`ModelResponse`] (provider support required).
    #[serde(default)]
    pub stream_model_completion: bool,
    /// Per-tool execute timeout (seconds). Applies to the agent loop wrapper around `Tool::execute`.
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
    /// Truncate tool output text beyond this many bytes (0 = unlimited).
    #[serde(default = "default_max_tool_output_bytes")]
    pub max_tool_output_bytes: usize,
}

fn default_max_parallel_tools() -> u32 {
    4
}

fn default_max_wall_time_per_turn_sec() -> u64 {
    900
}

fn default_max_tool_calls_per_turn() -> u32 {
    200
}

fn default_stall_threshold() -> u32 {
    12
}

fn default_doom_consecutive() -> usize {
    3
}

fn default_doom_window() -> usize {
    8
}

fn default_doom_same_args_min() -> usize {
    4
}

fn default_max_tool_retries() -> u32 {
    3
}

fn default_max_tool_retry_budget_per_turn() -> u32 {
    64
}

fn default_retry_backoff_max_ms() -> u64 {
    10_000
}

fn default_retry_jitter_numerator() -> u8 {
    25
}

fn default_tool_timeout_secs() -> u64 {
    300 // 5 minutes default for long-running tools
}

fn default_max_tool_output_bytes() -> usize {
    512_000
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: 200,
            max_budget_usd: None,
            max_budget_usd_per_turn: None,
            model: String::new(),
            system_prompt: None,
            permission_mode: PermissionMode::Default,
            permission_rules: Vec::new(),
            use_planner: false,
            use_index: false,
            max_context_tokens: None,
            compaction_threshold: None,
            quiet: false,
            max_parallel_tools: 4,
            no_rules: false,
            rules_file: None,
            max_output_tokens: None,
            max_wall_time_per_turn_sec: default_max_wall_time_per_turn_sec(),
            max_tool_calls_per_turn: default_max_tool_calls_per_turn(),
            stall_threshold: default_stall_threshold(),
            doom_consecutive_same_error: default_doom_consecutive(),
            doom_same_args_window: default_doom_window(),
            doom_same_args_min: default_doom_same_args_min(),
            max_tool_retries: default_max_tool_retries(),
            max_tool_retry_budget_per_turn: default_max_tool_retry_budget_per_turn(),
            retry_backoff_max_ms: default_retry_backoff_max_ms(),
            retry_jitter_numerator: default_retry_jitter_numerator(),
            provider_min_request_interval_ms: 0,
            stream_model_completion: false,
            tool_timeout_secs: default_tool_timeout_secs(),
            max_tool_output_bytes: default_max_tool_output_bytes(),
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
    Kimi,
    #[serde(rename = "kimi-code")]
    KimiCode,
    Alibaba,
    DeepSeek,
    Groq,
    Cerebras,
    #[serde(rename = "togetherai")]
    TogetherAI,
    Fireworks,
    #[serde(rename = "xai")]
    XAI,
    Perplexity,
    Gemini,
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

/// Configuration for the optional fast/cheap provider used for utility tasks
/// (summarization, title generation, commit messages, sub-agent work).
/// Parsed from `[profiles.<name>.fast]` in config.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastProviderConfig {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PermissionRule / glob coverage ──────────────────────────────────────

    #[test]
    fn permission_rule_matches_exact() {
        let rule = PermissionRule {
            pattern: "src/main.rs".to_string(),
            action: RuleAction::Allow,
            reason: None,
        };
        assert!(rule.matches("src/main.rs"));
        assert!(!rule.matches("src/lib.rs"));
    }

    #[test]
    fn permission_rule_matches_star_wildcard() {
        let rule = PermissionRule {
            pattern: "src/*.rs".to_string(),
            action: RuleAction::Deny,
            reason: Some("no rust edits".to_string()),
        };
        assert!(rule.matches("src/main.rs"));
        assert!(rule.matches("src/lib.rs"));
        assert!(!rule.matches("src/sub/lib.rs"));
    }

    #[test]
    fn permission_rule_matches_double_star() {
        let rule = PermissionRule {
            pattern: "src/**".to_string(),
            action: RuleAction::Ask,
            reason: None,
        };
        assert!(rule.matches("src/main.rs"));
        assert!(rule.matches("src/sub/deep/file.rs"));
        assert!(!rule.matches("tests/foo.rs"));
    }

    #[test]
    fn evaluate_rules_returns_first_match() {
        let rules = vec![
            PermissionRule {
                pattern: "src/**".to_string(),
                action: RuleAction::Allow,
                reason: None,
            },
            PermissionRule {
                pattern: "**".to_string(),
                action: RuleAction::Deny,
                reason: Some("deny all".to_string()),
            },
        ];
        let result = evaluate_rules(&rules, "src/foo.rs");
        assert!(matches!(result, Some((RuleAction::Allow, None))));
        let result2 = evaluate_rules(&rules, "other/bar.rs");
        assert!(matches!(result2, Some((RuleAction::Deny, Some(_)))));
    }

    #[test]
    fn evaluate_rules_returns_none_when_no_match() {
        let rules = vec![PermissionRule {
            pattern: "src/*.rs".to_string(),
            action: RuleAction::Allow,
            reason: None,
        }];
        assert!(evaluate_rules(&rules, "tests/foo.rs").is_none());
    }

    #[test]
    fn evaluate_rules_empty_list_returns_none() {
        assert!(evaluate_rules(&[], "anything").is_none());
    }

    // ── ProviderType serde ───────────────────────────────────────────────────

    #[test]
    fn provider_type_serialization() {
        assert_eq!(
            serde_json::to_string(&ProviderType::Anthropic).unwrap(),
            "\"anthropic\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderType::OpenAI).unwrap(),
            "\"openai\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderType::Kimi).unwrap(),
            "\"kimi\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderType::KimiCode).unwrap(),
            "\"kimi-code\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderType::Local).unwrap(),
            "\"local\""
        );
    }

    #[test]
    fn provider_type_deserialization() {
        let v: ProviderType = serde_json::from_str("\"kimi\"").unwrap();
        assert_eq!(v, ProviderType::Kimi);
        let v2: ProviderType = serde_json::from_str("\"kimi-code\"").unwrap();
        assert_eq!(v2, ProviderType::KimiCode);
        let v3: ProviderType = serde_json::from_str("\"anthropic\"").unwrap();
        assert_eq!(v3, ProviderType::Anthropic);
    }

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

    // ── Direct glob_match tests ─────────────────────────────────────────────

    #[test]
    fn glob_match_star_wildcard() {
        assert!(glob_match("src/*.rs", "src/main.rs"));
        assert!(glob_match("src/*.rs", "src/lib.rs"));
        // * does not cross directory boundaries
        assert!(!glob_match("src/*.rs", "src/sub/lib.rs"));
        // * matches empty
        assert!(glob_match("*.rs", "lib.rs"));
    }

    #[test]
    fn glob_match_double_star_wildcard() {
        assert!(glob_match("src/**", "src/main.rs"));
        assert!(glob_match("src/**", "src/a/b/c.rs"));
        assert!(glob_match("**/*.rs", "deep/nested/file.rs"));
        assert!(glob_match("**", "anything/at/all"));
        // ** at the start
        assert!(glob_match("**/config.rs", "src/config.rs"));
        assert!(glob_match("**/config.rs", "a/b/c/config.rs"));
    }

    #[test]
    fn glob_match_exact_path() {
        assert!(glob_match("src/main.rs", "src/main.rs"));
        assert!(!glob_match("src/main.rs", "src/lib.rs"));
        assert!(!glob_match("src/main.rs", "tests/main.rs"));
    }

    // ── evaluate_rules dedicated deny test ──────────────────────────────────

    #[test]
    fn evaluate_rules_deny_rule_match() {
        let rules = vec![PermissionRule {
            pattern: "secrets/**".to_string(),
            action: RuleAction::Deny,
            reason: Some("sensitive area".to_string()),
        }];
        let result = evaluate_rules(&rules, "secrets/key.pem");
        assert_eq!(
            result,
            Some((RuleAction::Deny, Some("sensitive area".to_string())))
        );
    }

    #[test]
    fn evaluate_rules_allow_rule_match() {
        let rules = vec![PermissionRule {
            pattern: "docs/**".to_string(),
            action: RuleAction::Allow,
            reason: None,
        }];
        let result = evaluate_rules(&rules, "docs/README.md");
        assert_eq!(result, Some((RuleAction::Allow, None)));
    }

    // ── RuleAction Copy trait ───────────────────────────────────────────────

    #[test]
    fn rule_action_is_copy() {
        let a = RuleAction::Allow;
        let b = a; // Copy
        #[allow(clippy::clone_on_copy)]
        let c = a.clone(); // Clone
        assert_eq!(a, b);
        assert_eq!(a, c);
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
