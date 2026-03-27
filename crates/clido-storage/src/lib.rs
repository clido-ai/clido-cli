//! Storage: session JSONL, paths (XDG data dir).

pub mod audit;
mod paths;
mod session;

pub use audit::{AuditEntry, AuditLog};
pub use paths::{
    data_dir, sanitize_for_audit, session_dir_for_project, session_file_path, workflow_run_path,
};
pub use session::{
    list_sessions, redact_secrets, stale_paths, SessionLine, SessionReader, SessionSummary,
    SessionWriter, StaleFileRecord, SCHEMA_VERSION,
};

use std::path::Path;

/// Path to the audit log file for a project.
pub fn audit_log_path(project_path: &Path) -> anyhow::Result<std::path::PathBuf> {
    let dir = data_dir()?
        .join("audit")
        .join(paths::sanitize_for_audit(project_path));
    Ok(dir.join("audit.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_path_ends_with_audit_jsonl() {
        let result = audit_log_path(std::path::Path::new("/some/project"));
        assert!(
            result.is_ok(),
            "audit_log_path should succeed: {:?}",
            result
        );
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with("audit.jsonl"));
    }
}
