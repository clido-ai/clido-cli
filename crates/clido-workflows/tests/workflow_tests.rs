//! Integration tests for the clido-workflows crate.
//!
//! These tests exercise the public API end-to-end across the
//! loader → context → template → executor pipeline.  They do NOT
//! invoke a real LLM: a lightweight mock runner is used instead.

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Mutex;

/// Serialize tests that mutate env vars so they don't race each other.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

use async_trait::async_trait;
use clido_workflows::executor::StepRunResult;
use clido_workflows::{
    check_prerequisites, load, preflight, render, render_default, render_save_to, validate,
    BackoffKind, InputDef, OnErrorPolicy, OutputDef, PreflightStatus, PrereqEntry,
    PrerequisitesDef, RetryConfig, StepDef, StepRunRequest, WorkflowContext, WorkflowDef,
    WorkflowStepRunner,
};

// ── helpers ──────────────────────────────────────────────────────────────────

/// A minimal valid YAML workflow string used by several tests.
fn minimal_yaml() -> &'static str {
    r#"
name: integration_test
version: "1"
steps:
  - id: step_a
    prompt: "Do the first thing"
"#
}

/// Build a simple two-step WorkflowDef without YAML parsing.
fn two_step_def() -> WorkflowDef {
    WorkflowDef {
        name: "two_step".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![
            make_step("s1", "First step", OnErrorPolicy::Fail, false),
            make_step(
                "s2",
                "Second step: {{ steps.s1.output }}",
                OnErrorPolicy::Fail,
                false,
            ),
        ],
        output: None,
        prerequisites: None,
    }
}

/// Construct a StepDef with sensible defaults for test use.
fn make_step(id: &str, prompt: &str, on_error: OnErrorPolicy, parallel: bool) -> StepDef {
    StepDef {
        id: id.into(),
        name: None,
        profile: None,
        tools: None,
        prompt: prompt.into(),
        outputs: vec![],
        on_error,
        retry: None,
        parallel,
        system_prompt: None,
        max_turns: None,
    }
}

// ── Mock runner ───────────────────────────────────────────────────────────────

/// A mock runner that always succeeds, returning `"out_<step_id>"` as output.
struct SuccessRunner;

#[async_trait]
impl WorkflowStepRunner for SuccessRunner {
    async fn run_step(&self, request: StepRunRequest) -> clido_core::Result<StepRunResult> {
        Ok(StepRunResult {
            output_text: format!("out_{}", request.step_id),
            cost_usd: 0.001,
            duration_ms: 5,
            error: None,
        })
    }
}

/// A mock runner that always reports an error.
struct FailRunner;

