//! XDG / platform data and session paths.

use std::path::{Path, PathBuf};

/// Data directory (e.g. ~/.local/share/clido on Linux).
pub fn data_dir() -> anyhow::Result<PathBuf> {
    let dir = directories::ProjectDirs::from("", "", "clido")
        .ok_or_else(|| anyhow::anyhow!("Could not determine project data directory"))?;
    Ok(dir.data_dir().to_path_buf())
}

/// Sanitize a project path for use as an audit/data directory name.
pub fn sanitize_for_audit(project_path: &Path) -> String {
    sanitize_project_path(project_path)
}

/// Sanitize path for use as a directory name (e.g. replace / with _).
fn sanitize_project_path(project_path: &Path) -> String {
    let s = project_path.display().to_string();
    s.chars()
        .map(|c| {
            if c == std::path::MAIN_SEPARATOR {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Session directory for a project: `{data_dir}/sessions/{sanitized_project_path}`.
pub fn session_dir_for_project(project_path: &Path) -> anyhow::Result<PathBuf> {
    let base = data_dir()?;
    let sanitized = sanitize_project_path(project_path);
    Ok(base.join("sessions").join(sanitized))
}

/// Full path to a session file: `{session_dir}/{session_id}.jsonl`.
pub fn session_file_path(project_path: &Path, session_id: &str) -> anyhow::Result<PathBuf> {
    Ok(session_dir_for_project(project_path)?.join(format!("{}.jsonl", session_id)))
}

/// Path for a workflow run audit file: `{data_dir}/workflows/{workflow_name}/{run_id}.json`.
pub fn workflow_run_path(workflow_name: &str, run_id: &str) -> anyhow::Result<PathBuf> {
    let base = data_dir()?;
    let sanitized_name = workflow_name
        .chars()
        .map(|c| {
            if c == std::path::MAIN_SEPARATOR {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    Ok(base
        .join("workflows")
        .join(sanitized_name)
        .join(format!("{}.json", run_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_separators() {
        let p = Path::new("/foo/bar");
        assert_eq!(sanitize_project_path(p), "_foo_bar");
    }
}
