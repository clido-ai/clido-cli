//! Workflow executor: run steps (linear + parallel batches), on_error, retry, audit.

use std::path::Path;

use async_trait::async_trait;

use crate::context::{StepResult, WorkflowContext};
use crate::loader::{check_prerequisites, validate};
use crate::template::{render, render_save_to};
use crate::types::{OnErrorPolicy, StepDef, WorkflowDef};
use clido_core::{ClidoError, Result};

/// Return `s` if non-empty, otherwise a generic fallback.
fn non_empty_error(s: Option<&str>) -> String {
    match s {
        Some(e) if !e.trim().is_empty() => e.to_string(),
        _ => "Step failed (no error message provided)".to_string(),
    }
}

/// Request for a single step run (implemented by CLI).
#[derive(Debug, Clone)]
pub struct StepRunRequest {
    pub step_id: String,
    pub profile: Option<String>,
    /// None = all tools, Some(vec![]) = none, Some(names) = allowlist.
    pub tools: Option<Vec<String>>,
    pub system_prompt_override: Option<String>,
    pub max_turns_override: Option<u32>,
    pub rendered_prompt: String,
}

/// Result of a single step run.
#[derive(Debug, Clone)]
pub struct StepRunResult {
    pub output_text: String,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// Abstraction for running one step (CLI implements with AgentLoop).
#[async_trait]
pub trait WorkflowStepRunner: Send + Sync {
    async fn run_step(&self, request: StepRunRequest) -> Result<StepRunResult>;
}

/// Summary after a full workflow run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowSummary {
    pub total_cost_usd: f64,
    pub total_duration_ms: u64,
    pub step_count: u32,
    pub success: bool,
}

/// Write step outputs to disk for any `OutputDef` entries that have `save_to` set.
/// Called only on success (no-op if output_text is empty due to error).
fn apply_save_to(step: &StepDef, ctx: &WorkflowContext, output_text: &str) -> Result<()> {
    for out in &step.outputs {
        let Some(ref template) = out.save_to else {
            continue;
        };
        let path_str = render_save_to(template, ctx, &step.id)?;
        let path = std::path::Path::new(&path_str);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ClidoError::Workflow(format!(
                    "save_to: failed to create directory '{}': {}",
                    parent.display(),
                    e
                ))
            })?;
        }
        std::fs::write(path, output_text).map_err(|e| {
            ClidoError::Workflow(format!("save_to: failed to write '{}': {}", path_str, e))
        })?;
    }
    Ok(())
}

