//! Tera template rendering for prompts and save_to paths.

use std::collections::HashMap;

use chrono::Utc;
use tera::{Context, Tera};

use crate::context::WorkflowContext;
use clido_core::{ClidoError, Result};

/// Build Tera context from WorkflowContext (inputs, step_outputs, date, datetime).
pub fn build_tera_context(ctx: &WorkflowContext) -> Result<Context> {
    let mut tera_ctx = Context::new();
    tera_ctx.insert("inputs", &ctx.inputs);
    for (k, v) in &ctx.inputs {
        tera_ctx.insert(k, v);
    }
    let step_map: HashMap<String, HashMap<String, String>> =
        ctx.step_outputs
            .iter()
            .fold(HashMap::new(), |mut acc, (key, value)| {
                if let Some((step_id, output_name)) = key.split_once('.') {
                    acc.entry(step_id.to_string())
                        .or_default()
                        .insert(output_name.to_string(), value.clone());
                }
                acc
            });
    tera_ctx.insert("steps", &step_map);
    tera_ctx.insert("date", &Utc::now().format("%Y-%m-%d").to_string());
    tera_ctx.insert("datetime", &Utc::now().to_rfc3339());
    Ok(tera_ctx)
}

/// Normalize `${{ ... }}` (GitHub Actions style) to `{{ ... }}` (Tera style).
/// This allows workflow YAML to use either notation interchangeably.
pub fn normalize_template(template_str: &str) -> String {
    template_str.replace("${{", "{{")
}

/// Render a template string with the given context. Missing variable → Workflow error.
/// Supports both `{{ inputs.name }}` (Tera) and `${{ inputs.name }}` (GitHub Actions) syntax.
pub fn render(template_str: &str, ctx: &WorkflowContext) -> Result<String> {
    let normalized = normalize_template(template_str);
    let tera_ctx = build_tera_context(ctx)?;
    let mut tera = Tera::default();
    tera.add_raw_template("inline", &normalized)
        .map_err(|e| ClidoError::Workflow(format!("Template parse error: {}", e)))?;
    tera.render("inline", &tera_ctx)
        .map_err(|e| ClidoError::Workflow(format!("Template render error: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::WorkflowContext;
    use std::collections::HashMap;

    #[test]
    fn render_inputs() {
        let mut inputs = HashMap::new();
        inputs.insert(
            "name".to_string(),
            serde_json::Value::String("Alice".to_string()),
        );
        let ctx = WorkflowContext::new(inputs);
        let out = render("Hello {{ inputs.name }}", &ctx).unwrap();
        assert_eq!(out, "Hello Alice");
    }

    #[test]
    fn render_steps() {
        let mut ctx = WorkflowContext::new(HashMap::new());
        ctx.set_step_output("a", "output", "step a result".to_string());
        let out = render("Previous: {{ steps.a.output }}", &ctx).unwrap();
        assert_eq!(out, "Previous: step a result");
    }

    #[test]
    fn render_github_actions_style_inputs() {
        let mut inputs = HashMap::new();
        inputs.insert(
            "repo".to_string(),
            serde_json::Value::String("my-repo".to_string()),
        );
        let ctx = WorkflowContext::new(inputs);
        // ${{ inputs.repo }} should work the same as {{ inputs.repo }}
        let out = render("Review ${{ inputs.repo }}", &ctx).unwrap();
        assert_eq!(out, "Review my-repo");
    }

    #[test]
    fn render_missing_var() {
        let ctx = WorkflowContext::new(HashMap::new());
        let err = render("Hello {{ inputs.missing }}", &ctx).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("render")
                || err.to_string().contains("missing")
        );
    }
}
