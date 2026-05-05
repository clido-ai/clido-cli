//! Tera template rendering for prompts and save_to paths.

use std::collections::HashMap;

use chrono::Utc;
use tera::{Context, Tera};

use crate::context::WorkflowContext;
use clido_core::{ClidoError, Result};

/// Get the current working directory, preferring CLIDO_WORKDIR env var.
fn get_cwd() -> String {
    std::env::var("CLIDO_WORKDIR")
        .ok()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| ".".to_string())
}
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
    let cwd = get_cwd();
    tera_ctx.insert("cwd", &cwd);
    // Inject foreach iteration variables.
    for (k, v) in &ctx.foreach_context {
        tera_ctx.insert(k, v);
    }
    Ok(tera_ctx)
}
/// Render a string default value that may contain template expressions.
/// Only `{{ cwd }}`, `{{ date }}`, and `{{ datetime }}` are available at this stage
/// (no inputs or step outputs yet).
pub fn render_default(template_str: &str) -> String {
    render_default_with_inputs(template_str, &HashMap::new())
}

/// Render a string default value with already-resolved inputs available as `{{ inputs.name }}`.
/// This allows later input defaults to reference earlier ones (e.g. `{{ inputs.work_dir }}/repos`).
pub fn render_default_with_inputs(
    template_str: &str,
    resolved_inputs: &HashMap<String, serde_json::Value>,
) -> String {
    let normalized = normalize_template(template_str);
    let mut tera_ctx = Context::new();
    let cwd = get_cwd();
    tera_ctx.insert("cwd", &cwd);
    tera_ctx.insert("date", &Utc::now().format("%Y-%m-%d").to_string());
    tera_ctx.insert("datetime", &Utc::now().to_rfc3339());
    tera_ctx.insert("inputs", resolved_inputs);
    for (k, v) in resolved_inputs {
        tera_ctx.insert(k, v);
    }
    let mut tera = Tera::default();
    if tera.add_raw_template("default", &normalized).is_ok() {
        tera.render("default", &tera_ctx)
            .unwrap_or_else(|_| template_str.to_string())
    } else {
        template_str.to_string()
    }
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

/// Render a `save_to` path template. Same as `render` but also injects `{{ step_id }}`.
pub fn render_save_to(template_str: &str, ctx: &WorkflowContext, step_id: &str) -> Result<String> {
    let normalized = normalize_template(template_str);
    let mut tera_ctx = build_tera_context(ctx)?;
    tera_ctx.insert("step_id", step_id);
    let mut tera = Tera::default();
    tera.add_raw_template("save_to", &normalized)
        .map_err(|e| ClidoError::Workflow(format!("Template parse error: {}", e)))?;
    tera.render("save_to", &tera_ctx)
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

    #[test]
    fn render_save_to_injects_step_id() {
        let ctx = WorkflowContext::new(HashMap::new());
        let out = render_save_to("outputs/{{ step_id }}.txt", &ctx, "my_step").unwrap();
        assert_eq!(out, "outputs/my_step.txt");
    }

    #[test]
    fn render_save_to_with_inputs() {
        let mut inputs = HashMap::new();
        inputs.insert(
            "repo".to_string(),
            serde_json::Value::String("/tmp/myrepo".to_string()),
        );
        let ctx = WorkflowContext::new(inputs);
        let out =
            render_save_to("{{ inputs.repo }}/.audit/{{ step_id }}.json", &ctx, "intel").unwrap();
        assert_eq!(out, "/tmp/myrepo/.audit/intel.json");
    }
}