#[async_trait]
impl WorkflowStepRunner for FailRunner {
    async fn run_step(&self, _request: StepRunRequest) -> clido_core::Result<StepRunResult> {
        Ok(StepRunResult {
            output_text: String::new(),
            cost_usd: 0.0,
            duration_ms: 0,
            error: Some("intentional test failure".into()),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// YAML PARSING TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn yaml_minimal_parses_without_error() {
    let def: WorkflowDef = serde_yaml::from_str(minimal_yaml()).expect("minimal YAML should parse");
    assert_eq!(def.name, "integration_test");
    assert_eq!(def.steps.len(), 1);
    assert_eq!(def.steps[0].id, "step_a");
    assert_eq!(def.steps[0].prompt, "Do the first thing");
}

#[test]
fn yaml_multiple_steps_parse_correctly() {
    let yaml = r#"
name: multi_step_workflow
version: "1"
description: "Tests multiple steps"
steps:
  - id: gather
    prompt: "Gather information"
  - id: analyse
    prompt: "Analyse {{ steps.gather.output }}"
  - id: report
    prompt: "Write report based on {{ steps.analyse.output }}"
    on_error: continue
"#;
    let def: WorkflowDef = serde_yaml::from_str(yaml).expect("multi-step YAML should parse");
    assert_eq!(def.name, "multi_step_workflow");
    assert_eq!(def.steps.len(), 3);
    assert_eq!(def.steps[0].id, "gather");
    assert_eq!(def.steps[1].id, "analyse");
    assert_eq!(def.steps[2].id, "report");
    assert_eq!(def.steps[2].on_error, OnErrorPolicy::Continue);
}

#[test]
fn yaml_with_inputs_and_outputs_parses() {
    let yaml = r#"
name: full_workflow
version: "1"
inputs:
  - name: repo_path
    required: true
  - name: output_dir
    required: false
    default: "/tmp/out"
steps:
  - id: scan
    prompt: "Scan {{ inputs.repo_path }}"
    outputs:
      - name: output
        type: text
output:
  print_summary: true
"#;
    let def: WorkflowDef = serde_yaml::from_str(yaml).expect("full YAML should parse");
    assert_eq!(def.inputs.len(), 2);
    assert!(def.inputs[0].required);
    assert!(!def.inputs[1].required);
    assert!(def
        .output
        .as_ref()
        .map(|o| o.print_summary)
        .unwrap_or(false));
    assert_eq!(def.steps[0].outputs[0].name, "output");
}

#[test]
fn invalid_yaml_returns_error() {
    let bad_yaml = "name: test\nsteps: [unclosed bracket";
    let result: Result<WorkflowDef, _> = serde_yaml::from_str(bad_yaml);
    assert!(result.is_err(), "invalid YAML must return an error");
}

#[test]
fn yaml_missing_required_name_field_returns_error() {
    // `name` is required (no #[serde(default)]) — omitting it must fail.
    let yaml = r#"
steps:
  - id: s1
    prompt: "hello"
"#;
    let result: Result<WorkflowDef, _> = serde_yaml::from_str(yaml);
    assert!(
        result.is_err(),
        "missing `name` field must return a parse error"
    );
}

#[test]
fn yaml_missing_steps_field_returns_error() {
    // `steps` has no default either.
    let yaml = r#"name: no_steps"#;
    let result: Result<WorkflowDef, _> = serde_yaml::from_str(yaml);
    assert!(
        result.is_err(),
        "missing `steps` field must return a parse error"
    );
}

#[test]
fn yaml_step_missing_prompt_returns_error() {
    let yaml = r#"
name: bad_step
steps:
  - id: s1
"#;
    let result: Result<WorkflowDef, _> = serde_yaml::from_str(yaml);
    assert!(result.is_err(), "step without `prompt` must fail");
}

#[test]
fn yaml_step_missing_id_returns_error() {
    let yaml = r#"
name: no_id
steps:
  - prompt: "hello"
"#;
    let result: Result<WorkflowDef, _> = serde_yaml::from_str(yaml);
    assert!(result.is_err(), "step without `id` must fail");
}

// ═══════════════════════════════════════════════════════════════════════════
// FILE-BASED LOADING TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn load_valid_yaml_file() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(minimal_yaml().as_bytes()).unwrap();
    f.flush().unwrap();

    let def = load(f.path()).expect("load should succeed for valid file");
    assert_eq!(def.name, "integration_test");
}

#[test]
fn load_nonexistent_file_returns_workflow_error() {
    let err = load(std::path::Path::new("/tmp/__nonexistent_clido_test__.yaml")).unwrap_err();
    assert!(
        err.to_string().contains("Failed to read"),
        "error message should mention 'Failed to read'"
    );
}

#[test]
fn load_invalid_yaml_file_returns_workflow_error() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(b"not: valid: yaml: [").unwrap();
    f.flush().unwrap();

    let err = load(f.path()).unwrap_err();
    assert!(
        err.to_string().contains("Invalid workflow YAML"),
        "error message should mention 'Invalid workflow YAML'"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// VALIDATION TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn validate_valid_workflow_passes() {
    let def = two_step_def();
    validate(&def).expect("valid workflow should pass validation");
}

#[test]
fn validate_duplicate_step_ids_fails() {
    let def = WorkflowDef {
        name: "dup".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![
            make_step("same", "p1", OnErrorPolicy::Fail, false),
            make_step("same", "p2", OnErrorPolicy::Fail, false),
        ],
        output: None,
        prerequisites: None,
    };
    let err = validate(&def).unwrap_err();
    assert!(
        err.to_string().contains("Duplicate step id"),
        "duplicate step ids must produce a validation error"
    );
}

#[test]
fn validate_retry_without_on_error_retry_fails() {
    let mut step = make_step("s", "prompt", OnErrorPolicy::Fail, false);
    step.retry = Some(RetryConfig {
        max_attempts: 3,
        backoff: BackoffKind::None,
    });
    let def = WorkflowDef {
        name: "bad_retry".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![step],
        output: None,
        prerequisites: None,
    };
    let err = validate(&def).unwrap_err();
    assert!(err
        .to_string()
        .contains("retry config only allowed when on_error: retry"));
}

// ═══════════════════════════════════════════════════════════════════════════
// PREFLIGHT TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn preflight_passes_for_valid_workflow_no_profiles_no_tools() {
    let def = two_step_def();
    let result = preflight(&def, &[], &[]);
    assert!(
        result.is_ok(),
        "valid workflow with no profiles/tools should pass preflight"
    );
}

#[test]
fn preflight_fails_for_invalid_workflow() {
    // Duplicate step id → validation fails → preflight check is Fail.
    let def = WorkflowDef {
        name: "x".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![
            make_step("dup", "p1", OnErrorPolicy::Fail, false),
            make_step("dup", "p2", OnErrorPolicy::Fail, false),
        ],
        output: None,
        prerequisites: None,
    };
    let result = preflight(&def, &[], &[]);
    assert!(!result.is_ok());
    assert!(result
        .checks
        .iter()
        .any(|c| matches!(c.status, PreflightStatus::Fail(_))));
}

#[test]
fn preflight_warns_for_unknown_tool_but_still_passes() {
    let mut step = make_step("s", "prompt", OnErrorPolicy::Fail, false);
    step.tools = Some(vec!["SomeTool".into()]);
    let def = WorkflowDef {
        name: "warn_tool".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![step],
        output: None,
        prerequisites: None,
    };
    let result = preflight(&def, &[], &["KnownTool"]);
    assert!(
        result.is_ok(),
        "unknown tool produces a warning but not a failure"
    );
    assert!(result
        .checks
        .iter()
        .any(|c| matches!(c.status, PreflightStatus::Warn(_))));
}

// ═══════════════════════════════════════════════════════════════════════════
// WORKFLOW CONTEXT TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn context_new_starts_empty() {
    let ctx = WorkflowContext::new(HashMap::new());
    assert!(ctx.inputs.is_empty());
    assert!(ctx.step_outputs.is_empty());
    assert!(ctx.step_results.is_empty());
}

