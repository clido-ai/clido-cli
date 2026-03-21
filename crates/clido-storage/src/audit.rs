//! Append-only audit log for tool calls.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: String,
    pub session_id: String,
    pub tool_name: String,
    pub input_summary: String,
    pub is_error: bool,
    pub duration_ms: u64,
}

pub struct AuditLog {
    file: std::fs::File,
}

impl AuditLog {
    /// Open (or create) the audit log for a project.
    pub fn open(project_path: &Path) -> anyhow::Result<Self> {
        let dir = crate::paths::data_dir()?.join("audit").join(
            crate::paths::sanitize_for_audit(project_path)
        );
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("audit.jsonl");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self { file })
    }

    pub fn append(&mut self, entry: &AuditEntry) -> anyhow::Result<()> {
        serde_json::to_writer(&mut self.file, entry)?;
        self.file.write_all(b"\n")?;
        Ok(())
    }
}
