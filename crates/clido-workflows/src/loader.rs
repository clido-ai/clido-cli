//! Load and validate workflow YAML.

use std::path::Path;

use crate::types::{OnErrorPolicy, WorkflowDef};
use clido_core::{ClidoError, Result};

/// Check whether a command exists somewhere in PATH.
fn command_exists(cmd: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(sep) {
            let mut candidate = std::path::PathBuf::from(dir);
            candidate.push(cmd);
            if candidate.exists() {
                return true;
            }
        }
    }
    false
}

/// Enforce prerequisites declared in the workflow: required env vars and commands.
/// Optional entries are skipped; missing required entries return an error.
pub fn check_prerequisites(def: &WorkflowDef) -> Result<()> {
    let Some(ref prereqs) = def.prerequisites else {
        return Ok(());
    };
    for entry in &prereqs.env {
        if !entry.optional() && std::env::var(entry.name()).is_err() {
            return Err(ClidoError::Workflow(format!(
                "Missing required environment variable: {}",
                entry.name()
            )));
        }
    }
    for entry in &prereqs.commands {
        if !entry.optional() && !command_exists(entry.name()) {
            return Err(ClidoError::Workflow(format!(
                "Required command not found in PATH: {}",
                entry.name()
            )));
        }
    }
    Ok(())
}

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
///
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
                status: PreflightStatus::Fail(format!("Profile '{}' not found in config", profile)),
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
                    foreach: None,
                    foreach_var: None,
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
                    foreach: None,
                    foreach_var: None,
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
                foreach: None,
                foreach_var: None,
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
                foreach: None,
                foreach_var: None,
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
                foreach: None,
                foreach_var: None,
            }],
            output: None,
            prerequisites: None,
        };
        let err = validate(&def).unwrap_err();
        assert!(err
            .to_string()
            .contains("retry config only allowed when on_error: retry"));
    }

    // ── additional coverage ────────────────────────────────────────────────

    #[test]
    fn load_invalid_yaml_returns_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"invalid: yaml: [unclosed").unwrap();
        f.flush().unwrap();
        let result = load(f.path());
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("Invalid workflow YAML"));
    }

    #[test]
    fn load_nonexistent_file_returns_error() {
        let result = load(std::path::Path::new("/nonexistent_file_xyz_12345.yaml"));
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("Failed to read"));
    }

    #[test]
    fn preflight_pass_with_known_profile() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "a".into(),
                name: None,
                profile: Some("default".into()),
                tools: None,
                prompt: "p".into(),
                outputs: vec![],
                on_error: OnErrorPolicy::Fail,
                retry: None,
                parallel: false,
                system_prompt: None,
                max_turns: None,
                foreach: None,
                foreach_var: None,
            }],
            output: None,
            prerequisites: None,
        };
        let result = preflight(&def, &["default"], &[]);
        let profile_check = result.checks.iter().find(|c| c.name == "profile:default");
        assert!(profile_check.is_some());
        assert!(matches!(
            profile_check.unwrap().status,
            PreflightStatus::Pass
        ));
        assert!(result.is_ok());
    }

    #[test]
    fn preflight_with_known_tool_passes() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "a".into(),
                name: None,
                profile: None,
                tools: Some(vec!["Read".into()]),
                prompt: "p".into(),
                outputs: vec![],
                on_error: OnErrorPolicy::Fail,
                retry: None,
                parallel: false,
                system_prompt: None,
                max_turns: None,
                foreach: None,
                foreach_var: None,
            }],
            output: None,
            prerequisites: None,
        };
        let result = preflight(&def, &[], &["Read", "Write"]);
        let tool_check = result.checks.iter().find(|c| c.name == "tool:Read");
        assert!(tool_check.is_some());
        assert!(matches!(tool_check.unwrap().status, PreflightStatus::Pass));
    }

    #[test]
    fn preflight_invalid_def_returns_fail() {
        // Def with duplicate ids → validate fails → preflight returns Fail
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![
                StepDef {
                    id: "dup".into(),
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
                    foreach: None,
                    foreach_var: None,
                },
                StepDef {
                    id: "dup".into(),
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
                    foreach: None,
                    foreach_var: None,
                },
            ],
            output: None,
            prerequisites: None,
        };
        let result = preflight(&def, &[], &[]);
        assert!(!result.is_ok());
        let validate_check = result.checks.iter().find(|c| c.name == "validate");
        assert!(matches!(
            validate_check.unwrap().status,
            PreflightStatus::Fail(_)
        ));
    }

    #[test]
    fn required_tools_and_profiles_no_entries() {
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
                retry: None,
                parallel: false,
                system_prompt: None,
                max_turns: None,
                foreach: None,
                foreach_var: None,
            }],
            output: None,
            prerequisites: None,
        };
        let (tools, profiles) = required_tools_and_profiles(&def);
        assert!(tools.is_empty());
        assert!(profiles.is_empty());
    }

    #[test]
    fn preflight_result_is_ok_with_only_warns() {
        let result = PreflightResult {
            checks: vec![
                PreflightCheck {
                    name: "a".into(),
                    status: PreflightStatus::Pass,
                },
                PreflightCheck {
                    name: "b".into(),
                    status: PreflightStatus::Warn("warning".into()),
                },
            ],
        };
        assert!(result.is_ok());
    }

    #[test]
    fn preflight_empty_checks_gets_default_pass() {
        // A workflow with no steps → no checks except the validate Pass
        let def = WorkflowDef {
            name: "empty".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![],
            output: None,
            prerequisites: None,
        };
        let result = preflight(&def, &[], &[]);
        // Either has "validate" pass or a generic "preflight" pass
        assert!(result.is_ok());
        assert!(!result.checks.is_empty());
    }

    // ── check_prerequisites ───────────────────────────────────────────────

    #[test]
    fn check_prerequisites_no_prerequisites_passes() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![],
            output: None,
            prerequisites: None,
        };
        assert!(check_prerequisites(&def).is_ok());
    }

    #[test]
    fn check_prerequisites_missing_required_env_fails() {
        use crate::types::{PrereqEntry, PrerequisitesDef};
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![],
            output: None,
            prerequisites: Some(PrerequisitesDef {
                commands: vec![],
                env: vec![PrereqEntry::Required(
                    "__CLIDO_DEFINITELY_NOT_SET_XYZ__".into(),
                )],
            }),
        };
        let err = check_prerequisites(&def).unwrap_err();
        assert!(err
            .to_string()
            .contains("Missing required environment variable"));
    }

    #[test]
    fn check_prerequisites_optional_env_missing_passes() {
        use crate::types::{PrereqEntry, PrerequisitesDef};
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![],
            output: None,
            prerequisites: Some(PrerequisitesDef {
                commands: vec![],
                env: vec![PrereqEntry::Optional {
                    name: "__CLIDO_DEFINITELY_NOT_SET_XYZ__".into(),
                    optional: true,
                }],
            }),
        };
        assert!(check_prerequisites(&def).is_ok());
    }

    #[test]
    fn check_prerequisites_missing_required_command_fails() {
        use crate::types::{PrereqEntry, PrerequisitesDef};
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![],
            output: None,
            prerequisites: Some(PrerequisitesDef {
                commands: vec![PrereqEntry::Required(
                    "__clido_no_such_cmd_xyz_12345__".into(),
                )],
                env: vec![],
            }),
        };
        let err = check_prerequisites(&def).unwrap_err();
        assert!(err.to_string().contains("Required command not found"));
    }

    #[test]
    fn check_prerequisites_present_command_passes() {
        use crate::types::{PrereqEntry, PrerequisitesDef};
        // "sh" should exist everywhere
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![],
            output: None,
            prerequisites: Some(PrerequisitesDef {
                commands: vec![PrereqEntry::Required("sh".into())],
                env: vec![],
            }),
        };
        assert!(check_prerequisites(&def).is_ok());
    }
}
