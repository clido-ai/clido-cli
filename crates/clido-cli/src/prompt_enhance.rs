//! Auto Prompt Enhancement — appends active project rules to user prompts for
//! coding tasks.
//!
//! # What it does
//!
//! When mode is `Auto`, `enhance_prompt()` appends active project rules (style
//! preferences, architecture patterns, etc.) to prompts that look like coding
//! tasks.  Informational requests ("show me X", "what is Y", questions) are
//! always passed through unchanged.
//!
//! No quality-instruction suffix is added to user messages — generic boilerplate
//! like "provide a production-ready solution" belongs in the system prompt, not
//! in user turns, because models may echo it back to the user.
//!
//! Mode `Off` returns the original message unchanged.
//!
//! No extra LLM call is made — transformation is pure Rust and runs instantly.
//!
//! # Config files
//!
//! Mode is persisted in:
//!   - Global:  `~/.config/clido/prompt-settings.json`
//!   - Project: `{workspace}/.clido/prompt-settings.json`  (overrides global)
//!
//! Rules are stored in:
//!   - Global:  `~/.config/clido/prompt-rules.json`
//!   - Project: `{workspace}/.clido/prompt-rules.json`  (merged, project wins on id conflict)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Mode ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    /// Automatically enhance prompts (default).
    #[default]
    Auto,
    /// Use raw user input unchanged.
    Off,
}

impl PromptMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PromptMode::Auto => "auto",
            PromptMode::Off => "off",
        }
    }
}

impl std::fmt::Display for PromptMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Settings file ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptSettings {
    pub mode: PromptMode,
}

// ── Rule entry ────────────────────────────────────────────────────────────────

/// A single learnable constraint applied during prompt enhancement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleEntry {
    /// Stable identifier (kebab-case).
    pub id: String,
    /// Human-readable constraint text appended as a requirement.
    pub text: String,
    /// Confidence 0.0–1.0.  Rules below 0.5 are stored but not applied.
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    /// How the rule was created: "manual" | "inferred".
    #[serde(default = "default_source")]
    pub source: String,
    /// How many times this pattern was observed in user turns.
    #[serde(default)]
    pub observation_count: u32,
}

fn default_confidence() -> f64 {
    1.0
}
fn default_source() -> String {
    "manual".to_string()
}

impl RuleEntry {
    pub fn new_manual(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            confidence: 1.0,
            source: "manual".to_string(),
            observation_count: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.confidence >= 0.5
    }
}

// ── Rules store ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptRules {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub rules: Vec<RuleEntry>,
    /// Frequency table for rule evolution: phrase → observed count.
    #[serde(default)]
    pub phrase_counts: HashMap<String, u32>,
}

impl PromptRules {
    /// Rules that are active (confidence ≥ 0.5), deduplicated by id.
    pub fn active_rules(&self) -> Vec<&RuleEntry> {
        let mut seen = std::collections::HashSet::new();
        self.rules
            .iter()
            .filter(|r| r.is_active() && seen.insert(r.id.clone()))
            .collect()
    }

    /// Add or update a rule by id.
    pub fn upsert(&mut self, entry: RuleEntry) {
        if let Some(existing) = self.rules.iter_mut().find(|r| r.id == entry.id) {
            *existing = entry;
        } else {
            self.rules.push(entry);
        }
    }

    /// Remove a rule by id.  Returns true if removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| r.id != id);
        self.rules.len() < before
    }

    /// Observe a user turn and potentially promote inferred rules.
    /// Returns a list of newly promoted rule texts (for UI feedback).
    pub fn observe_turn(&mut self, user_message: &str) -> Vec<String> {
        let patterns = extract_instruction_patterns(user_message);
        let mut promoted = Vec::new();

        for phrase in patterns {
            let count = self.phrase_counts.entry(phrase.clone()).or_insert(0);
            *count += 1;

            // Promote to inferred rule after 3 observations if not already present.
            if *count == 3 {
                let id = phrase_to_id(&phrase);
                if !self.rules.iter().any(|r| r.id == id) {
                    self.rules.push(RuleEntry {
                        id,
                        text: phrase.clone(),
                        confidence: 0.6,
                        source: "inferred".to_string(),
                        observation_count: 3,
                    });
                    promoted.push(phrase);
                }
            } else if let Some(rule) = self
                .rules
                .iter_mut()
                .find(|r| r.source == "inferred" && r.id == phrase_to_id(&phrase))
            {
                rule.observation_count += 1;
                // Increase confidence (capped at 0.9 for inferred rules).
                rule.confidence = (rule.confidence + 0.05).min(0.9);
            }
        }

        promoted
    }
}

