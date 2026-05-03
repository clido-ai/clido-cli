//! Workflow definition types (YAML schema).

use serde::{Deserialize, Serialize};

/// Top-level workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WorkflowDef {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inputs: Vec<InputDef>,
    pub steps: Vec<StepDef>,
    #[serde(default)]
    pub output: Option<OutputConfig>,
    #[serde(default)]
    pub prerequisites: Option<PrerequisitesDef>,
}

/// Input parameter definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InputDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Optional hint for the UI. Supported values: `"path"` (file path picker),
    /// `"dir"` (directory picker). If omitted, the UI infers from the field name.
    #[serde(default)]
    pub hint: Option<String>,
}

/// Single step definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StepDef {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    /// None = all tools, Some(vec![]) = no tools, Some(names) = allowlist.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    pub prompt: String,
    #[serde(default)]
    pub outputs: Vec<OutputDef>,
    #[serde(default)]
    pub on_error: OnErrorPolicy,
    #[serde(default)]
    pub retry: Option<RetryConfig>,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Optional iteration: Tera expression that evaluates to a JSON array or
    /// newline-delimited list. The step is run once per item. Each iteration
    /// receives the item as `{{ item }}` (or the name from `foreach_var`).
    #[serde(default)]
    pub foreach: Option<String>,
    /// Variable name injected per foreach iteration. Default: "item".
    #[serde(default)]
    pub foreach_var: Option<String>,
}

/// Output binding for a step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OutputDef {
    pub name: String,
    #[serde(default)]
    pub r#type: String, // "text" for V1
    #[serde(default)]
    pub save_to: Option<String>, // Tera template for path
}

/// On-error policy for a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnErrorPolicy {
    #[default]
    Fail,
    Continue,
    Retry,
}

/// Retry configuration (only valid when on_error: retry).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RetryConfig {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub backoff: BackoffKind,
}

fn default_max_attempts() -> u32 {
    3
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackoffKind {
    #[default]
    None,
    Linear,
    Exponential,
}

/// Top-level output config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OutputConfig {
    #[serde(default)]
    pub print_summary: bool,
}

/// Prerequisites (commands, env) for pre-flight.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PrerequisitesDef {
    #[serde(default)]
    pub commands: Vec<PrereqEntry>,
    #[serde(default)]
    pub env: Vec<PrereqEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PrereqEntry {
    Required(String),
    Optional { name: String, optional: bool },
}

impl PrereqEntry {
    pub fn name(&self) -> &str {
        match self {
            PrereqEntry::Required(s) => s.as_str(),
            PrereqEntry::Optional { name, .. } => name.as_str(),
        }
    }
    pub fn optional(&self) -> bool {
        match self {
            PrereqEntry::Required(_) => false,
            PrereqEntry::Optional { optional, .. } => *optional,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── WorkflowDef construction and defaults ──────────────────────────────

    #[test]
    fn workflow_def_minimal_roundtrip() {
        let yaml = r#"
name: test_workflow
steps:
  - id: step1
    prompt: "Do something"
"#;
        let def: WorkflowDef = serde_yaml::from_str(yaml).expect("parse failed");
        assert_eq!(def.name, "test_workflow");
        assert_eq!(def.steps.len(), 1);
        assert_eq!(def.steps[0].id, "step1");
        assert_eq!(def.steps[0].prompt, "Do something");
        // Defaults
        assert!(def.version.is_empty());
        assert!(def.description.is_empty());
        assert!(def.inputs.is_empty());
        assert!(def.output.is_none());
        assert!(def.prerequisites.is_none());
    }

    #[test]
    fn step_def_defaults() {
        let yaml = r#"
id: s1
prompt: "hello"
"#;
        let step: StepDef = serde_yaml::from_str(yaml).expect("parse failed");
        assert_eq!(step.id, "s1");
        assert!(step.name.is_none());
        assert!(step.profile.is_none());
        assert!(step.tools.is_none());
        assert!(step.outputs.is_empty());
        assert_eq!(step.on_error, OnErrorPolicy::Fail); // default
        assert!(step.retry.is_none());
        assert!(!step.parallel);
        assert!(step.system_prompt.is_none());
        assert!(step.max_turns.is_none());
    }

    // ── OnErrorPolicy default ────────────────────────────────────────────

    #[test]
    fn on_error_policy_default_is_fail() {
        let policy = OnErrorPolicy::default();
        assert_eq!(policy, OnErrorPolicy::Fail);
    }

    #[test]
    fn on_error_policy_roundtrip() {
        let json = serde_json::to_string(&OnErrorPolicy::Continue).unwrap();
        let parsed: OnErrorPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, OnErrorPolicy::Continue);

        let json = serde_json::to_string(&OnErrorPolicy::Retry).unwrap();
        let parsed: OnErrorPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, OnErrorPolicy::Retry);
    }

