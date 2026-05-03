//! Write tool: create or overwrite file.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::file_tracker::FileTracker;
use crate::path_guard::PathGuard;
use crate::secrets::{scan_for_secrets, secret_findings_prefix};
use crate::{Tool, ToolOutput};

pub struct WriteTool {
    guard: PathGuard,
    tracker: Option<FileTracker>,
}

impl WriteTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            guard: PathGuard::new(workspace_root),
            tracker: None,
        }
    }
    pub fn new_with_guard(guard: PathGuard) -> Self {
        Self {
            guard,
            tracker: None,
        }
    }
    pub fn new_with_tracker(guard: PathGuard, tracker: FileTracker) -> Self {
        Self {
            guard,
            tracker: Some(tracker),
        }
    }
}

/// Return the first `max_lines` lines of `content` as a preview.
fn content_preview(content: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = content.lines().take(max_lines).collect();
    let preview = lines.join("\n");
    if content.lines().count() > max_lines {
        format!(
            "{preview}\n... ({} more lines)",
            content.lines().count() - max_lines
        )
    } else {
        preview
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> &str {
        "Create a new file or overwrite existing file with content."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to the file to write (relative to cwd)" },
                "content": { "type": "string", "description": "Full content to write to the file" }
            },
            "required": ["file_path", "content"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let path_str = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");

        if path_str.is_empty() {
            return ToolOutput::err("Missing required field: file_path or path".to_string());
        }

        // Secret detection: warn but do not block (message in tool output + tracing; no stderr)
        let findings = scan_for_secrets(content);
        if !findings.is_empty() {
            tracing::warn!(
                tool = "Write",
                ?findings,
                "potential secrets in tool content"
            );
        }
        let secret_prefix = secret_findings_prefix(&findings);

        let path = match self.guard.resolve_for_write(path_str) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };

        // Check for external modification only for files that were previously read.
        if let Some(ref tracker) = self.tracker {
            if let Some(err) = tracker.check_not_stale(&path) {
                return ToolOutput::err(err);
            }
        }

        if let Some(parent) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return ToolOutput::err(e.to_string());
            }
        }

        match tokio::fs::write(&path, content).await {
            Ok(()) => {
                let hash = hex::encode(Sha256::digest(content.as_bytes()));
                let mtime_nanos = tokio::fs::metadata(&path)
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                // Update tracker so the next write to this file (same session) doesn't false-alarm.
                if let Some(ref tracker) = self.tracker {
                    tracker.update(&path, mtime_nanos);
                }
                // Include a content preview so the user immediately sees what was written.
                let preview = content_preview(content, 15);
                let mut msg = format!("{}File written successfully.", secret_prefix);
                if !preview.is_empty() {
                    msg.push_str(&format!(
                        "\n\n--- Preview of {} ({}) ---\n{}\n--- End preview ---",
                        path.display(),
                        content.len(),
                        preview
                    ));
                }
                ToolOutput::ok_with_meta(msg, path.display().to_string(), hash, mtime_nanos)
            }
            Err(e) => ToolOutput::err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "file_path": "new.txt", "content": "hello" }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn write_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "old").unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "file_path": "f.txt", "content": "new" }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[tokio::test]
    async fn write_missing_path_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let out = tool.execute(serde_json::json!({ "content": "data" })).await;
        assert!(out.is_error);
        assert!(out.content.contains("Missing"));
    }

    #[tokio::test]
    async fn write_path_alias() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "path": "alias.txt", "content": "via alias" }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("alias.txt")).unwrap(),
            "via alias"
        );
    }

    /// Line 24: new_with_guard constructor.
    #[test]
    fn write_tool_new_with_guard() {
        use crate::path_guard::PathGuard;
        let dir = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(dir.path().to_path_buf());
        let tool = WriteTool::new_with_guard(guard);
        assert_eq!(tool.name(), "Write");
    }

    /// Lines 59-60: is_read_only returns false.
    #[test]
    fn write_tool_is_not_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        assert!(!tool.is_read_only());
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "file_path": "sub/dir/file.txt", "content": "deep" }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sub/dir/file.txt")).unwrap(),
            "deep"
        );
    }
}
