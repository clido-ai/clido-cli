//! Workflow commands: run, validate, inspect, list.

use async_trait::async_trait;
use clido_agent::AgentLoop;
use clido_core::{
    agent_config_from_loaded, default_workflows_directory, load_config, load_pricing, ClidoError,
    LoadedConfig, PermissionMode, Result as CoreResult,
};
use clido_storage::{workflow_run_path, SessionWriter};
use clido_tools::default_registry_with_options;
use clido_workflows::{
    load as load_workflow, preflight as preflight_workflow, render,
    run_workflow as run_workflow_exec, validate as validate_workflow, PreflightStatus,
    StepRunRequest, StepRunResult, WorkflowContext, WorkflowStepRunner,
};
use std::env;
use std::io::Write;
use std::path::Path;

use crate::agent_setup::with_optional_trace_metrics;
use crate::cli::{Cli, WorkflowCmd};
use crate::errors::CliError;
use crate::provider::make_provider;
use crate::ui::{ansi, cli_use_color};

pub async fn run_workflow(cli: &Cli, cmd: &WorkflowCmd) -> Result<(), anyhow::Error> {
    match cmd {
        WorkflowCmd::Run {
            workflow,
            input,
            profile,
            dry_run,
            yes: _,
        } => run_workflow_run(cli, workflow, input, profile.as_deref(), *dry_run).await,
        WorkflowCmd::Validate { path } => run_workflow_validate(path).await,
        WorkflowCmd::Inspect { path } => run_workflow_inspect(path).await,
        WorkflowCmd::List => run_workflow_list().await,
        WorkflowCmd::Check { path, json } => run_workflow_check(cli, path, *json).await,
    }
}

struct CliWorkflowRunner {
    workspace_root: std::path::PathBuf,
    run_id: String,
    sandbox: bool,
    mcp_config: Option<std::path::PathBuf>,
    quiet: bool,
    /// Profile override from `--profile` flag. Applied to steps with no explicit profile.
    profile_override: Option<String>,
}

#[async_trait]
impl WorkflowStepRunner for CliWorkflowRunner {
    async fn run_step(&self, request: StepRunRequest) -> CoreResult<StepRunResult> {
        let loaded =
            load_config(&self.workspace_root).map_err(|e| ClidoError::Workflow(e.to_string()))?;
        let (pricing_table, _) = load_pricing();
        let profile_name = request
            .profile
            .as_deref()
            .or(self.profile_override.as_deref())
            .unwrap_or(loaded.default_profile.as_str());
        let profile = loaded
            .get_profile(profile_name)
            .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        LoadedConfig::validate_provider(&profile.provider)
            .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        let provider =
            make_provider(profile_name, profile, None, None).map_err(ClidoError::Workflow)?;
        let model = profile.model.clone();
        let blocked = clido_core::global_config_path()
            .into_iter()
            .collect::<Vec<_>>();
        let mut registry =
            default_registry_with_options(self.workspace_root.clone(), blocked, self.sandbox);
        registry = crate::agent_setup::load_mcp_tools_from_path(
            self.mcp_config.as_deref(),
            self.quiet,
            registry,
        );
        let tools_explicitly_empty = request.tools.as_ref().is_some_and(|t| t.is_empty());
        registry = registry.with_filters(request.tools, None);
        // Only error if tools became empty unintentionally (not when tools: [] was set explicitly).
        if registry.schemas().is_empty() && !tools_explicitly_empty {
            return Err(ClidoError::Workflow(
                "No tools available for step".to_string(),
            ));
        }
        let system_prompt = request
            .system_prompt_override
            .unwrap_or_else(|| "You are a helpful coding assistant.".to_string());
        let mut config = agent_config_from_loaded(
            &loaded,
            profile_name,
            request.max_turns_override,
            None,
            Some(model),
            Some(system_prompt),
            Some(PermissionMode::Default),
            false,
            None,
        )
        .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        if config.max_context_tokens.is_none() {
            if let Some(entry) = pricing_table.models.get(&config.model) {
                if let Some(cw) = entry.context_window {
                    config.max_context_tokens = Some(cw);
                }
            }
        }
        let session_id = format!("{}_{}", self.run_id, request.step_id);
        let mut writer = SessionWriter::create(&self.workspace_root, &session_id)
            .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        let mut loop_ =
            with_optional_trace_metrics(AgentLoop::new(provider, registry, config, None));
        let start = std::time::Instant::now();
        let result = loop_
            .run(
                &request.rendered_prompt,
                Some(&mut writer),
                Some(&pricing_table),
                None,
            )
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;
        let _ = writer.flush();
        match result {
            Ok(text) => Ok(StepRunResult {
                output_text: text,
                cost_usd: loop_.cumulative_cost_usd,
                duration_ms,
                error: None,
            }),
            Err(e) => Ok(StepRunResult {
                output_text: String::new(),
                cost_usd: loop_.cumulative_cost_usd,
                duration_ms,
                error: Some(e.to_string()),
            }),
        }
    }
}

