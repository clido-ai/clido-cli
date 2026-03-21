//! Load and validate workflow YAML.

use std::path::Path;

use crate::types::{OnErrorPolicy, WorkflowDef};
use clido_core::{ClidoError, Result};

/// Load workflow from YAML file.
pub fn load(path: &Path) -> Result<WorkflowDef> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| ClidoError::Workflow(format!("Failed to read workflow file: {}", e)))?;
    let def: WorkflowDef = serde_yaml::from_str(&s)
        .map_err(|e| ClidoError::Workflow(format!("Invalid workflow YAML: {}", e)))?;
    Ok(def)
}

/// Validate workflow: unique step ids, template refs only to prior steps, retry only when on_error: retry.
pub fn validate(def: &WorkflowDef) -> Result<()> {
    let mut seen_ids = std::collections::HashSet::new();
    for step in &def.steps {
        if !seen_ids.insert(step.id.clone()) {
            return Err(ClidoError::Workflow(format!(
                "Duplicate step id: {}",
                step.id
            )));
        }
        if step.retry.is_some() && step.on_error != OnErrorPolicy::Retry {
            return Err(ClidoError::Workflow(format!(
                "Step '{}': retry config only allowed when on_error: retry",
                step.id
            )));
        }
    }
    // Template refs to prior steps: we can only check at render time; loader doesn't parse template vars.
    Ok(())
}

/// Result of a preflight check.
#[derive(Debug, Clone, PartialEq)]
pub enum PreflightStatus {
    Pass,
    Warn(String),
    Fail(String),
}

/// A single preflight check result.
#[derive(Debug, Clone)]
pub struct PreflightCheck {
    pub name: String,
    pub status: PreflightStatus,
}

/// Overall result of running all preflight checks.
#[derive(Debug, Clone)]
pub struct PreflightResult {
    pub checks: Vec<PreflightCheck>,
}

impl PreflightResult {
    /// Returns true if no check is Fail.
    pub fn is_ok(&self) -> bool {
        self.checks
            .iter()
            .all(|c| !matches!(c.status, PreflightStatus::Fail(_)))
    }
}

/// Run preflight on a workflow:
/// 1. Validates the workflow (unique step ids, retry rules).
/// 2. Checks that required profiles exist in the provided profile list.
/// 3. Checks that step tools are in the provided tool list.
/// Returns a PreflightResult with a pass/warn/fail per check.
pub fn preflight(
    def: &WorkflowDef,
    available_profiles: &[&str],
    available_tools: &[&str],
) -> PreflightResult {
    let mut checks = Vec::new();

    // Check 1: basic validation
    match validate(def) {
        Ok(()) => checks.push(PreflightCheck {
            name: "validate".to_string(),
            status: PreflightStatus::Pass,
        }),
        Err(e) => checks.push(PreflightCheck {
            name: "validate".to_string(),
            status: PreflightStatus::Fail(e.to_string()),
        }),
    }

    // Check 2: profiles
    let (required_tools, required_profiles) = required_tools_and_profiles(def);
    for profile in &required_profiles {
        if available_profiles.contains(&profile.as_str()) {
            checks.push(PreflightCheck {
                name: format!("profile:{}", profile),
                status: PreflightStatus::Pass,
            });
        } else {
            checks.push(PreflightCheck {
                name: format!("profile:{}", profile),
                status: PreflightStatus::Fail(format!(
                    "Profile '{}' not found in config",
                    profile
                )),
            });
        }
    }

    // Check 3: tools (warn if unknown, since tools may be dynamic)
    for tool in &required_tools {
        if available_tools.is_empty() || available_tools.contains(&tool.as_str()) {
            checks.push(PreflightCheck {
                name: format!("tool:{}", tool),
                status: PreflightStatus::Pass,
            });
        } else {
            checks.push(PreflightCheck {
                name: format!("tool:{}", tool),
                status: PreflightStatus::Warn(format!(
                    "Tool '{}' not found in registry (may be dynamic)",
                    tool
                )),
            });
        }
    }

    if checks.is_empty() {
        checks.push(PreflightCheck {
            name: "preflight".to_string(),
            status: PreflightStatus::Pass,
        });
    }

    PreflightResult { checks }
}

