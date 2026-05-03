//! Workflow execution context: inputs, step outputs, results.

use std::collections::HashMap;

use crate::template::render_default;
use crate::types::WorkflowDef;
use clido_core::{ClidoError, Result};

/// Runtime context for a workflow run: resolved inputs and step outputs.
#[derive(Debug, Clone, Default)]
pub struct WorkflowContext {
    /// Resolved input values (key = input name).
    pub inputs: HashMap<String, serde_json::Value>,
    /// Step output text by "step_id.output_name".
    pub step_outputs: HashMap<String, String>,
    /// Per-step result (output_text, cost_usd, duration_ms, error).
    pub step_results: Vec<StepResult>,
    /// Per-iteration foreach variable bindings (variable_name → current value).
    pub foreach_context: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub output_text: String,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub error: Option<String>,
}

impl WorkflowContext {
    /// Resolve inputs from def and overrides. Missing required → Error.
    pub fn resolve_inputs(
        def: &WorkflowDef,
        overrides: &[(String, serde_json::Value)],
    ) -> Result<HashMap<String, serde_json::Value>> {
        let mut map: HashMap<String, serde_json::Value> = HashMap::new();
        for (k, v) in overrides {
            map.insert(k.clone(), v.clone());
        }
        for input in &def.inputs {
            if map.contains_key(&input.name) {
                continue;
            }
            if let Some(ref default) = input.default {
                // Render string defaults as templates so `{{ cwd }}`, `{{ date }}`,
                // `{{ datetime }}` work in default values.
                let resolved = match default {
                    serde_json::Value::String(s) if s.contains("{{") || s.contains("${{") => {
                        serde_json::Value::String(render_default(s))
                    }
                    other => other.clone(),
                };
                map.insert(input.name.clone(), resolved);
            } else if input.required {
                return Err(ClidoError::Workflow(format!(
                    "Missing required input: {}",
                    input.name
                )));
            }
        }
        Ok(map)
    }

    /// Create context with given inputs and empty step_outputs/step_results.
    pub fn new(inputs: HashMap<String, serde_json::Value>) -> Self {
        Self {
            inputs,
            step_outputs: HashMap::new(),
            step_results: Vec::new(),
        }
    }

    /// Set output for a step (e.g. "explore.output" -> "...").
    pub fn set_step_output(&mut self, step_id: &str, output_name: &str, value: impl Into<String>) {
        self.step_outputs
            .insert(format!("{}.{}", step_id, output_name), value.into());
    }

    /// Get step output for template (steps.step_id.output_name).
    pub fn get_step_output(&self, step_id: &str, output_name: &str) -> Option<&str> {
        self.step_outputs
            .get(&format!("{}.{}", step_id, output_name))
            .map(String::as_str)
    }

    /// Set a foreach iteration variable binding.
    pub fn set_foreach_item(&mut self, var_name: &str, value: serde_json::Value) {
        self.foreach_context.insert(var_name.to_string(), value);
    }

    /// Clear all foreach iteration variable bindings.
    pub fn clear_foreach_context(&mut self) {
        self.foreach_context.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{InputDef, WorkflowDef};

    #[test]
    fn resolve_inputs_required_missing() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![InputDef {
                name: "required_key".to_string(),
                description: String::new(),
                required: true,
                default: None,
            }],
            steps: vec![],
            output: None,
            prerequisites: None,
        };
        let overrides: Vec<(String, serde_json::Value)> = vec![];
        let err = WorkflowContext::resolve_inputs(&def, &overrides).unwrap_err();
        assert!(err.to_string().contains("Missing required input"));
        assert!(err.to_string().contains("required_key"));
    }

    /// Lines 36, 40: overrides are inserted into map, and inputs already in map are skipped.
    #[test]
    fn resolve_inputs_override_skips_default() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![InputDef {
                name: "key".to_string(),
                description: String::new(),
                required: false,
                default: Some(serde_json::Value::String("default_val".to_string())),
            }],
            steps: vec![],
            output: None,
            prerequisites: None,
        };
        // Override the key so the default should be skipped (line 40: continue)
        let overrides = vec![(
            "key".to_string(),
            serde_json::Value::String("override_val".to_string()),
        )];
        let map = WorkflowContext::resolve_inputs(&def, &overrides).unwrap();
        // Should use override value, not default
        assert_eq!(
            map.get("key").and_then(|v| v.as_str()),
            Some("override_val")
        );
    }

    #[test]
    fn resolve_inputs_default_applied() {
        let def = WorkflowDef {
            name: "x".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![InputDef {
                name: "opt".to_string(),
                description: String::new(),
                required: false,
                default: Some(serde_json::Value::String("default_val".to_string())),
            }],
            steps: vec![],
            output: None,
            prerequisites: None,
        };
        let overrides: Vec<(String, serde_json::Value)> = vec![];
        let map = WorkflowContext::resolve_inputs(&def, &overrides).unwrap();
        assert_eq!(map.get("opt").and_then(|v| v.as_str()), Some("default_val"));
    }
}