// ── Enhancement context ───────────────────────────────────────────────────────

/// Everything the enhancer needs at call time.
pub struct EnhancementCtx<'a> {
    pub mode: PromptMode,
    pub rules: &'a PromptRules,
}

// ── Core enhancement function ─────────────────────────────────────────────────

/// Transform a raw user prompt into a structured, high-quality prompt.
///
/// Returns `(enhanced_text, was_modified)`.  If `was_modified` is false, the
/// returned text is identical to the input (nothing was changed).
pub fn enhance_prompt(raw: &str, ctx: &EnhancementCtx<'_>) -> (String, bool) {
    if ctx.mode == PromptMode::Off || raw.trim().is_empty() {
        return (raw.to_string(), false);
    }

    let trimmed = raw.trim();
    let active_rules = ctx.rules.active_rules();

    // No rules to apply → nothing to do.
    if active_rules.is_empty() {
        return (raw.to_string(), false);
    }

    // Only apply rules to prompts that look like coding/modification tasks.
    // Informational, read, and question prompts are always passed through
    // unchanged to avoid injecting irrelevant constraints.
    if !looks_like_coding_task(trimmed) {
        return (raw.to_string(), false);
    }

    // Append active project rules as explicit constraints.
    let rule_lines: Vec<String> = active_rules
        .iter()
        .map(|r| format!("- {}", r.text))
        .collect();
    let enhanced = format!(
        "{}\n\nAdditional requirements:\n{}",
        trimmed,
        rule_lines.join("\n")
    );
    let was_modified = enhanced != raw;
    (enhanced, was_modified)
}

/// Returns true when the prompt looks like a coding or modification task rather
/// than an informational / read / question request.  Used to gate rule injection
/// so that prompts like "show me config.toml" are never altered.
fn looks_like_coding_task(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();

    // Questions are informational.
    if lower.contains('?') {
        return false;
    }

    // Common read/info prefixes — pass through unchanged.
    let read_prefixes = [
        "show ",
        "show\n",
        "what ",
        "how ",
        "why ",
        "where ",
        "when ",
        "list ",
        "find ",
        "display ",
        "print ",
        "read ",
        "open ",
        "view ",
        "explain ",
        "describe ",
        "tell ",
        "can you show",
        "can you tell",
        "cat ",
        "ls ",
        "grep ",
    ];
    for prefix in &read_prefixes {
        if lower.starts_with(prefix) {
            return false;
        }
    }

    // Coding action keywords — enhance these.
    let coding_keywords = [
        "implement",
        "create",
        "add ",
        "add\n",
        "fix ",
        "fix\n",
        "build",
        "write ",
        "write\n",
        "refactor",
        "update ",
        "update\n",
        "change ",
        "change\n",
        "modify",
        "generate",
        "make ",
        "make\n",
        "delete ",
        "remove ",
        "rename ",
        "move ",
        "migrate",
        "convert",
        "optimize",
        "improve",
        "extend",
        "replace",
        "rewrite",
    ];
    for kw in &coding_keywords {
        if lower.contains(kw) {
            return true;
        }
    }

    // Long multi-line prompts are likely detailed coding instructions.
    if prompt.contains('\n') && prompt.split_whitespace().count() > 20 {
        return true;
    }

    false
}

// ── Persistence ───────────────────────────────────────────────────────────────

/// Path to the global prompt settings file.
pub fn global_settings_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CLIDO_CONFIG") {
        let base = PathBuf::from(p);
        let dir = if base.is_file() {
            base.parent()?.to_path_buf()
        } else {
            base
        };
        return Some(dir.join("prompt-settings.json"));
    }
    directories::ProjectDirs::from("", "", "clido")
        .map(|d| d.config_dir().join("prompt-settings.json"))
}

