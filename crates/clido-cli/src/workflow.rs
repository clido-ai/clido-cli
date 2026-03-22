//! Workflow commands: run, validate, inspect, list.

use async_trait::async_trait;
use clido_agent::AgentLoop;
use clido_core::{
    agent_config_from_loaded, load_config, load_pricing, ClidoError, LoadedConfig, PermissionMode,
    Result as CoreResult,
};
use clido_storage::{workflow_run_path, SessionWriter};
use clido_tools::default_registry;
use clido_workflows::{
    load as load_workflow, preflight as preflight_workflow, render,
    run_workflow as run_workflow_exec, validate as validate_workflow, PreflightStatus,
    StepRunRequest, StepRunResult, WorkflowContext, WorkflowStepRunner,
};
use std::env;
use std::io::Write;
use std::path::Path;

use crate::cli::WorkflowCmd;
use crate::errors::CliError;
use crate::provider::make_provider;
use crate::ui::{ansi, cli_use_color};

pub async fn run_workflow(cmd: &WorkflowCmd) -> Result<(), anyhow::Error> {
    match cmd {
        WorkflowCmd::Run {
            workflow,
            input,
            dry_run,
            yes: _,
        } => run_workflow_run(workflow, input, *dry_run).await,
        WorkflowCmd::Validate { path } => run_workflow_validate(path).await,
        WorkflowCmd::Inspect { path } => run_workflow_inspect(path).await,
        WorkflowCmd::List => run_workflow_list().await,
        WorkflowCmd::Check { path, json } => run_workflow_check(path, *json).await,
    }
}

struct CliWorkflowRunner {
    workspace_root: std::path::PathBuf,
    run_id: String,
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
            .unwrap_or(loaded.default_profile.as_str());
        let profile = loaded
            .get_profile(profile_name)
            .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        LoadedConfig::validate_provider(&profile.provider)
            .map_err(|e| ClidoError::Workflow(e.to_string()))?;
        let provider =
            make_provider(profile_name, profile, None, None).map_err(ClidoError::Workflow)?;
        let model = profile.model.clone();
        let mut registry = default_registry(self.workspace_root.clone());
        registry = registry.with_filters(request.tools, None);
        if registry.schemas().is_empty() {
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
        let mut loop_ = AgentLoop::new(provider, registry, config, None);
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
    workflow: &str,
    input: &[String],
    dry_run: bool,
) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(&workspace_root).map_err(|e| CliError::Usage(e.to_string()))?;
    let path = resolve_workflow_path(workflow, &workspace_root, &loaded.workflows.directory)?;
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

fn resolve_workflow_path(
    workflow: &str,
    workspace_root: &Path,
    project_workflow_dir: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let p = Path::new(workflow);
    if p.is_absolute() || p.exists() {
        return Ok(p.to_path_buf());
    }
    let project_base = workspace_root.join(project_workflow_dir);
    for candidate in [
        workflow,
        &format!("{}.yaml", workflow),
        &format!("{}.yml", workflow),
    ] {
        let path = project_base.join(candidate);
        if path.exists() {
            return Ok(path);
        }
    }
    if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") {
        let global_base = dirs.config_dir().join("workflows");
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
    }
    Err(anyhow::anyhow!(
        "Workflow not found: {} (tried current path, {}, ~/.config/clido/workflows/)",
        workflow,
        project_workflow_dir
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
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let workflow_dir = load_config(&cwd)
        .map(|l| l.workflows.directory)
        .unwrap_or_else(|_| ".clido/workflows".to_string());
    let mut found = Vec::new();
    collect_yaml_files(&cwd.join(&workflow_dir), &mut found);
    if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") {
        collect_yaml_files(&dirs.config_dir().join("workflows"), &mut found);
    }
    for path in found {
        if let Ok(def) = load_workflow(&path) {
            println!("  {}  {}", def.name, path.display());
        }
    }
    Ok(())
}

async fn run_workflow_check(path: &Path, json: bool) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let def = load_workflow(path).map_err(|e| CliError::Usage(e.to_string()))?;
    // Gather available profiles and tools from config
    let (available_profiles, available_tools) = match load_config(&workspace_root) {
        Ok(loaded) => {
            let profiles: Vec<String> = loaded.profiles.keys().cloned().collect();
            let tools: Vec<String> = {
                let reg = clido_tools::default_registry(workspace_root.clone());
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