/// Return union of all tool names and all profile names referenced by steps.
pub fn required_tools_and_profiles(def: &WorkflowDef) -> (Vec<String>, Vec<String>) {
    let mut tools = std::collections::HashSet::new();
    let mut profiles = std::collections::HashSet::new();
    for step in &def.steps {
        if let Some(ref p) = step.profile {
            profiles.insert(p.clone());
        }
        if let Some(ref t) = step.tools {
            for name in t {
                tools.insert(name.clone());
            }
        }
    }
    (tools.into_iter().collect(), profiles.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{OnErrorPolicy, RetryConfig, StepDef};
    use std::io::Write;

    fn valid_yaml() -> String {
        r#"
name: test
version: "1"
steps:
  - id: a
    prompt: "Hello"
  - id: b
    prompt: "World"
"#
        .to_string()
    }

    #[test]
    fn load_valid() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(valid_yaml().as_bytes()).unwrap();
        f.flush().unwrap();
        let def = load(f.path()).unwrap();
        assert_eq!(def.name, "test");
        assert_eq!(def.steps.len(), 2);
        assert_eq!(def.steps[0].id, "a");
        validate(&def).unwrap();
    }

    #[test]
    fn validate_duplicate_step_id() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![
                StepDef {
                    id: "a".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "p".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: false,
                    system_prompt: None,
                    max_turns: None,
                },
                StepDef {
                    id: "a".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "q".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: false,
                    system_prompt: None,
                    max_turns: None,
                },
            ],
            output: None,
            prerequisites: None,
        };
        let err = validate(&def).unwrap_err();
        assert!(err.to_string().contains("Duplicate step id"));
    }

    #[test]
    fn preflight_pass() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(valid_yaml().as_bytes()).unwrap();
        f.flush().unwrap();
        let def = load(f.path()).unwrap();
        let result = preflight(&def, &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn preflight_fail_missing_profile() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "a".into(),
                name: None,
                profile: Some("nonexistent".into()),
                tools: None,
                prompt: "p".into(),
                outputs: vec![],
                on_error: OnErrorPolicy::Fail,
                retry: None,
                parallel: false,
                system_prompt: None,
                max_turns: None,
            }],
            output: None,
            prerequisites: None,
        };
        let result = preflight(&def, &["default"], &[]);
        let profile_check = result
            .checks
            .iter()
            .find(|c| c.name == "profile:nonexistent");
        assert!(profile_check.is_some());
        assert!(matches!(
            profile_check.unwrap().status,
            PreflightStatus::Fail(_)
        ));
        assert!(!result.is_ok());
    }

    #[test]
    fn preflight_warn_unknown_tool() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "a".into(),
                name: None,
                profile: None,
                tools: Some(vec!["UnknownTool".into()]),
                prompt: "p".into(),
                outputs: vec![],
                on_error: OnErrorPolicy::Fail,
                retry: None,
                parallel: false,
                system_prompt: None,
                max_turns: None,
            }],
            output: None,
            prerequisites: None,
        };
        let result = preflight(&def, &[], &["Read", "Write"]);
        let tool_check = result.checks.iter().find(|c| c.name == "tool:UnknownTool");
        assert!(tool_check.is_some());
        assert!(matches!(
            tool_check.unwrap().status,
            PreflightStatus::Warn(_)
        ));
        // Warn doesn't fail the preflight
        assert!(result.is_ok());
    }

    #[test]
    fn validate_retry_without_on_error_retry() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "a".into(),
                name: None,
                profile: None,
                tools: None,
                prompt: "p".into(),
                outputs: vec![],
                on_error: OnErrorPolicy::Fail,
                retry: Some(RetryConfig {
                    max_attempts: 2,
                    backoff: crate::types::BackoffKind::None,
                }),
                parallel: false,
                system_prompt: None,
                max_turns: None,
            }],
            output: None,
            prerequisites: None,
        };
        let err = validate(&def).unwrap_err();
        assert!(err
            .to_string()
            .contains("retry config only allowed when on_error: retry"));
    }
}
