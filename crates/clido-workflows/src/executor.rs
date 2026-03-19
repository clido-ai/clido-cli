//! Workflow executor: run steps (linear + parallel batches), on_error, retry, audit.

use std::path::Path;

use async_trait::async_trait;

use crate::context::{StepResult, WorkflowContext};
use crate::loader::validate;
use crate::template::render;
use crate::types::{OnErrorPolicy, StepDef, WorkflowDef};
use clido_core::{ClidoError, Result};

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

/// Execute workflow: validate, run steps (linear + parallel batches), apply on_error/retry, write audit.
pub async fn run(
    def: &WorkflowDef,
    context: &mut WorkflowContext,
    runner: &dyn WorkflowStepRunner,
    audit_path: Option<&Path>,
) -> Result<WorkflowSummary> {
    validate(def)?;

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
                        return Err(ClidoError::Workflow(
                            result.error.unwrap_or_else(|| "Step failed".to_string()),
                        ));
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
                context.step_results.push(step_result);
                if run_res.error.is_some() {
                    success = false;
                    if step.on_error == OnErrorPolicy::Fail {
                        return Err(ClidoError::Workflow(
                            run_res.error.unwrap_or_else(|| "Step failed".to_string()),
                        ));
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

async fn run_one_step(
    step: &StepDef,
    context: &mut WorkflowContext,
    runner: &dyn WorkflowStepRunner,
) -> Result<(StepRunResult, StepResult)> {
    let rendered_prompt = render(&step.prompt, context)?;
    let request = StepRunRequest {
        step_id: step.id.clone(),
        profile: step.profile.clone(),
        tools: step.tools.clone(),
        system_prompt_override: step.system_prompt.clone(),
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
}