#[test]
fn context_set_and_get_step_output() {
    let mut ctx = WorkflowContext::new(HashMap::new());
    ctx.set_step_output("gather", "output", "the gathered data");
    assert_eq!(
        ctx.get_step_output("gather", "output"),
        Some("the gathered data")
    );
    assert_eq!(ctx.get_step_output("gather", "missing"), None);
}

#[test]
fn context_resolve_inputs_uses_override_over_default() {
    let def = WorkflowDef {
        name: "x".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![InputDef {
            name: "key".into(),
            description: String::new(),
            required: false,
            default: Some(serde_json::Value::String("default".into())),
        }],
        steps: vec![make_step("s", "p", OnErrorPolicy::Fail, false)],
        output: None,
        prerequisites: None,
    };
    let overrides = vec![("key".into(), serde_json::Value::String("override".into()))];
    let inputs = WorkflowContext::resolve_inputs(&def, &overrides).unwrap();
    assert_eq!(inputs["key"].as_str(), Some("override"));
}

#[test]
fn context_resolve_inputs_errors_on_missing_required() {
    let def = WorkflowDef {
        name: "x".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![InputDef {
            name: "must_provide".into(),
            description: String::new(),
            required: true,
            default: None,
        }],
        steps: vec![make_step("s", "p", OnErrorPolicy::Fail, false)],
        output: None,
        prerequisites: None,
    };
    let err = WorkflowContext::resolve_inputs(&def, &[]).unwrap_err();
    assert!(err.to_string().contains("Missing required input"));
    assert!(err.to_string().contains("must_provide"));
}