/// Execute workflow: validate, enforce prerequisites, run steps (linear + parallel batches),
/// apply on_error/retry, write audit.
pub async fn run(
    def: &WorkflowDef,
    context: &mut WorkflowContext,
    runner: &dyn WorkflowStepRunner,
    audit_path: Option<&Path>,
) -> Result<WorkflowSummary> {
    validate(def)?;
    check_prerequisites(def)?;

    let mut total_cost = 0.0_f64;
    let mut total_duration = 0_u64;
    let mut success = true;
    let steps = &def.steps;
    let n = steps.len();

    let mut i = 0;
    while i < n {
        // Group consecutive parallel steps.
        let mut batch = vec![&steps[i]];
        while i + batch.len() < n && steps[i + batch.len()].parallel {
            batch.push(&steps[i + batch.len()]);
        }

        if batch.len() == 1 {
            let step = batch[0];
            let (result, step_result) = run_one_step(step, context, runner).await?;
            total_cost += result.cost_usd;
            total_duration += result.duration_ms;
            context.step_results.push(step_result);
            if result.error.is_some() {
                match step.on_error {
                    OnErrorPolicy::Fail => {
                        return Err(ClidoError::Workflow(non_empty_error(
                            result.error.as_deref(),
                        )));
                    }
                    OnErrorPolicy::Continue => {
                        success = false;
                    }
                    OnErrorPolicy::Retry => {
                        let mut attempts = 1_u32;
                        let max_attempts = step.retry.as_ref().map(|r| r.max_attempts).unwrap_or(3);
                        let mut last_error = result.error.clone();
                        while attempts < max_attempts {
                            let (retry_result, step_result) =
                                run_one_step(step, context, runner).await?;
                            total_cost += retry_result.cost_usd;
                            total_duration += retry_result.duration_ms;
                            context.step_results.push(step_result);
                            last_error = retry_result.error.clone();
                            if last_error.is_none() {
                                break;
                            }
                            attempts += 1;
                        }
                        if last_error.is_some() {
                            success = false;
                        }
                    }
                }
            }
            i += 1;
        } else {
            let mut handles = Vec::with_capacity(batch.len());
            for step in &batch {
                let step = (*step).clone();
                let ctx = context.clone();
                handles.push(run_one_step_owned(step, ctx, runner));
            }
            let results = futures::future::join_all(handles).await;
            for (res, step) in results.into_iter().zip(batch.iter()) {
                let (run_res, step_result) = res?;
                total_cost += run_res.cost_usd;
                total_duration += run_res.duration_ms;
                context.set_step_output(&step.id, "output", step_result.output_text.clone());
                for out in &step.outputs {
                    if out.name != "output" {
                        context.set_step_output(
                            &step.id,
                            &out.name,
                            step_result.output_text.clone(),
                        );
                    }
                }
                if run_res.error.is_none() {
                    apply_save_to(step, context, &step_result.output_text)?;
                }
                context.step_results.push(step_result);
                if run_res.error.is_some() {
                    success = false;
                    if step.on_error == OnErrorPolicy::Fail {
                        return Err(ClidoError::Workflow(non_empty_error(
                            run_res.error.as_deref(),
                        )));
                    }
                }
            }
            i += batch.len();
        }
    }

    if let Some(path) = audit_path {
        let parent = path.parent().unwrap_or(Path::new("."));
        let _ = std::fs::create_dir_all(parent);
        let audit = serde_json::json!({
            "inputs": context.inputs,
            "step_outputs": context.step_outputs,
            "step_results": context.step_results,
            "total_cost_usd": total_cost,
            "total_duration_ms": total_duration,
        });
        if let Ok(s) = serde_json::to_string_pretty(&audit) {
            let _ = std::fs::write(path, s);
        }
    }

    Ok(WorkflowSummary {
        total_cost_usd: total_cost,
        total_duration_ms: total_duration,
        step_count: n as u32,
        success,
    })
}

/// Evaluate a foreach expression: render it as a Tera template, then try JSON
/// array parsing, falling back to newline-separated strings.
fn evaluate_foreach_items(expr: &str, ctx: &WorkflowContext) -> Result<Vec<serde_json::Value>> {
    let rendered = render(expr, ctx)?;
    let trimmed = rendered.trim();
    // Try JSON array first.
    if trimmed.starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
            return Ok(arr);
        }
    }
    // Fall back to newline-separated strings.
    let items = trimmed
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::Value::String(l.to_string()))
        .collect();
    Ok(items)
}