async fn run_workflow_run(
    cli: &Cli,
    workflow: &str,
    input: &[String],
    profile_override: Option<&str>,
    dry_run: bool,
) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let _loaded = load_config(&workspace_root).map_err(|e| CliError::Usage(e.to_string()))?;
    let path = resolve_workflow_path(workflow)?;
    let def = load_workflow(&path).map_err(|e| CliError::Usage(e.to_string()))?;
    validate_workflow(&def).map_err(|e| CliError::Usage(e.to_string()))?;
    let overrides: Vec<(String, serde_json::Value)> = input
        .iter()
        .filter_map(|s| {
            let (k, v) = s.split_once('=')?;
            Some((
                k.trim().to_string(),
                serde_json::Value::String(v.trim().to_string()),
            ))
        })
        .collect();
    let inputs = WorkflowContext::resolve_inputs(&def, &overrides)
        .map_err(|e| CliError::Usage(e.to_string()))?;
    let mut context = WorkflowContext::new(inputs);
    if dry_run {
        for step in &def.steps {
            let prompt =
                render(&step.prompt, &context).map_err(|e| CliError::Usage(e.to_string()))?;
            println!("Step {}: rendered prompt ({} chars)", step.id, prompt.len());
            println!("---\n{}\n---", prompt);
        }
        return Ok(());
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    let runner = CliWorkflowRunner {
        workspace_root: workspace_root.clone(),
        run_id: run_id.clone(),
        sandbox: cli.sandbox,
        mcp_config: cli.mcp_config.clone(),
        quiet: cli.quiet,
        profile_override: profile_override.map(str::to_string),
    };
    let audit_path = workflow_run_path(&def.name, &run_id).ok();
    let summary = run_workflow_exec(&def, &mut context, &runner, audit_path.as_deref()).await?;
    if cli_use_color() {
        println!(
            "{}Workflow completed: {} steps, ${:.4} total, {} ms{}",
            ansi::GREEN,
            summary.step_count,
            summary.total_cost_usd,
            summary.total_duration_ms,
            ansi::RESET
        );
    } else {
        println!(
            "Workflow completed: {} steps, ${:.4} total, {} ms",
            summary.step_count, summary.total_cost_usd, summary.total_duration_ms
        );
    }
    Ok(())
}

fn resolve_workflow_path(workflow: &str) -> anyhow::Result<std::path::PathBuf> {
    let p = Path::new(workflow);
    if p.is_absolute() || p.exists() {
        return Ok(p.to_path_buf());
    }

    // Load config to get the workflows directory (respects [workflows].directory override)
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let workflows_dir = match load_config(&cwd) {
        Ok(loaded) => loaded.workflows.directory,
        Err(_) => default_workflows_directory(),
    };
    let global_base = Path::new(&workflows_dir);
    for candidate in [
        workflow,
        &format!("{}.yaml", workflow),
        &format!("{}.yml", workflow),
    ] {
        let path = global_base.join(candidate);
        if path.exists() {
            return Ok(path);
        }
    }

    Err(anyhow::anyhow!(
        "Workflow not found: {} (tried {}, current path)",
        workflow,
        workflows_dir
    ))
}