/// Path to the project prompt settings file (inside `{workspace}/.clido/`).
pub fn project_settings_path(workspace: &Path) -> PathBuf {
    workspace.join(".clido").join("prompt-settings.json")
}

/// Path to the global prompt rules file.
pub fn global_rules_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CLIDO_CONFIG") {
        let base = PathBuf::from(p);
        let dir = if base.is_file() {
            base.parent()?.to_path_buf()
        } else {
            base
        };
        return Some(dir.join("prompt-rules.json"));
    }
    directories::ProjectDirs::from("", "", "clido")
        .map(|d| d.config_dir().join("prompt-rules.json"))
}

/// Path to the project-level prompt rules file.
pub fn project_rules_path(workspace: &Path) -> PathBuf {
    workspace.join(".clido").join("prompt-rules.json")
}

/// Load the effective prompt mode (project overrides global).
pub fn load_prompt_mode(workspace: &Path) -> PromptMode {
    // Project-level takes priority.
    let project = project_settings_path(workspace);
    if project.exists() {
        if let Ok(content) = std::fs::read_to_string(&project) {
            if let Ok(s) = serde_json::from_str::<PromptSettings>(&content) {
                return s.mode;
            }
        }
    }
    // Fallback to global.
    if let Some(global) = global_settings_path() {
        if global.exists() {
            if let Ok(content) = std::fs::read_to_string(&global) {
                if let Ok(s) = serde_json::from_str::<PromptSettings>(&content) {
                    return s.mode;
                }
            }
        }
    }
    PromptMode::Auto
}

/// Persist prompt mode.  Writes to `path` (create parent dirs as needed).
pub fn save_prompt_mode(path: &Path, mode: PromptMode) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let settings = PromptSettings { mode };
    let json = serde_json::to_string_pretty(&settings)?;
    std::fs::write(path, json)
}

/// Load merged rules: global + project (project wins on id collision).
pub fn load_rules(workspace: &Path) -> PromptRules {
    let mut merged = PromptRules::default();

    // Load global rules first.
    if let Some(global_path) = global_rules_path() {
        if global_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&global_path) {
                if let Ok(r) = serde_json::from_str::<PromptRules>(&content) {
                    merged = r;
                }
            }
        }
    }

    // Layer project rules on top (project rules override by id).
    let project_path = project_rules_path(workspace);
    if project_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&project_path) {
            if let Ok(project_rules) = serde_json::from_str::<PromptRules>(&content) {
                for rule in project_rules.rules {
                    merged.upsert(rule);
                }
                // Merge phrase counts.
                for (k, v) in project_rules.phrase_counts {
                    *merged.phrase_counts.entry(k).or_insert(0) += v;
                }
            }
        }
    }

    merged.version = 1;
    merged
}

/// Persist rules to `path` (creates parent dirs if needed).
pub fn save_rules(path: &Path, rules: &PromptRules) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(rules)?;
    std::fs::write(path, json)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Extract recurring instruction phrases from a user message.
/// These are short imperative clauses that carry explicit requirements.
fn extract_instruction_patterns(msg: &str) -> Vec<String> {
    let lower = msg.to_lowercase();
    let mut patterns = Vec::new();

    // Patterns that suggest a reusable coding preference.
    let triggers: &[(&str, &str)] = &[
        ("always add tests", "Always add tests for new code"),
        ("add tests", "Always add tests for new code"),
        ("write tests", "Always add tests for new code"),
        (
            "don't use unwrap",
            "Avoid .unwrap() — use proper error handling",
        ),
        (
            "avoid unwrap",
            "Avoid .unwrap() — use proper error handling",
        ),
        ("no unwrap", "Avoid .unwrap() — use proper error handling"),
        (
            "add documentation",
            "Add documentation comments to public items",
        ),
        (
            "add doc comments",
            "Add documentation comments to public items",
        ),
        (
            "follow existing style",
            "Follow the existing code style and naming conventions",
        ),
        (
            "match existing style",
            "Follow the existing code style and naming conventions",
        ),
        (
            "keep it minimal",
            "Keep changes minimal — avoid unrelated modifications",
        ),
        (
            "minimal changes",
            "Keep changes minimal — avoid unrelated modifications",
        ),
        (
            "no breaking changes",
            "Do not introduce breaking changes to public APIs",
        ),
    ];

    for (trigger, canonical) in triggers {
        if lower.contains(trigger) {
            patterns.push(canonical.to_string());
        }
    }

    patterns
}

