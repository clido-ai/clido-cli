//! `clido index` subcommand: build, stats, clear.

use std::path::PathBuf;

use clido_index::RepoIndex;

use crate::cli::IndexCmd;

pub async fn run_index(cmd: &IndexCmd) -> anyhow::Result<()> {
    let workspace_root = std::env::current_dir()?;
    let db_path = workspace_root.join(".clido").join("index.db");
    std::fs::create_dir_all(db_path.parent().unwrap())?;

    match cmd {
        IndexCmd::Build { dir, ext } => {
            let target: PathBuf = dir.clone().unwrap_or_else(|| workspace_root.clone());
            let exts: Vec<&str> = ext.split(',').map(|s| s.trim()).collect();
            let mut index = RepoIndex::open(&db_path)?;
            let count = index.build(&target, &exts)?;
            println!("Indexed {} files in {}", count, target.display());
            let (files, symbols) = index.stats()?;
            println!("  {} files, {} symbols", files, symbols);
        }
        IndexCmd::Stats => {
            if !db_path.exists() {
                println!("No index found. Run `clido index build` first.");
                return Ok(());
            }
            let index = RepoIndex::open(&db_path)?;
            let (files, symbols) = index.stats()?;
            println!("Index: {} files, {} symbols", files, symbols);
        }
        IndexCmd::Clear => {
            if db_path.exists() {
                std::fs::remove_file(&db_path)?;
                println!("Index cleared.");
            } else {
                println!("No index found.");
            }
        }
    }
    Ok(())
}