async fn run_workflow_validate(path: &Path) -> Result<(), anyhow::Error> {
    let def = load_workflow(path).map_err(|e| CliError::Usage(e.to_string()))?;
    validate_workflow(&def).map_err(|e| CliError::Usage(e.to_string()))?;
    println!("Valid: {}", path.display());
    Ok(())
}

async fn run_workflow_inspect(path: &Path) -> Result<(), anyhow::Error> {
    let def = load_workflow(path).map_err(|e| CliError::Usage(e.to_string()))?;
    println!("Workflow: {} (version {})", def.name, def.version);
    for (i, step) in def.steps.iter().enumerate() {
        let parallel = if step.parallel { " [parallel]" } else { "" };
        println!("  {}. {}{}", i + 1, step.id, parallel);
    }
    Ok(())
}

async fn run_workflow_list() -> Result<(), anyhow::Error> {
    // Load config to get the workflows directory (respects [workflows].directory override)
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let workflows_dir = match load_config(&cwd) {
        Ok(loaded) => loaded.workflows.directory,
        Err(_) => default_workflows_directory(),
    };
    let mut found = Vec::new();
    collect_yaml_files(Path::new(&workflows_dir), &mut found);
    for path in found {
        if let Ok(def) = load_workflow(&path) {
            println!("  {}  {}", def.name, path.display());
        }
    }
    Ok(())
}

async fn run_workflow_check(cli: &Cli, path: &Path, json: bool) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let def = load_workflow(path).map_err(|e| CliError::Usage(e.to_string()))?;
    // Gather available profiles and tools from config
    let (available_profiles, available_tools) = match load_config(&workspace_root) {
        Ok(loaded) => {
            let profiles: Vec<String> = loaded.profiles.keys().cloned().collect();
            let tools: Vec<String> = {
                let blocked = clido_core::global_config_path()
                    .into_iter()
                    .collect::<Vec<_>>();
                let mut reg =
                    default_registry_with_options(workspace_root.clone(), blocked, cli.sandbox);
                reg = crate::agent_setup::load_mcp_tools_from_path(
                    cli.mcp_config.as_deref(),
                    cli.quiet,
                    reg,
                );
                reg.schemas().into_iter().map(|s| s.name).collect()
            };
            (profiles, tools)
        }
        Err(_) => (vec![], vec![]),
    };
    let profile_refs: Vec<&str> = available_profiles.iter().map(String::as_str).collect();
    let tool_refs: Vec<&str> = available_tools.iter().map(String::as_str).collect();
    let result = preflight_workflow(&def, &profile_refs, &tool_refs);

    if json {
        let items: Vec<serde_json::Value> = result
            .checks
            .iter()
            .map(|c| {
                let (status, msg) = match &c.status {
                    PreflightStatus::Pass => ("pass", String::new()),
                    PreflightStatus::Warn(m) => ("warn", m.clone()),
                    PreflightStatus::Fail(m) => ("fail", m.clone()),
                };
                serde_json::json!({ "name": c.name, "status": status, "message": msg })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_default()
        );
    } else {
        for check in &result.checks {
            let (icon, msg) = match &check.status {
                PreflightStatus::Pass => ("PASS", String::new()),
                PreflightStatus::Warn(m) => ("WARN", format!(" — {}", m)),
                PreflightStatus::Fail(m) => ("FAIL", format!(" — {}", m)),
            };
            println!("[{}] {}{}", icon, check.name, msg);
        }
        if result.is_ok() {
            println!("Preflight OK");
        } else {
            println!("Preflight FAILED");
            return Err(CliError::Usage("Preflight failed".into()).into());
        }
    }
    Ok(())
}

fn collect_yaml_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if !dir.exists() {
        return;
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path
                .extension()
                .map(|x| x == "yaml" || x == "yml")
                .unwrap_or(false)
            {
                out.push(path);
            }
        }
    }
}
