//! Write tool: create or overwrite file.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::file_tracker::FileTracker;
use crate::path_guard::PathGuard;
use crate::secrets::scan_for_secrets;
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

        // Secret detection: warn but do not block
        let findings = scan_for_secrets(content);
        for finding in &findings {
            eprintln!(
                "Warning: potential secret detected in write content: {}",
                finding
            );
        }

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
                ToolOutput::ok_with_meta(
                    "File written successfully.".to_string(),
                    path.display().to_string(),
                    hash,
                    mtime_nanos,
                )
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
