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
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
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