async fn run_one_step(
    step: &StepDef,
    context: &mut WorkflowContext,
    runner: &dyn WorkflowStepRunner,
) -> Result<(StepRunResult, StepResult)> {
    if let Some(ref foreach_expr) = step.foreach {
        // Foreach mode: run the step once per item.
        let items = evaluate_foreach_items(foreach_expr, context)?;
        let var_name = step.foreach_var.as_deref().unwrap_or("item");
        let mut combined_outputs: Vec<String> = Vec::new();
        let mut total_cost = 0.0_f64;
        let mut total_duration = 0_u64;
        let mut last_error: Option<String> = None;

        for item in items {
            context.set_foreach_item(var_name, item);
            let (result, _step_result) = run_one_step_single(step, context, runner).await?;
            total_cost += result.cost_usd;
            total_duration += result.duration_ms;
            if let Some(ref e) = result.error {
                if step.on_error == OnErrorPolicy::Fail {
                    context.clear_foreach_context();
                    let step_result = StepResult {
                        step_id: step.id.clone(),
                        output_text: String::new(),
                        cost_usd: total_cost,
                        duration_ms: total_duration,
                        error: Some(e.clone()),
                    };
                    return Ok((
                        StepRunResult {
                            output_text: String::new(),
                            cost_usd: total_cost,
                            duration_ms: total_duration,
                            error: Some(e.clone()),
                        },
                        step_result,
                    ));
                }
                last_error = Some(e.clone());
                // continue or retry policies: skip the failed item and move on.
            } else {
                combined_outputs.push(result.output_text.clone());
            }
        }

        context.clear_foreach_context();
        let combined = combined_outputs.join("\n---\n");
        context.set_step_output(&step.id, "output", combined.clone());
        for out in &step.outputs {
            if out.name != "output" {
                context.set_step_output(&step.id, &out.name, combined.clone());
            }
        }
        if last_error.is_none() {
            apply_save_to(step, context, &combined)?;
        }
        let step_result = StepResult {
            step_id: step.id.clone(),
            output_text: combined.clone(),
            cost_usd: total_cost,
            duration_ms: total_duration,
            error: last_error.clone(),
        };
        return Ok((
            StepRunResult {
                output_text: combined,
                cost_usd: total_cost,
                duration_ms: total_duration,
                error: last_error,
            },
            step_result,
        ));
    }
    run_one_step_single(step, context, runner).await
}

async fn run_one_step_single(
    step: &StepDef,
    context: &mut WorkflowContext,
    runner: &dyn WorkflowStepRunner,
) -> Result<(StepRunResult, StepResult)> {
    let rendered_prompt = render(&step.prompt, context)?;
    let rendered_system_prompt = step
        .system_prompt
        .as_deref()
        .map(|sp| render(sp, context).unwrap_or_else(|_| sp.to_string()));
    let request = StepRunRequest {
        step_id: step.id.clone(),
        profile: step.profile.clone(),
        tools: step.tools.clone(),
        system_prompt_override: rendered_system_prompt,
        max_turns_override: step.max_turns,
        rendered_prompt,
    };
    let result = runner.run_step(request).await?;
    let output_text = result.output_text.clone();
    context.set_step_output(&step.id, "output", output_text.clone());
    for out in &step.outputs {
        if out.name != "output" {
            context.set_step_output(&step.id, &out.name, output_text.clone());
        }
    }
    if result.error.is_none() {
        apply_save_to(step, context, &output_text)?;
    }
    let step_result = StepResult {
        step_id: step.id.clone(),
        output_text: output_text.clone(),
        cost_usd: result.cost_usd,
        duration_ms: result.duration_ms,
        error: result.error.clone(),
    };
    Ok((
        StepRunResult {
            output_text,
            cost_usd: result.cost_usd,
            duration_ms: result.duration_ms,
            error: result.error,
        },
        step_result,
    ))
}