/// Convert a phrase to a stable kebab-case rule id.
fn phrase_to_id(phrase: &str) -> String {
    phrase
        .to_lowercase()
        .split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_auto(rules: &PromptRules) -> EnhancementCtx<'_> {
        EnhancementCtx {
            mode: PromptMode::Auto,
            rules,
        }
    }
    fn ctx_off(rules: &PromptRules) -> EnhancementCtx<'_> {
        EnhancementCtx {
            mode: PromptMode::Off,
            rules,
        }
    }
    fn empty_rules() -> PromptRules {
        PromptRules::default()
    }

    #[test]
    fn mode_off_passes_through_unchanged() {
        let rules = empty_rules();
        let (out, modified) = enhance_prompt("fix the bug", &ctx_off(&rules));
        assert_eq!(out, "fix the bug");
        assert!(!modified);
    }

    #[test]
    fn empty_prompt_passes_through() {
        let rules = empty_rules();
        let (out, modified) = enhance_prompt("", &ctx_auto(&rules));
        assert!(!modified);
        assert_eq!(out, "");
    }

    #[test]
    fn short_coding_prompt_with_rules_gets_enhanced() {
        let mut rules = empty_rules();
        rules.upsert(RuleEntry::new_manual(
            "no-unwrap",
            "Avoid .unwrap() in all new code",
        ));
        let (out, modified) = enhance_prompt("fix the login bug", &ctx_auto(&rules));
        assert!(modified, "coding prompt with rules should be modified");
        assert!(out.contains("fix the login bug"), "original text preserved");
        assert!(out.contains("Additional requirements:"), "rules appended");
        // No quality boilerplate — that belongs in system prompt
        assert!(!out.contains("production-ready"));
    }

    #[test]
    fn short_coding_prompt_no_rules_passes_through() {
        // Without rules there is nothing to inject.
        let rules = empty_rules();
        let (out, modified) = enhance_prompt("fix the login bug", &ctx_auto(&rules));
        assert!(!modified, "no rules → no modification");
        assert_eq!(out, "fix the login bug");
    }

    #[test]
    fn already_detailed_prompt_no_rules_passes_through() {
        let rules = empty_rules();
        let long = "Please refactor the authentication module to use JWT tokens instead of \
                    session cookies. Ensure backward compatibility with existing tests, update \
                    the documentation, and add integration tests for the new flow.";
        let (out, modified) = enhance_prompt(long, &ctx_auto(&rules));
        assert!(
            !modified,
            "detailed prompt with no rules should not be modified"
        );
        assert_eq!(out, long);
    }

    #[test]
    fn rules_appended_to_short_prompt() {
        let mut rules = empty_rules();
        rules.upsert(RuleEntry::new_manual(
            "no-unwrap",
            "Avoid .unwrap() in all new code",
        ));
        let (out, modified) = enhance_prompt("add a config parser", &ctx_auto(&rules));
        assert!(modified);
        assert!(out.contains("Additional requirements:"));
        assert!(out.contains("Avoid .unwrap()"));
    }

    #[test]
    fn rules_appended_even_to_detailed_prompt() {
        let mut rules = empty_rules();
        rules.upsert(RuleEntry::new_manual(
            "no-unwrap",
            "Avoid .unwrap() in all new code",
        ));
        let long = "Please refactor the authentication module to use JWT tokens instead of \
                    session cookies. Ensure backward compatibility and update the documentation \
                    and add integration tests for the new flow.";
        let (out, modified) = enhance_prompt(long, &ctx_auto(&rules));
        assert!(
            modified,
            "rule should cause modification even on detailed prompt"
        );
        assert!(out.contains("Additional requirements:"));
    }

    #[test]
    fn inactive_rule_not_applied() {
        let mut rules = empty_rules();
        rules.upsert(RuleEntry {
            id: "low-conf".to_string(),
            text: "Some low confidence rule".to_string(),
            confidence: 0.3,
            source: "inferred".to_string(),
            observation_count: 1,
        });
        // Inactive rule (confidence < 0.5) must never appear in output.
        let (out, _) = enhance_prompt("fix the bug", &ctx_auto(&rules));
        assert!(!out.contains("Some low confidence rule"));
    }

    #[test]
    fn original_text_always_preserved() {
        let mut rules = empty_rules();
        rules.upsert(RuleEntry::new_manual("test-rule", "Always write tests"));
        let raw = "refactor database layer";
        let (out, _) = enhance_prompt(raw, &ctx_auto(&rules));
        assert!(out.starts_with(raw), "raw message must appear at start");
    }

    #[test]
    fn question_prompt_not_enhanced() {
        let mut rules = empty_rules();
        rules.upsert(RuleEntry::new_manual("test-rule", "Always write tests"));
        let (out, modified) = enhance_prompt("what does this function do?", &ctx_auto(&rules));
        // Questions are informational — rules should not be injected.
        assert!(!modified, "questions should not be modified");
        assert_eq!(out, "what does this function do?");
    }

    #[test]
    fn read_prompt_not_enhanced() {
        let mut rules = empty_rules();
        rules.upsert(RuleEntry::new_manual("test-rule", "Always write tests"));
        let (out, modified) = enhance_prompt("show me /tmp/config.toml", &ctx_auto(&rules));
        assert!(!modified, "read prompts should not be modified");
        assert_eq!(out, "show me /tmp/config.toml");
    }

    #[test]
    fn rule_evolution_promotes_after_threshold() {
        let mut rules = PromptRules::default();
        let msg = "please add tests for the new code";
        rules.observe_turn(msg);
        rules.observe_turn(msg);
        assert!(rules.rules.is_empty(), "should not promote at count 2");
        let promoted = rules.observe_turn(msg);
        assert!(!promoted.is_empty(), "should promote at count 3");
        assert!(rules.rules.iter().any(|r| r.source == "inferred"));
    }

    #[test]
    fn rule_evolution_no_duplicate_ids() {
        let mut rules = PromptRules::default();
        let msg = "add tests";
        for _ in 0..5 {
            rules.observe_turn(msg);
        }
        let inferred: Vec<_> = rules
            .rules
            .iter()
            .filter(|r| r.source == "inferred")
            .collect();
        // Should not have duplicate ids.
        let ids: Vec<_> = inferred.iter().map(|r| &r.id).collect();
        let unique_ids: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique_ids.len(), "no duplicate rule ids");
    }

    #[test]
    fn phrase_to_id_is_stable() {
        let id1 = phrase_to_id("Always add tests for new code");
        let id2 = phrase_to_id("Always add tests for new code");
        assert_eq!(id1, id2);
        // Should be lowercase kebab.
        assert!(id1
            .chars()
            .all(|c| c.is_lowercase() || c == '-' || c.is_numeric()));
    }

    #[test]
    fn mode_roundtrip_serde() {
        let modes = [PromptMode::Auto, PromptMode::Off];
        for mode in modes {
            let s = serde_json::to_string(&mode).unwrap();
            let back: PromptMode = serde_json::from_str(&s).unwrap();
            assert_eq!(mode, back);
        }
    }

    #[test]
    fn settings_persist_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prompt-settings.json");
        save_prompt_mode(&path, PromptMode::Off).unwrap();
        let loaded_mode = {
            let content = std::fs::read_to_string(&path).unwrap();
            let s: PromptSettings = serde_json::from_str(&content).unwrap();
            s.mode
        };
        assert_eq!(loaded_mode, PromptMode::Off);
    }

    #[test]
    fn rules_persist_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prompt-rules.json");
        let mut rules = PromptRules::default();
        rules.upsert(RuleEntry::new_manual("test-rule", "Always write tests"));
        save_rules(&path, &rules).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let loaded: PromptRules = serde_json::from_str(&content).unwrap();
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].id, "test-rule");
    }
}
