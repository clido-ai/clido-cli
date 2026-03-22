//! Handler for `clido checkpoint` and `clido rollback` subcommands.

use crate::cli::{CheckpointCmd, Cli};
use clido_checkpoint::CheckpointStore;

fn resolve_store(cli: &Cli, session_id: Option<&str>) -> anyhow::Result<CheckpointStore> {
    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));

    let sid =
        session_id.ok_or_else(|| anyhow::anyhow!("No session ID provided. Use --session <id>."))?;

    let store_dir = workspace_root.join(".clido").join("checkpoints").join(sid);

    Ok(CheckpointStore::new(store_dir))
}

pub async fn run_checkpoint(cmd: &CheckpointCmd, cli: &Cli) -> anyhow::Result<()> {
    match cmd {
        CheckpointCmd::List { session } => {
            let store = resolve_store(cli, session.as_deref())?;
            let metas = store.list().map_err(|e| anyhow::anyhow!("{}", e))?;
            if metas.is_empty() {
                println!("No checkpoints found.");
            } else {
                println!("{:<20}  {:<30}  {:<6}  Created", "ID", "Name", "Files");
                for m in &metas {
                    println!(
                        "{:<20}  {:<30}  {:<6}  {}",
                        m.id,
                        m.name.as_deref().unwrap_or("-"),
                        m.file_count,
                        m.created_at,
                    );
                }
            }
        }
        CheckpointCmd::Save { name } => {
            // Save with no files: stores an empty checkpoint (user can specify files via the API).
            let workspace_root = cli
                .workdir
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            // Without a session context here, save to a "manual" session dir.
            let store_dir = workspace_root
                .join(".clido")
                .join("checkpoints")
                .join("manual");
            let store = CheckpointStore::new(store_dir);
            let cp = store
                .create(name.as_deref(), false, &[])
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Checkpoint saved: {}", cp.id);
        }
        CheckpointCmd::Rollback { id, yes } => {
            let workspace_root = cli
                .workdir
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            let store_dir = workspace_root
                .join(".clido")
                .join("checkpoints")
                .join("manual");
            let store = CheckpointStore::new(store_dir);

            let checkpoint_id = id.as_deref().ok_or_else(|| {
                anyhow::anyhow!("Checkpoint ID required. Use: clido checkpoint rollback <id>")
            })?;

            if !yes {
                let diffs = store
                    .diff_since(checkpoint_id)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                if diffs.is_empty() {
                    println!("No changes since checkpoint {}.", checkpoint_id);
                    return Ok(());
                }
                println!("Changes since checkpoint {}:", checkpoint_id);
                for d in &diffs {
                    println!("  {}", d.path.display());
                }
                print!("Roll back {} file(s)? [y/N] ", diffs.len());
                std::io::Write::flush(&mut std::io::stdout())?;
                let mut line = String::new();
                std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)?;
                if !line.trim().eq_ignore_ascii_case("y") {
                    println!("Rollback cancelled.");
                    return Ok(());
                }
            }

            let restored = store
                .restore(checkpoint_id)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!(
                "Restored {} file(s) from checkpoint {}.",
                restored.len(),
                checkpoint_id
            );
        }
        CheckpointCmd::Diff { id } => {
            let workspace_root = cli
                .workdir
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            let store_dir = workspace_root
                .join(".clido")
                .join("checkpoints")
                .join("manual");
            let store = CheckpointStore::new(store_dir);
            let diffs = store.diff_since(id).map_err(|e| anyhow::anyhow!("{}", e))?;
            if diffs.is_empty() {
                println!("No changes since checkpoint {}.", id);
            } else {
                for d in &diffs {
                    println!("--- {}", d.path.display());
                    println!("+++ {}", d.path.display());
                    for line in d.old_content.lines() {
                        println!("-{}", line);
                    }
                    for line in d.new_content.lines() {
                        println!("+{}", line);
                    }
                    println!();
                }
            }
        }
    }
    Ok(())
}

pub async fn run_rollback(
    id: Option<&str>,
    session: Option<&str>,
    yes: bool,
    cli: &Cli,
) -> anyhow::Result<()> {
    // Top-level `clido rollback` uses session to find the checkpoint store.
    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));

    let sid = session.unwrap_or("manual");
    let store = CheckpointStore::new(workspace_root.join(".clido").join("checkpoints").join(sid));

    let checkpoint_id = id.ok_or_else(|| {
        anyhow::anyhow!("Checkpoint ID required. Usage: clido rollback <checkpoint-id>")
    })?;

    if !yes {
        let diffs = store
            .diff_since(checkpoint_id)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        if diffs.is_empty() {
            println!("No changes since checkpoint {}.", checkpoint_id);
            return Ok(());
        }
        println!("Changes since checkpoint {}:", checkpoint_id);
        for d in &diffs {
            println!("  {}", d.path.display());
        }
        print!("Roll back {} file(s)? [y/N] ", diffs.len());
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            println!("Rollback cancelled.");
            return Ok(());
        }
    }

    let restored = store
        .restore(checkpoint_id)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    println!(
        "Restored {} file(s) from checkpoint {}.",
        restored.len(),
        checkpoint_id
    );
    Ok(())
}