#[test]
fn context_cwd_is_set_from_current_dir() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let old = std::env::var("CLIDO_WORKDIR").ok();
    std::env::remove_var("CLIDO_WORKDIR");

    let ctx = WorkflowContext::new(HashMap::new());
    let rendered = render("{{ cwd }}", &ctx).unwrap();
    let expected = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(rendered, expected);

    if let Some(v) = old {
        std::env::set_var("CLIDO_WORKDIR", v);
    }
}

#[test]
#[serial_test::serial]
fn context_cwd_uses_clido_workdir_env_var() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let old = std::env::var("CLIDO_WORKDIR").ok();
    std::env::set_var("CLIDO_WORKDIR", "/tmp/fake_workdir");

    let ctx = WorkflowContext::new(HashMap::new());
    let rendered = render("{{ cwd }}", &ctx).unwrap();
    assert_eq!(rendered, "/tmp/fake_workdir");

    match old {
        Some(v) => std::env::set_var("CLIDO_WORKDIR", v),
        None => std::env::remove_var("CLIDO_WORKDIR"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// TEMPLATE RENDERING TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn render_cwd_variable() {
    let old = std::env::var("CLIDO_WORKDIR").ok();
    std::env::set_var("CLIDO_WORKDIR", "/repo");

    let ctx = WorkflowContext::new(HashMap::new());
    let out = render("Working in {{ cwd }}", &ctx).unwrap();
    assert_eq!(out, "Working in /repo");

    match old {
        Some(v) => std::env::set_var("CLIDO_WORKDIR", v),
        None => std::env::remove_var("CLIDO_WORKDIR"),
    }
}

#[test]
fn render_date_variable_is_non_empty() {
    let ctx = WorkflowContext::new(HashMap::new());
    let out = render("Date: {{ date }}", &ctx).unwrap();
    assert!(out.starts_with("Date: 20"), "date should look like a year");
}

#[test]
fn render_datetime_variable_is_non_empty() {
    let ctx = WorkflowContext::new(HashMap::new());
    let out = render("{{ datetime }}", &ctx).unwrap();
    // RFC 3339 format contains 'T'
    assert!(out.contains('T'), "datetime should be RFC 3339 format");
}

#[test]
fn render_inputs_variable() {
    let mut inputs = HashMap::new();
    inputs.insert("lang".into(), serde_json::Value::String("Rust".into()));
    let ctx = WorkflowContext::new(inputs);
    let out = render("Language: {{ inputs.lang }}", &ctx).unwrap();
    assert_eq!(out, "Language: Rust");
}

#[test]
fn render_direct_input_shorthand() {
    // Inputs are also inserted as top-level keys (not just under `inputs`).
    let mut inputs = HashMap::new();
    inputs.insert("city".into(), serde_json::Value::String("London".into()));
    let ctx = WorkflowContext::new(inputs);
    let out = render("City: {{ city }}", &ctx).unwrap();
    assert_eq!(out, "City: London");
}

#[test]
fn render_step_output_variable() {
    let mut ctx = WorkflowContext::new(HashMap::new());
    ctx.set_step_output("step_one", "output", "hello world");
    let out = render("Got: {{ steps.step_one.output }}", &ctx).unwrap();
    assert_eq!(out, "Got: hello world");
}

#[test]
fn render_github_actions_style_notation() {
    let mut inputs = HashMap::new();
    inputs.insert("repo".into(), serde_json::Value::String("myrepo".into()));
    let ctx = WorkflowContext::new(inputs);
    // ${{ inputs.repo }} must work identically to {{ inputs.repo }}
    let out = render("Repo: ${{ inputs.repo }}", &ctx).unwrap();
    assert_eq!(out, "Repo: myrepo");
}

#[test]
fn render_unknown_variable_returns_error() {
    let ctx = WorkflowContext::new(HashMap::new());
    let result = render("{{ inputs.no_such_var }}", &ctx);
    assert!(result.is_err(), "unknown variable should return an error");
    let msg = result.unwrap_err().to_string().to_lowercase();
    assert!(msg.contains("render") || msg.contains("error") || msg.contains("no_such_var"));
}

#[test]
fn render_for_loop_over_items() {
    // Tera supports for-loops; inject a JSON array as an input and iterate it.
    let mut inputs = HashMap::new();
    inputs.insert(
        "items".into(),
        serde_json::json!(["alpha", "beta", "gamma"]),
    );
    let ctx = WorkflowContext::new(inputs);
    let tpl = "{% for item in items %}{{ item }} {% endfor %}";
    let out = render(tpl, &ctx).unwrap();
    assert_eq!(out, "alpha beta gamma ");
}

#[test]
fn render_default_resolves_cwd_placeholder() {
    let old = std::env::var("CLIDO_WORKDIR").ok();
    std::env::set_var("CLIDO_WORKDIR", "/workspace");

    let out = render_default("{{ cwd }}/file.txt");
    assert_eq!(out, "/workspace/file.txt");

    match old {
        Some(v) => std::env::set_var("CLIDO_WORKDIR", v),
        None => std::env::remove_var("CLIDO_WORKDIR"),
    }
}

#[test]
fn render_default_resolves_date_placeholder() {
    let out = render_default("report-{{ date }}.txt");
    assert!(
        out.starts_with("report-20"),
        "date placeholder should expand to a year"
    );
    assert!(out.ends_with(".txt"));
}

#[test]
fn render_default_returns_input_unchanged_for_unknown_var() {
    // render_default falls back to the raw string on render failure.
    let input = "{{ unknown_var }}/path";
    let out = render_default(input);
    // The fallback returns the original string unchanged.
    assert_eq!(out, input);
}

#[test]
fn render_save_to_injects_step_id() {
    let ctx = WorkflowContext::new(HashMap::new());
    let out = render_save_to("reports/{{ step_id }}.json", &ctx, "audit").unwrap();
    assert_eq!(out, "reports/audit.json");
}

#[test]
fn render_save_to_combined_with_inputs() {
    let mut inputs = HashMap::new();
    inputs.insert("base".into(), serde_json::Value::String("/out".into()));
    let ctx = WorkflowContext::new(inputs);
    let out = render_save_to("{{ inputs.base }}/{{ step_id }}.txt", &ctx, "scan").unwrap();
    assert_eq!(out, "/out/scan.txt");
}

// ═══════════════════════════════════════════════════════════════════════════
// EXECUTOR INTEGRATION TESTS (mock runner, no LLM)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn run_linear_workflow_end_to_end() {
    let def = two_step_def();
    let mut ctx = WorkflowContext::new(HashMap::new());
    let summary = clido_workflows::run_workflow(&def, &mut ctx, &SuccessRunner, None)
        .await
        .expect("workflow should succeed");

    assert!(summary.success);
    assert_eq!(summary.step_count, 2);
    assert_eq!(ctx.get_step_output("s1", "output"), Some("out_s1"));
    assert_eq!(ctx.get_step_output("s2", "output"), Some("out_s2"));
    assert_eq!(ctx.step_results.len(), 2);
}

