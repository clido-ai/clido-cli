//! XDG / platform data and session paths.

use std::path::{Path, PathBuf};

/// Data directory (e.g. ~/.local/share/clido on Linux).
///
/// Overridden by `CLIDO_DATA_DIR` if set, e.g.:
/// `CLIDO_DATA_DIR=/mnt/fast/clido clido <prompt>`
pub fn data_dir() -> anyhow::Result<PathBuf> {
    if let Ok(val) = std::env::var("CLIDO_DATA_DIR") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }
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
///
/// Overridden by `CLIDO_SESSION_DIR` if set. When set, all sessions for all
/// projects share that single directory (no per-project subdirectory is appended).
pub fn session_dir_for_project(project_path: &Path) -> anyhow::Result<PathBuf> {
    if let Ok(val) = std::env::var("CLIDO_SESSION_DIR") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }
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
    use std::sync::Mutex;

    // Serialize tests that mutate environment variables to prevent races.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn sanitize_separators() {
        let p = Path::new("/foo/bar");
        assert_eq!(sanitize_project_path(p), "_foo_bar");
    }

    #[test]
    fn sanitize_plain_name_unchanged() {
        let p = Path::new("myproject");
        assert_eq!(sanitize_project_path(p), "myproject");
    }

    #[test]
    fn sanitize_for_audit_same_as_internal() {
        let p = Path::new("/home/user/project");
        assert_eq!(sanitize_for_audit(p), sanitize_project_path(p));
    }

    #[test]
    fn session_file_path_contains_session_id_and_jsonl() {
        let p = Path::new("/tmp/myproject");
        let path = session_file_path(p, "sess-001").unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        assert_eq!(name, "sess-001.jsonl");
    }

    #[test]
    fn session_dir_for_project_ends_with_sanitized_path() {
        let _g = ENV_LOCK.lock().unwrap();
        // Unset env override so we hit the default XDG path.
        std::env::remove_var("CLIDO_DATA_DIR");
        std::env::remove_var("CLIDO_SESSION_DIR");
        let p = Path::new("/tmp/myproject");
        let dir = session_dir_for_project(p).unwrap();
        let dir_str = dir.to_string_lossy().to_string();
        assert!(
            dir_str.contains("sessions"),
            "expected 'sessions' in path: {}",
            dir_str
        );
    }

    #[test]
    fn clido_data_dir_env_overrides_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CLIDO_DATA_DIR", "/tmp/clido-test-data");
        std::env::remove_var("CLIDO_SESSION_DIR");
        let d = data_dir().unwrap();
        std::env::remove_var("CLIDO_DATA_DIR");
        assert_eq!(d, PathBuf::from("/tmp/clido-test-data"));
    }

    #[test]
    fn clido_session_dir_env_overrides_project_subdir() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLIDO_DATA_DIR");
        std::env::set_var("CLIDO_SESSION_DIR", "/tmp/clido-test-sessions");
        let dir = session_dir_for_project(Path::new("/any/project")).unwrap();
        std::env::remove_var("CLIDO_SESSION_DIR");
        assert_eq!(dir, PathBuf::from("/tmp/clido-test-sessions"));
    }

    #[test]
    fn clido_data_dir_empty_falls_back_to_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CLIDO_DATA_DIR", "");
        std::env::remove_var("CLIDO_SESSION_DIR");
        // Should not error — falls back to XDG default.
        let result = data_dir();
        std::env::remove_var("CLIDO_DATA_DIR");
        assert!(result.is_ok());
    }

    #[test]
    fn workflow_run_path_contains_workflow_name_and_run_id() {
        let path = workflow_run_path("my-workflow", "run-001").unwrap();
        let path_str = path.to_string_lossy().to_string();
        assert!(path_str.contains("my-workflow"), "path: {}", path_str);
        assert!(path_str.ends_with("run-001.json"), "path: {}", path_str);
    }

    #[test]
    fn workflow_run_path_sanitizes_separators_in_name() {
        let path = workflow_run_path("a/b", "r1").unwrap();
        let path_str = path.to_string_lossy().to_string();
        // The slash in "a/b" should be replaced with "_"
        assert!(
            !path_str.contains("a/b"),
            "slash should be sanitized: {}",
            path_str
        );
    }
}
