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
