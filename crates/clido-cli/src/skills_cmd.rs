//! CLI entry points for `clido skills`.

use std::path::Path;

use clido_core::skills::{
    discover_skills, is_skill_active_for_config, resolve_skill_directories, SkillSourceKind,
};
use clido_core::{load_config, set_skill_disabled_in_project, SkillsSection};

use crate::cli::SkillsCmd;

fn truncate_summary_line(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let take = max_chars.saturating_sub(1);
    format!("{}…", s.chars().take(take).collect::<String>())
}

pub fn run_skills(cmd: SkillsCmd, workspace: &Path) -> anyhow::Result<()> {
    let cfg: SkillsSection = load_config(workspace).map(|c| c.skills).unwrap_or_default();

    match cmd {
        SkillsCmd::List => {
            let skills =
                discover_skills(workspace, &cfg.extra_paths).map_err(anyhow::Error::msg)?;
            if skills.is_empty() {
                println!(
                    "No skills found. Add .md or .txt files under .clido/skills/ or ~/.clido/skills/"
                );
                return Ok(());
            }
            if cfg.no_skills {
                println!("Note: [skills] no-skills = true — nothing is injected until you turn that off.");
            }
            if !cfg.enabled.is_empty() {
                println!(
                    "Note: whitelist mode — only {:?} can be active.",
                    cfg.enabled
                );
            }
            println!("{:<8} {:<24} summary (first line)", "active", "id");
            for s in skills {
                let on = is_skill_active_for_config(&s.manifest.id, &cfg);
                let sum_short = truncate_summary_line(&s.manifest.description, 56);
                println!(
                    "{:<8} {:<24} {}",
                    if on { "yes" } else { "no" },
                    s.manifest.id,
                    sum_short
                );
            }
            println!("\nRestart the agent session to pick up file changes.");
        }
        SkillsCmd::Paths => {
            let dirs = resolve_skill_directories(workspace, &cfg.extra_paths);
            println!("Skill search paths (in merge order, first wins per id):");
            for (p, kind) in dirs {
                let label = match kind {
                    SkillSourceKind::Workspace => "workspace",
                    SkillSourceKind::Global => "global",
                    SkillSourceKind::Extra => "extra",
                };
                let exists = if p.is_dir() { "exists" } else { "missing" };
                println!("  [{label}] {exists}: {}", p.display());
            }
            if !cfg.registry_urls.is_empty() {
                println!("Configured registry URLs (reserved, not fetched yet):");
                for u in &cfg.registry_urls {
                    println!("  {u}");
                }
            }
        }
        SkillsCmd::Disable { id } => {
            set_skill_disabled_in_project(workspace, &id, true).map_err(anyhow::Error::msg)?;
            println!(
                "Disabled skill `{id}` in {}.",
                workspace.join(".clido/config.toml").display()
            );
            println!("Restart the agent to apply.");
        }
        SkillsCmd::Enable { id } => {
            set_skill_disabled_in_project(workspace, &id, false).map_err(anyhow::Error::msg)?;
            println!("Removed `{id}` from disabled list in project config.");
            println!("Restart the agent to apply.");
        }
    }
    Ok(())
}