    // ── BackoffKind default ───────────────────────────────────────────────

    #[test]
    fn backoff_kind_default_is_none() {
        let b = BackoffKind::default();
        assert_eq!(b, BackoffKind::None);
    }

    #[test]
    fn backoff_kind_roundtrip() {
        let json = serde_json::to_string(&BackoffKind::Exponential).unwrap();
        let parsed: BackoffKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, BackoffKind::Exponential);

        let json = serde_json::to_string(&BackoffKind::Linear).unwrap();
        let parsed: BackoffKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, BackoffKind::Linear);
    }

    // ── RetryConfig ───────────────────────────────────────────────────────

    #[test]
    fn retry_config_default_max_attempts() {
        let yaml = r#"backoff: exponential"#;
        let r: RetryConfig = serde_yaml::from_str(yaml).expect("parse failed");
        assert_eq!(r.max_attempts, 3); // default_max_attempts()
        assert_eq!(r.backoff, BackoffKind::Exponential);
    }

    // ── OutputConfig ──────────────────────────────────────────────────────

    #[test]
    fn output_config_default() {
        let c = OutputConfig::default();
        assert!(!c.print_summary);
    }

    #[test]
    fn output_config_roundtrip() {
        let yaml = r#"print_summary: true"#;
        let c: OutputConfig = serde_yaml::from_str(yaml).expect("parse failed");
        assert!(c.print_summary);
    }

    // ── PrerequisitesDef ──────────────────────────────────────────────────

    #[test]
    fn prerequisites_def_default() {
        let p = PrerequisitesDef::default();
        assert!(p.commands.is_empty());
        assert!(p.env.is_empty());
    }

    // ── PrereqEntry ───────────────────────────────────────────────────────

    #[test]
    fn prereq_entry_required_name_and_not_optional() {
        let e = PrereqEntry::Required("cargo".to_string());
        assert_eq!(e.name(), "cargo");
        assert!(!e.optional());
    }

    #[test]
    fn prereq_entry_optional_name_and_optional() {
        let e = PrereqEntry::Optional {
            name: "node".to_string(),
            optional: true,
        };
        assert_eq!(e.name(), "node");
        assert!(e.optional());
    }

    #[test]
    fn prereq_entry_optional_false() {
        let e = PrereqEntry::Optional {
            name: "go".to_string(),
            optional: false,
        };
        assert!(!e.optional());
    }

    #[test]
    fn prereq_entry_required_json_roundtrip() {
        let e = PrereqEntry::Required("git".to_string());
        let json = serde_json::to_string(&e).unwrap();
        let parsed: PrereqEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name(), "git");
        assert!(!parsed.optional());
    }

    // ── InputDef ──────────────────────────────────────────────────────────

    #[test]
    fn input_def_required_and_default() {
        let yaml = r#"
name: my_input
required: true
default: "hello"
"#;
        let i: InputDef = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(i.name, "my_input");
        assert!(i.required);
        assert_eq!(i.default, Some(serde_json::json!("hello")));
    }

    #[test]
    fn input_def_defaults() {
        let yaml = r#"name: x"#;
        let i: InputDef = serde_yaml::from_str(yaml).expect("parse");
        assert!(!i.required);
        assert!(i.default.is_none());
    }

    // ── OutputDef ─────────────────────────────────────────────────────────

    #[test]
    fn output_def_defaults() {
        let yaml = r#"name: result"#;
        let o: OutputDef = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(o.name, "result");
        assert!(o.r#type.is_empty());
        assert!(o.save_to.is_none());
    }

    #[test]
    fn output_def_with_all_fields() {
        let yaml = r#"
name: report
type: text
save_to: "outputs/{{step_id}}.txt"
"#;
        let o: OutputDef = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(o.name, "report");
        assert_eq!(o.r#type, "text");
        assert_eq!(o.save_to.as_deref(), Some("outputs/{{step_id}}.txt"));
    }
}
