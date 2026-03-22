//! Handler for `clido plan` subcommands.

use crate::cli::{Cli, PlanCmd};
use clido_planner::{delete_plan, list_plans, load_plan, TaskStatus};
use std::path::Path;

pub async fn run_plan(cmd: &PlanCmd, cli: &Cli) -> anyhow::Result<()> {
    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));

    match cmd {
        PlanCmd::List => run_plan_list(&workspace_root),
        PlanCmd::Show { id } => run_plan_show(&workspace_root, id),
        PlanCmd::Run { id } => run_plan_run(&workspace_root, id, cli).await,
        PlanCmd::Delete { id } => run_plan_delete(&workspace_root, id),
    }
}

fn run_plan_list(workspace_root: &Path) -> anyhow::Result<()> {
    let summaries = list_plans(workspace_root).map_err(|e| anyhow::anyhow!("{}", e))?;
    if summaries.is_empty() {
        println!("No saved plans. Use --plan with a prompt to generate and save one.");
        return Ok(());
    }
    println!(
        "{:<20}  {:<8}  {:<8}  {:<8}  Goal",
        "ID", "Tasks", "Done", "Failed"
    );
    for s in &summaries {
        println!(
            "{:<20}  {:<8}  {:<8}  {:<8}  {}",
            s.id,
            s.task_count,
            s.done,
            s.failed,
            if s.goal.len() > 60 {
                format!("{}…", &s.goal[..59])
            } else {
                s.goal.clone()
            }
        );
    }
    Ok(())
}

fn run_plan_show(workspace_root: &Path, id: &str) -> anyhow::Result<()> {
    let plan = load_plan(workspace_root, id).map_err(|e| anyhow::anyhow!("{}", e))?;
    println!("Plan: {}", plan.meta.id);
    println!("Goal: {}", plan.meta.goal);
    println!("Created: {}", plan.meta.created_at);
    println!();

    let total = plan.tasks.len();
    let done = plan
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();
    let failed = plan
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Failed)
        .count();
    let skipped = plan
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Skipped)
        .count();
    let pending = plan
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .count();

    println!(
        "Progress: {}/{} done  {} failed  {} skipped  {} pending",
        done, total, failed, skipped, pending
    );
    println!();

    for task in &plan.tasks {
        let icon = match task.status {
            TaskStatus::Pending => "○",
            TaskStatus::Running => "↻",
            TaskStatus::Done => "✓",
            TaskStatus::Failed => "✗",
            TaskStatus::Skipped => "⊘",
        };
        let complexity_badge = match task.complexity {
            clido_planner::Complexity::Low => "[low]  ",
            clido_planner::Complexity::Medium => "[med]  ",
            clido_planner::Complexity::High => "[high] ",
        };
        let skip_note = if task.skip { "  (skip)" } else { "" };
        let deps = if task.depends_on.is_empty() {
            String::new()
        } else {
            format!("  → needs {}", task.depends_on.join(", "))
        };
        println!(
            "  {} {}  {}  {}{}{}",
            icon, task.id, complexity_badge, task.description, skip_note, deps
        );
        if !task.notes.is_empty() {
            println!("       note: {}", task.notes);
        }
    }
    Ok(())
}

async fn run_plan_run(workspace_root: &Path, id: &str, cli: &Cli) -> anyhow::Result<()> {
    let plan = load_plan(workspace_root, id).map_err(|e| anyhow::anyhow!("{}", e))?;

    let pending_tasks: Vec<_> = plan
        .tasks
        .iter()
        .filter(|t| !matches!(t.status, TaskStatus::Done | TaskStatus::Skipped))
        .collect();

    if pending_tasks.is_empty() {
        println!("All tasks in plan '{}' are already done.", id);
        return Ok(());
    }

    println!("Running plan: {}", plan.meta.goal);
    println!("Executing {} pending task(s)…\n", pending_tasks.len());

    // Build a combined prompt with all pending task descriptions.
    let mut prompt_parts = vec![format!("Goal: {}", plan.meta.goal), String::new()];
    for (i, task) in pending_tasks.iter().enumerate() {
        prompt_parts.push(format!(
            "Task {}: {}{}",
            i + 1,
            task.description,
            if task.notes.is_empty() {
                String::new()
            } else {
                format!(" (note: {})", task.notes)
            }
        ));
    }
    let prompt = prompt_parts.join("\n");

    // Run using the regular agent via the run module.
    let mut run_cli = cli.clone();
    run_cli.subcommand = None;
    run_cli.prompt = prompt.split_whitespace().map(|s| s.to_string()).collect();

    crate::run::run_agent(run_cli).await
}

fn run_plan_delete(workspace_root: &Path, id: &str) -> anyhow::Result<()> {
    delete_plan(workspace_root, id).map_err(|e| anyhow::anyhow!("{}", e))?;
    println!("Deleted plan '{}'.", id);
    Ok(())
}
