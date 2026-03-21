//! Edit tool: replace old_string with new_string in file.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use std::path::PathBuf;

use crate::file_tracker::FileTracker;
use crate::path_guard::PathGuard;
use crate::secrets::scan_for_secrets;
use crate::{Tool, ToolOutput};

pub struct EditTool {
    guard: PathGuard,
    tracker: Option<FileTracker>,
}

impl EditTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            guard: PathGuard::new(workspace_root),
            tracker: None,
        }
    }
    pub fn new_with_guard(guard: PathGuard) -> Self {
        Self { guard, tracker: None }
    }
    pub fn new_with_tracker(guard: PathGuard, tracker: FileTracker) -> Self {
        Self { guard, tracker: Some(tracker) }
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        "Replace old_string with new_string in file. Use replace_all for multiple occurrences."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to the file to edit (relative to cwd)" },
                "old_string": { "type": "string", "description": "Exact string to replace" },
                "new_string": { "type": "string", "description": "Replacement string" },
                "replace_all": { "type": "boolean", "default": false, "description": "Replace all occurrences instead of just the first" }
            },
            "required": ["file_path", "old_string", "new_string"]
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
        let old_string = input
            .get("old_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_string = input
            .get("new_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let replace_all = input
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if path_str.is_empty() {
            return ToolOutput::err("Missing required field: file_path or path".to_string());
        }
        if old_string.is_empty() {
            return ToolOutput::err("Missing required field: old_string".to_string());
        }

        let path = match self.guard.resolve_and_check(path_str) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };

        // Check for external modification before reading+writing.
        if let Some(ref tracker) = self.tracker {
            if let Some(err) = tracker.check_not_stale(&path) {
                return ToolOutput::err(err);
            }
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(e.to_string()),
        };

        // Secret detection: warn on new_string content, but do not block
        let findings = scan_for_secrets(new_string);
        for finding in &findings {
            eprintln!(
                "Warning: potential secret detected in edit content: {}",
                finding
            );
        }

        let old_content = content.clone();
        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            if let Some(pos) = content.find(old_string) {
                let mut out = content;
                out.replace_range(pos..pos + old_string.len(), new_string);
                out
            } else {
                return ToolOutput::err(format!(
                    "<tool_use_error>String to replace not found in file.\nString: {}</tool_use_error>",
                    old_string
                ));
            }
        };

        if let Err(e) = tokio::fs::write(&path, &new_content).await {
            return ToolOutput::err(e.to_string());
        }

        let hash = hex::encode(Sha256::digest(new_content.as_bytes()));
        let mtime_nanos = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // Update tracker so subsequent edits to the same file in one session don't false-alarm.
        if let Some(ref tracker) = self.tracker {
            tracker.update(&path, mtime_nanos);
        }
        let diff = build_unified_diff(path_str, &old_content, &new_content);
        let mut out = ToolOutput::ok_with_meta(
            format!("The file {} has been updated successfully.", path.display()),
            path.display().to_string(),
            hash,
            mtime_nanos,
        );
        out.diff = Some(diff);
        out
    }
}

/// Produce a compact unified diff string (±5 context lines).
fn build_unified_diff(path: &str, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    for group in diff.grouped_ops(3) {
        // Header: --- a/path  +++ b/path
        if out.is_empty() {
            out.push_str(&format!("--- a/{}\n+++ b/{}\n", path, path));
        }
        let first = group.first().unwrap();
        let last = group.last().unwrap();
        let old_start = first.old_range().start + 1;
        let old_len: usize = group.iter().map(|op| op.old_range().len()).sum();
        let new_start = first.new_range().start + 1;
        let new_len: usize = group.iter().map(|op| op.new_range().len()).sum();
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start, old_len, new_start, new_len
        ));
        let _ = last; // suppress unused warning
        for op in &group {
            for change in diff.iter_changes(op) {
                let prefix = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                out.push_str(prefix);
                out.push_str(change.value());
                if !change.value().ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn edit_basic_replace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "hello world").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.txt",
                "old_string": "world",
                "new_string": "rust"
            }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello rust");
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "a a a").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.txt",
                "old_string": "a",
                "new_string": "b",
                "replace_all": true
            }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b b b");
    }

    #[tokio::test]
    async fn edit_string_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "hello").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "file_path": "f.txt",
                "old_string": "not_there",
                "new_string": "x"
            }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("not found"));
    }

    #[tokio::test]
    async fn edit_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "old_string": "x", "new_string": "y" }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Missing"));
    }

    #[tokio::test]
    async fn edit_missing_old_string() {
        let dir = tempfile::tempdir().unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "file_path": "f.txt", "new_string": "y" }))
            .await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn edit_path_alias() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.txt");
        std::fs::write(&path, "foo bar").unwrap();
        let tool = EditTool::new(dir.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({
                "path": "g.txt",
                "old_string": "foo",
                "new_string": "baz"
            }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "baz bar");
    }
}
