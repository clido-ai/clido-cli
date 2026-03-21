//! Memory management CLI commands.

use clido_memory::MemoryStore;
use std::path::PathBuf;

use crate::cli::MemoryCmd;
use crate::errors::CliError;

/// Return path to the global memory database.
fn memory_db_path() -> anyhow::Result<PathBuf> {
    if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") {
        let data = dirs.data_dir().to_path_buf();
        std::fs::create_dir_all(&data)?;
        return Ok(data.join("memory.db"));
    }
    // Fallback: use current directory
    Ok(PathBuf::from(".clido-memory.db"))
}

pub fn run_memory(cmd: &MemoryCmd) -> Result<(), anyhow::Error> {
    match cmd {
        MemoryCmd::List { limit } => run_memory_list(*limit),
        MemoryCmd::Prune { keep } => run_memory_prune(*keep),
        MemoryCmd::Reset { force } => run_memory_reset(*force),
    }
}

fn run_memory_list(limit: usize) -> Result<(), anyhow::Error> {
    let path = memory_db_path()?;
    let store = MemoryStore::open(&path).map_err(|e| CliError::Usage(e.to_string()))?;
    let entries = store.list(limit).map_err(|e| CliError::Usage(e.to_string()))?;
    if entries.is_empty() {
        println!("No memories stored.");
        return Ok(());
    }
    for entry in &entries {
        let tags = if entry.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", entry.tags.join(", "))
        };
        println!("[{}]{} {}", entry.created_at, tags, entry.content);
    }
    println!("\n{} memory/memories.", entries.len());
    Ok(())
}

fn run_memory_prune(keep: Option<usize>) -> Result<(), anyhow::Error> {
    let keep_n = keep.unwrap_or(100);
    let path = memory_db_path()?;
    let mut store = MemoryStore::open(&path).map_err(|e| CliError::Usage(e.to_string()))?;
    let deleted = store.prune_old(keep_n).map_err(|e| CliError::Usage(e.to_string()))?;
    println!("Pruned {} memories (keeping {} most recent).", deleted, keep_n);
    Ok(())
}

fn run_memory_reset(force: bool) -> Result<(), anyhow::Error> {
    if !force {
        use std::io::{self, Write};
        print!("Reset all memories? This cannot be undone. [y/N] ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }
    let path = memory_db_path()?;
    let mut store = MemoryStore::open(&path).map_err(|e| CliError::Usage(e.to_string()))?;
    store.reset().map_err(|e| CliError::Usage(e.to_string()))?;
    println!("Memory reset.");
    Ok(())
}