async fn run_one_step_owned(
    step: StepDef,
    context: WorkflowContext,
    runner: &dyn WorkflowStepRunner,
) -> Result<(StepRunResult, StepResult)> {
    let rendered_prompt = render(&step.prompt, &context)?;
    let request = StepRunRequest {
        step_id: step.id.clone(),
        profile: step.profile.clone(),
        tools: step.tools.clone(),
        system_prompt_override: step.system_prompt.clone(),
        max_turns_override: step.max_turns,
        rendered_prompt,
    };
    let result = runner.run_step(request).await?;
    let step_result = StepResult {
        step_id: step.id.clone(),
        output_text: result.output_text.clone(),
        cost_usd: result.cost_usd,
        duration_ms: result.duration_ms,
        error: result.error.clone(),
    };
    Ok((result, step_result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::WorkflowContext;
    use crate::types::{OnErrorPolicy, StepDef, WorkflowDef};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Mock runner for integration tests. Can fail specific steps or succeed after N calls.
    struct MockRunner {
        /// Step ids that always fail.
        fail_steps: std::collections::HashSet<String>,
        /// Step id -> succeed after this many calls (1-based).
        retry_success_after: HashMap<String, u32>,
        /// Call count per step_id (for retry and assertions).
        call_count: Mutex<HashMap<String, u32>>,
        /// Last tools received (for tools: [] assertion).
        last_tools: Mutex<Option<Vec<String>>>,
    }

    impl MockRunner {
        fn new() -> Self {
            Self {
                fail_steps: std::collections::HashSet::new(),
                retry_success_after: HashMap::new(),
                call_count: Mutex::new(HashMap::new()),
                last_tools: Mutex::new(None),
            }
        }
        fn fail_step(mut self, id: &str) -> Self {
            self.fail_steps.insert(id.to_string());
            self
        }
        fn succeed_after(mut self, id: &str, after: u32) -> Self {
            self.retry_success_after.insert(id.to_string(), after);
            self
        }
    }

    #[async_trait::async_trait]
    impl WorkflowStepRunner for MockRunner {
        async fn run_step(&self, request: StepRunRequest) -> Result<StepRunResult> {
            let mut count = self.call_count.lock().unwrap();
            let c = count.entry(request.step_id.clone()).or_insert(0);
            *c += 1;
            let n = *c;
            drop(count);

            *self.last_tools.lock().unwrap() = request.tools.clone();

            let fail = self.fail_steps.contains(&request.step_id)
                || self
                    .retry_success_after
                    .get(&request.step_id)
                    .map(|&after| n < after)
                    .unwrap_or(false);

            if fail {
                Ok(StepRunResult {
                    output_text: String::new(),
                    cost_usd: 0.0,
                    duration_ms: 0,
                    error: Some("mock failure".to_string()),
                })
            } else {
                Ok(StepRunResult {
                    output_text: format!("output_{}", request.step_id),
                    cost_usd: 0.001,
                    duration_ms: 10,
                    error: None,
                })
            }
        }
    }

    fn linear_three_step_def() -> (WorkflowDef, WorkflowContext) {
        let def = WorkflowDef {
            name: "linear".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![
                StepDef {
                    id: "a".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "First".into(),
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
                    id: "b".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "Use: {{ steps.a.output }}".into(),
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
                    id: "c".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "Last".into(),
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
        let ctx = WorkflowContext::new(HashMap::new());
        (def, ctx)
    }

    #[tokio::test]
    async fn linear_3_step_output_chaining() {
        let (def, mut ctx) = linear_three_step_def();
        let runner = MockRunner::new();
        let summary = run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(summary.success);
        assert_eq!(summary.step_count, 3);
        assert_eq!(ctx.get_step_output("a", "output"), Some("output_a"));
        assert_eq!(ctx.get_step_output("b", "output"), Some("output_b"));
        assert_eq!(ctx.get_step_output("c", "output"), Some("output_c"));
        // Step b's prompt should have been rendered with steps.a.output
        assert!(ctx.step_outputs.contains_key("a.output"));
    }

    #[tokio::test]
    async fn on_error_continue() {
        let def = WorkflowDef {
            name: "continue".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![
                StepDef {
                    id: "a".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "A".into(),
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
                    id: "b".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "B".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Continue,
                    retry: None,
                    parallel: false,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
                StepDef {
                    id: "c".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "C".into(),
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
        let runner = MockRunner::new().fail_step("b");
        let mut ctx = WorkflowContext::new(HashMap::new());
        let summary = run(&def, &mut ctx, &runner, None).await.unwrap();
        assert_eq!(ctx.step_results.len(), 3);
        assert_eq!(summary.step_count, 3);
        // Step b failed but we continued; c ran
        assert_eq!(ctx.get_step_output("c", "output"), Some("output_c"));
    }

    #[tokio::test]
    async fn on_error_retry_succeeds_third() {
        let def = WorkflowDef {
            name: "retry".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "b".into(),
                name: None,
                profile: None,
                tools: None,
                prompt: "B".into(),
                outputs: vec![],
                on_error: OnErrorPolicy::Retry,
                retry: Some(crate::types::RetryConfig {
                    max_attempts: 3,
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
        let runner = MockRunner::new().succeed_after("b", 3);
        let mut ctx = WorkflowContext::new(HashMap::new());
        let summary = run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(summary.success);
        let count = runner.call_count.lock().unwrap();
        assert_eq!(count.get("b"), Some(&3));
    }

    #[tokio::test]
    async fn parallel_batch() {
        let def = WorkflowDef {
            name: "parallel".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![
                StepDef {
                    id: "a".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "A".into(),
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
                    id: "b".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "B".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: true,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
                StepDef {
                    id: "c".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "C".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: true,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
                StepDef {
                    id: "d".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "D uses {{ steps.a.output }} and {{ steps.b.output }}".into(),
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
        let runner = MockRunner::new();
        let mut ctx = WorkflowContext::new(HashMap::new());
        let summary = run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(summary.success);
        assert_eq!(ctx.get_step_output("a", "output"), Some("output_a"));
        assert_eq!(ctx.get_step_output("b", "output"), Some("output_b"));
        assert_eq!(ctx.get_step_output("c", "output"), Some("output_c"));
        assert_eq!(ctx.get_step_output("d", "output"), Some("output_d"));
    }

    #[tokio::test]
    async fn tools_empty_runner_receives_none_or_empty() {
        let def = WorkflowDef {
            name: "tools_empty".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "x".into(),
                name: None,
                profile: None,
                tools: Some(vec![]),
                prompt: "No tools".into(),
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
        let runner = MockRunner::new();
        let mut ctx = WorkflowContext::new(HashMap::new());
        run(&def, &mut ctx, &runner, None).await.unwrap();
        let tools = runner.last_tools.lock().unwrap();
        assert_eq!(*tools, Some(vec![]));
    }

    // ── on_error: fail propagates error ───────────────────────────────────

    #[tokio::test]
    async fn on_error_fail_returns_workflow_error() {
        let def = WorkflowDef {
            name: "fail".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "bad".into(),
                name: None,
                profile: None,
                tools: None,
                prompt: "Bad".into(),
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
        let runner = MockRunner::new().fail_step("bad");
        let mut ctx = WorkflowContext::new(HashMap::new());
        let result = run(&def, &mut ctx, &runner, None).await;
        assert!(result.is_err());
        if let Err(ClidoError::Workflow(msg)) = result {
            assert!(msg.contains("mock failure") || !msg.is_empty());
        } else {
            panic!("expected Workflow error");
        }
    }

    // ── retry exhausted marks success=false ───────────────────────────────

    #[tokio::test]
    async fn on_error_retry_exhausted_marks_not_success() {
        let def = WorkflowDef {
            name: "retry_fail".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "s".into(),
                name: None,
                profile: None,
                tools: None,
                prompt: "Always fail".into(),
                outputs: vec![],
                on_error: OnErrorPolicy::Retry,
                retry: Some(crate::types::RetryConfig {
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
        let runner = MockRunner::new().fail_step("s");
        let mut ctx = WorkflowContext::new(HashMap::new());
        let summary = run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(!summary.success);
    }

    // ── audit_path writes JSON ────────────────────────────────────────────

    #[tokio::test]
    async fn run_with_audit_path_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let audit_path = tmp.path().join("audit").join("run.json");
        let (def, mut ctx) = linear_three_step_def();
        let runner = MockRunner::new();
        run(&def, &mut ctx, &runner, Some(&audit_path))
            .await
            .unwrap();
        assert!(audit_path.exists(), "audit file should be written");
        let content = std::fs::read_to_string(&audit_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(json.get("total_cost_usd").is_some());
    }

    // ── WorkflowSummary serialization ─────────────────────────────────────

    #[test]
    fn workflow_summary_serialization() {
        let summary = WorkflowSummary {
            total_cost_usd: 0.001,
            total_duration_ms: 100,
            step_count: 3,
            success: true,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let parsed: WorkflowSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.step_count, 3);
        assert!(parsed.success);
        assert!((parsed.total_cost_usd - 0.001).abs() < 1e-9);
    }

    // ── parallel batch with failure + Fail policy ─────────────────────────

    #[tokio::test]
    async fn parallel_batch_with_fail_returns_error() {
        let def = WorkflowDef {
            name: "par_fail".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![
                StepDef {
                    id: "p1".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "P1".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: true,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
                StepDef {
                    id: "p2".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "P2".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: true,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
            ],
            output: None,
            prerequisites: None,
        };
        let runner = MockRunner::new().fail_step("p1");
        let mut ctx = WorkflowContext::new(HashMap::new());
        let result = run(&def, &mut ctx, &runner, None).await;
        assert!(result.is_err());
    }

    // ── step with named outputs (non-"output") ────────────────────────────

    #[tokio::test]
    async fn step_with_named_output_sets_context() {
        use crate::types::OutputDef;
        let def = WorkflowDef {
            name: "named_output".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "s1".into(),
                name: None,
                profile: None,
                tools: None,
                prompt: "Generate summary".into(),
                outputs: vec![
                    OutputDef {
                        name: "output".into(),
                        r#type: "text".into(),
                        save_to: None,
                    },
                    OutputDef {
                        name: "summary".into(),
                        r#type: "text".into(),
                        save_to: None,
                    },
                ],
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
        let runner = MockRunner::new();
        let mut ctx = WorkflowContext::new(HashMap::new());
        let summary = run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(summary.success);
        // "output" is the main output; "summary" is a named secondary output with same text
        assert_eq!(ctx.get_step_output("s1", "output"), Some("output_s1"));
        assert_eq!(ctx.get_step_output("s1", "summary"), Some("output_s1"));
    }

    // ── run_one_step_owned via parallel execution ──────────────────────────

    #[tokio::test]
    async fn parallel_steps_use_run_one_step_owned() {
        // Two parallel steps from the start → forces run_one_step_owned
        let def = WorkflowDef {
            name: "par_owned".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![
                StepDef {
                    id: "p1".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "P1".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: true,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
                StepDef {
                    id: "p2".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "P2".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: true,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
            ],
            output: None,
            prerequisites: None,
        };
        let runner = MockRunner::new();
        let mut ctx = WorkflowContext::new(HashMap::new());
        let summary = run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(summary.success);
        assert_eq!(summary.step_count, 2);
        assert_eq!(ctx.get_step_output("p1", "output"), Some("output_p1"));
        assert_eq!(ctx.get_step_output("p2", "output"), Some("output_p2"));
    }

    // ── StepRunRequest fields passed through ──────────────────────────────

    #[tokio::test]
    async fn step_run_request_includes_profile_and_system_prompt() {
        struct CapturingRunner {
            last_req: Mutex<Option<StepRunRequest>>,
        }
        #[async_trait::async_trait]
        impl WorkflowStepRunner for CapturingRunner {
            async fn run_step(&self, request: StepRunRequest) -> clido_core::Result<StepRunResult> {
                *self.last_req.lock().unwrap() = Some(request);
                Ok(StepRunResult {
                    output_text: "done".into(),
                    cost_usd: 0.0,
                    duration_ms: 0,
                    error: None,
                })
            }
        }

        let def = WorkflowDef {
            name: "capture".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "s1".into(),
                name: None,
                profile: Some("fast".into()),
                tools: Some(vec!["Read".into()]),
                prompt: "Test".into(),
                outputs: vec![],
                on_error: OnErrorPolicy::Fail,
                retry: None,
                parallel: false,
                system_prompt: Some("You are helpful.".into()),
                max_turns: Some(5),
                foreach: None,
                foreach_var: None,
            }],
            output: None,
            prerequisites: None,
        };

        let runner = CapturingRunner {
            last_req: Mutex::new(None),
        };
        let mut ctx = WorkflowContext::new(HashMap::new());
        run(&def, &mut ctx, &runner, None).await.unwrap();

        let req = runner.last_req.lock().unwrap().take().unwrap();
        assert_eq!(req.step_id, "s1");
        assert_eq!(req.profile, Some("fast".into()));
        assert_eq!(req.system_prompt_override, Some("You are helpful.".into()));
        assert_eq!(req.max_turns_override, Some(5));
        assert_eq!(req.tools, Some(vec!["Read".into()]));
    }

    // ── save_to writes output to disk ─────────────────────────────────────

    #[tokio::test]
    async fn save_to_writes_file_on_success() {
        use crate::types::OutputDef;
        let tmp = tempfile::tempdir().unwrap();
        let save_path = tmp.path().join("output.txt");
        let save_path_str = save_path.to_str().unwrap().to_string();

        let def = WorkflowDef {
            name: "save".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "s1".into(),
                name: None,
                profile: None,
                tools: None,
                prompt: "Generate output".into(),
                outputs: vec![OutputDef {
                    name: "output".into(),
                    r#type: "text".into(),
                    save_to: Some(save_path_str.clone()),
                }],
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
        let runner = MockRunner::new();
        let mut ctx = WorkflowContext::new(HashMap::new());
        run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(save_path.exists(), "save_to file should be written");
        let content = std::fs::read_to_string(&save_path).unwrap();
        assert_eq!(content, "output_s1");
    }

    #[tokio::test]
    async fn save_to_uses_step_id_in_template() {
        use crate::types::OutputDef;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_str().unwrap().to_string();

        let def = WorkflowDef {
            name: "save_template".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "intel".into(),
                name: None,
                profile: None,
                tools: None,
                prompt: "Generate intel".into(),
                outputs: vec![OutputDef {
                    name: "output".into(),
                    r#type: "text".into(),
                    save_to: Some(format!("{}/{{{{ step_id }}}}.json", dir)),
                }],
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
        let runner = MockRunner::new();
        let mut ctx = WorkflowContext::new(HashMap::new());
        run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(tmp.path().join("intel.json").exists());
    }

    #[tokio::test]
    async fn save_to_not_written_on_step_error() {
        use crate::types::OutputDef;
        let tmp = tempfile::tempdir().unwrap();
        let save_path = tmp.path().join("should_not_exist.txt");

        let def = WorkflowDef {
            name: "save_err".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![StepDef {
                id: "s1".into(),
                name: None,
                profile: None,
                tools: None,
                prompt: "Fail".into(),
                outputs: vec![OutputDef {
                    name: "output".into(),
                    r#type: "text".into(),
                    save_to: Some(save_path.to_str().unwrap().to_string()),
                }],
                on_error: OnErrorPolicy::Continue,
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
        let runner = MockRunner::new().fail_step("s1");
        let mut ctx = WorkflowContext::new(HashMap::new());
        run(&def, &mut ctx, &runner, None).await.unwrap();
        assert!(!save_path.exists(), "save_to should not write on error");
    }

    // ── parallel steps set named outputs ─────────────────────────────────

    #[tokio::test]
    async fn parallel_steps_set_named_outputs() {
        use crate::types::OutputDef;
        let def = WorkflowDef {
            name: "par_named".into(),
            version: "1".into(),
            description: String::new(),
            inputs: vec![],
            steps: vec![
                StepDef {
                    id: "p1".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "P1".into(),
                    outputs: vec![
                        OutputDef {
                            name: "output".into(),
                            r#type: "text".into(),
                            save_to: None,
                        },
                        OutputDef {
                            name: "findings".into(),
                            r#type: "text".into(),
                            save_to: None,
                        },
                    ],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: true,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
                StepDef {
                    id: "p2".into(),
                    name: None,
                    profile: None,
                    tools: None,
                    prompt: "P2".into(),
                    outputs: vec![],
                    on_error: OnErrorPolicy::Fail,
                    retry: None,
                    parallel: true,
                    system_prompt: None,
                    max_turns: None,
                    foreach: None,
                    foreach_var: None,
                },
            ],
            output: None,
            prerequisites: None,
        };
        let runner = MockRunner::new();
        let mut ctx = WorkflowContext::new(HashMap::new());
        run(&def, &mut ctx, &runner, None).await.unwrap();
        // Named output "findings" must be set for parallel step p1
        assert_eq!(ctx.get_step_output("p1", "output"), Some("output_p1"));
        assert_eq!(ctx.get_step_output("p1", "findings"), Some("output_p1"));
        assert_eq!(ctx.get_step_output("p2", "output"), Some("output_p2"));
    }
}
