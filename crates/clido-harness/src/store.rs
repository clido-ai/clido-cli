//! Filesystem layout under `<workspace>/.clido/harness/`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::state::HarnessState;

fn harness_dir(workspace: &Path) -> PathBuf {
    workspace.join(".clido").join("harness")
}

pub fn tasks_path(workspace: &Path) -> PathBuf {
    harness_dir(workspace).join("tasks.json")
}

pub fn progress_path(workspace: &Path) -> PathBuf {
    harness_dir(workspace).join("progress.ndjson")
}

pub fn read_state(workspace: &Path) -> Result<HarnessState> {
    let p = tasks_path(workspace);
    if !p.exists() {
        return Ok(HarnessState::empty());
    }
    let raw = fs::read_to_string(&p)?;
    let s: HarnessState = serde_json::from_str(&raw)?;
    Ok(s)
}

pub fn write_state(workspace: &Path, state: &HarnessState) -> Result<()> {
    let dir = harness_dir(workspace);
    fs::create_dir_all(&dir)?;
    let p = tasks_path(workspace);
    let tmp = p.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(state)?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

pub fn append_progress(workspace: &Path, line: &serde_json::Value) -> Result<()> {
    let dir = harness_dir(workspace);
    fs::create_dir_all(&dir)?;
    let p = progress_path(workspace);
    let mut f = OpenOptions::new().create(true).append(true).open(&p)?;
    writeln!(f, "{}", serde_json::to_string(line)?)?;
    Ok(())
}

pub fn read_progress_tail(workspace: &Path, max_lines: usize) -> Result<String> {
    let p = progress_path(workspace);
    if !p.exists() || max_lines == 0 {
        return Ok(String::new());
    }
    let s = fs::read_to_string(&p)?;
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    Ok(lines[start..].join("\n"))
}

/// Recent git commits for session handoff (best-effort).
pub fn git_log_snippet(workspace: &Path, n: u32) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "-n", &n.to_string(), "--oneline", "--no-decorate"])
        .current_dir(workspace)
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "(git log unavailable — not a repo or git missing)".to_string(),
    }
}

pub fn touch_meta_timestamp(state: &mut HarnessState) {
    state.meta.updated_at_rfc3339 = Some(chrono::Utc::now().to_rfc3339());
}

/// Repair `task_order` against `tasks`: drop order entries whose id no longer exists in `tasks`,
/// append any task id missing from `task_order` at the end.
///
/// This does **not** remove rows from `tasks` — only fixes the order list after manual file edits.
/// Normal tool operations never delete tasks.
pub fn reconcile_order(state: &mut HarnessState) {
    let ids: std::collections::HashSet<&str> = state.tasks.iter().map(|t| t.id.as_str()).collect();
    state.task_order.retain(|id| ids.contains(id.as_str()));
    for t in &state.tasks {
        if !state.task_order.contains(&t.id) {
            state.task_order.push(t.id.clone());
        }
    }
}