#[tokio::test]
async fn run_workflow_accumulates_cost_and_duration() {
    let def = two_step_def();
    let mut ctx = WorkflowContext::new(HashMap::new());
    let summary = clido_workflows::run_workflow(&def, &mut ctx, &SuccessRunner, None)
        .await
        .unwrap();

    // SuccessRunner returns 0.001 cost and 5 ms per step.
    assert!((summary.total_cost_usd - 0.002).abs() < 1e-9);
    assert_eq!(summary.total_duration_ms, 10);
}

#[tokio::test]
async fn run_workflow_on_error_fail_returns_error() {
    let def = WorkflowDef {
        name: "fail_test".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![make_step("bad", "fail me", OnErrorPolicy::Fail, false)],
        output: None,
        prerequisites: None,
    };
    let mut ctx = WorkflowContext::new(HashMap::new());
    let result = clido_workflows::run_workflow(&def, &mut ctx, &FailRunner, None).await;
    assert!(result.is_err(), "on_error: fail must propagate the error");
}

#[tokio::test]
async fn run_workflow_on_error_continue_runs_remaining_steps() {
    let def = WorkflowDef {
        name: "continue_test".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![
            make_step("ok_first", "First", OnErrorPolicy::Fail, false),
            make_step("bad_middle", "Middle", OnErrorPolicy::Continue, false),
            make_step("ok_last", "Last", OnErrorPolicy::Fail, false),
        ],
        output: None,
        prerequisites: None,
    };
    // Use a runner that fails only "bad_middle".
    struct SelectiveFail;
    #[async_trait]
    impl WorkflowStepRunner for SelectiveFail {
        async fn run_step(&self, req: StepRunRequest) -> clido_core::Result<StepRunResult> {
            if req.step_id == "bad_middle" {
                Ok(StepRunResult {
                    output_text: String::new(),
                    cost_usd: 0.0,
                    duration_ms: 0,
                    error: Some("oops".into()),
                })
            } else {
                Ok(StepRunResult {
                    output_text: format!("out_{}", req.step_id),
                    cost_usd: 0.0,
                    duration_ms: 0,
                    error: None,
                })
            }
        }
    }
    let mut ctx = WorkflowContext::new(HashMap::new());
    let summary = clido_workflows::run_workflow(&def, &mut ctx, &SelectiveFail, None)
        .await
        .expect("should not return Err when on_error: continue");

    // All three steps ran; ok_last produced output.
    assert_eq!(ctx.step_results.len(), 3);
    assert_eq!(
        ctx.get_step_output("ok_last", "output"),
        Some("out_ok_last")
    );
    // Workflow-level success=false because a step had an error.
    assert!(!summary.success);
}

#[tokio::test]
async fn run_workflow_output_chaining_across_steps() {
    // Step 2's prompt references step 1's output via template.
    let mut step2 = make_step(
        "step2",
        "Input was: {{ steps.step1.output }}",
        OnErrorPolicy::Fail,
        false,
    );
    // Add a named output so we can verify the rendered prompt was used.
    step2.outputs = vec![OutputDef {
        name: "output".into(),
        r#type: "text".into(),
        save_to: None,
    }];

    let def = WorkflowDef {
        name: "chain".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![
            make_step("step1", "Generate data", OnErrorPolicy::Fail, false),
            step2,
        ],
        output: None,
        prerequisites: None,
    };
    let mut ctx = WorkflowContext::new(HashMap::new());
    clido_workflows::run_workflow(&def, &mut ctx, &SuccessRunner, None)
        .await
        .unwrap();

    // step1 produced "out_step1"; step2 should have run with the chained template.
    assert_eq!(ctx.get_step_output("step1", "output"), Some("out_step1"));
    assert_eq!(ctx.get_step_output("step2", "output"), Some("out_step2"));
}

#[tokio::test]
async fn run_workflow_with_inputs_passed_to_template() {
    let def = WorkflowDef {
        name: "inputs_test".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![InputDef {
            name: "lang".into(),
            description: String::new(),
            required: true,
            default: None,
        }],
        steps: vec![make_step(
            "s1",
            "Review {{ inputs.lang }} code",
            OnErrorPolicy::Fail,
            false,
        )],
        output: None,
        prerequisites: None,
    };
    let mut inputs = HashMap::new();
    inputs.insert("lang".into(), serde_json::Value::String("Rust".into()));
    let mut ctx = WorkflowContext::new(inputs);
    let summary = clido_workflows::run_workflow(&def, &mut ctx, &SuccessRunner, None)
        .await
        .unwrap();
    assert!(summary.success);
}

#[tokio::test]
async fn run_workflow_with_audit_path_writes_json() {
    let tmp = tempfile::tempdir().unwrap();
    let audit = tmp.path().join("audit").join("result.json");

    let def = two_step_def();
    let mut ctx = WorkflowContext::new(HashMap::new());
    clido_workflows::run_workflow(&def, &mut ctx, &SuccessRunner, Some(&audit))
        .await
        .unwrap();

    assert!(audit.exists(), "audit file must be written");
    let raw = std::fs::read_to_string(&audit).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(json.get("total_cost_usd").is_some());
    assert!(json.get("step_results").is_some());
}

#[tokio::test]
async fn run_workflow_save_to_writes_step_output() {
    let tmp = tempfile::tempdir().unwrap();
    let out_file = tmp.path().join("result.txt");

    let mut step = make_step("writer", "Write something", OnErrorPolicy::Fail, false);
    step.outputs = vec![OutputDef {
        name: "output".into(),
        r#type: "text".into(),
        save_to: Some(out_file.to_str().unwrap().to_string()),
    }];

    let def = WorkflowDef {
        name: "save_to_test".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![step],
        output: None,
        prerequisites: None,
    };
    let mut ctx = WorkflowContext::new(HashMap::new());
    clido_workflows::run_workflow(&def, &mut ctx, &SuccessRunner, None)
        .await
        .unwrap();

    assert!(out_file.exists(), "save_to must write the output file");
    let content = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(content, "out_writer");
}

// ═══════════════════════════════════════════════════════════════════════════
// PREREQUISITES TESTS (cross-module via public API)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn check_prerequisites_missing_required_env_fails() {
    let def = WorkflowDef {
        name: "prereq_test".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![],
        output: None,
        prerequisites: Some(PrerequisitesDef {
            commands: vec![],
            env: vec![PrereqEntry::Required(
                "__CLIDO_INTEGRATION_TEST_NOT_SET__".into(),
            )],
        }),
    };
    let err = check_prerequisites(&def).unwrap_err();
    assert!(err
        .to_string()
        .contains("Missing required environment variable"));
}

#[test]
fn check_prerequisites_optional_missing_env_passes() {
    let def = WorkflowDef {
        name: "prereq_opt".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![],
        output: None,
        prerequisites: Some(PrerequisitesDef {
            commands: vec![],
            env: vec![PrereqEntry::Optional {
                name: "__CLIDO_INTEGRATION_TEST_NOT_SET__".into(),
                optional: true,
            }],
        }),
    };
    check_prerequisites(&def).expect("optional missing env should pass");
}

#[test]
fn check_prerequisites_missing_required_command_fails() {
    let def = WorkflowDef {
        name: "prereq_cmd".into(),
        version: "1".into(),
        description: String::new(),
        inputs: vec![],
        steps: vec![],
        output: None,
        prerequisites: Some(PrerequisitesDef {
            commands: vec![PrereqEntry::Required("__clido_no_such_cmd_xyz__".into())],
            env: vec![],
        }),
    };
    let err = check_prerequisites(&def).unwrap_err();
    assert!(err.to_string().contains("Required command not found"));
}

// ═══════════════════════════════════════════════════════════════════════════
// FULL ROUND-TRIP: YAML FILE → LOAD → VALIDATE → CONTEXT → RUN
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn full_pipeline_yaml_to_run() {
    let yaml = r#"
name: round_trip
version: "1"
inputs:
  - name: subject
    required: false
    default: "world"
steps:
  - id: greet
    prompt: "Say hello to {{ inputs.subject }}"
  - id: farewell
    prompt: "Say goodbye to {{ inputs.subject }} after: {{ steps.greet.output }}"
"#;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(yaml.as_bytes()).unwrap();
    f.flush().unwrap();

    // load + validate
    let def = load(f.path()).expect("load failed");
    validate(&def).expect("validate failed");

    // build context with no overrides (uses default)
    let inputs = WorkflowContext::resolve_inputs(&def, &[]).expect("resolve_inputs failed");
    assert_eq!(inputs["subject"].as_str(), Some("world"));

    let mut ctx = WorkflowContext::new(inputs);
    let summary = clido_workflows::run_workflow(&def, &mut ctx, &SuccessRunner, None)
        .await
        .expect("run failed");

    assert!(summary.success);
    assert_eq!(summary.step_count, 2);
    assert_eq!(ctx.get_step_output("greet", "output"), Some("out_greet"));
    assert_eq!(
        ctx.get_step_output("farewell", "output"),
        Some("out_farewell")
    );
}
