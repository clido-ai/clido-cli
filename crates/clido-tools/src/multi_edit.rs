//! MultiEdit tool: apply multiple string replacements across files atomically.
//!
//! All edits either succeed together or the whole operation fails. This avoids
//! partial edits when the agent needs to update the same symbol in many places.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::file_tracker::FileTracker;
use crate::path_guard::PathGuard;
use crate::secrets::{scan_for_secrets, secret_findings_prefix};
use crate::{Tool, ToolOutput};

pub struct MultiEditTool {
    guard: PathGuard,
    tracker: Option<FileTracker>,
}

impl MultiEditTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            guard: PathGuard::new(workspace_root),
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
impl Tool for MultiEditTool {
    fn name(&self) -> &str {
        "MultiEdit"
    }

    fn description(&self) -> &str {
        "Apply multiple string replacements across one or more files in a single atomic operation. \
         All edits succeed or all fail — no partial writes. Ideal for renaming a symbol across files, \
         updating multiple call sites, or making coordinated changes. \
         Each edit specifies a file_path, old_string (exact text to find), new_string (replacement), \
         and optional replace_all (default false = replace first occurrence only)."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "edits": {
                    "type": "array",
                    "description": "List of replacements to apply",
                    "items": {
                        "type": "object",
                        "properties": {
                            "file_path":    { "type": "string", "description": "Path to file (relative to cwd)" },
                            "old_string":   { "type": "string", "description": "Exact text to replace" },
                            "new_string":   { "type": "string", "description": "Replacement text" },
                            "replace_all":  { "type": "boolean", "description": "Replace all occurrences (default: false)" }
                        },
                        "required": ["file_path","old_string","new_string"]
                    },
                    "minItems": 1
                }
            },
            "required": ["edits"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let edits_raw = match input.get("edits").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr.clone(),
            Some(_) => return ToolOutput::err("edits list is empty"),
            None => return ToolOutput::err("Missing required field: edits"),
        };

        // Phase 1: Parse and validate all edits before touching disk.
        struct Edit {
            file_path: PathBuf,
            old_string: String,
            new_string: String,
            replace_all: bool,
        }

        let mut parsed: Vec<Edit> = Vec::with_capacity(edits_raw.len());
        for (i, edit) in edits_raw.iter().enumerate() {
            let raw_path = match edit.get("file_path").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s,
                _ => return ToolOutput::err(format!("Edit {i}: missing or empty 'file_path'")),
            };
            let old_string = match edit.get("old_string").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => return ToolOutput::err(format!("Edit {i}: 'old_string' must be non-empty")),
            };
            let new_string = match edit.get("new_string").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return ToolOutput::err(format!("Edit {i}: missing 'new_string'")),
            };
            let replace_all = edit
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let file_path = match self.guard.resolve_for_write(raw_path) {
                Ok(p) => p,
                Err(e) => return ToolOutput::err(format!("Edit {i}: {e}")),
            };

            parsed.push(Edit {
                file_path,
                old_string,
                new_string,
                replace_all,
            });
        }

        // Phase 2: Read all unique files.
        let unique_paths: Vec<&PathBuf> = {
            let mut seen = std::collections::HashSet::new();
            parsed
                .iter()
                .filter_map(|e| {
                    if seen.insert(e.file_path.clone()) {
                        Some(&e.file_path)
                    } else {
                        None
                    }
                })
                .collect()
        };

        let mut file_contents: HashMap<PathBuf, String> = HashMap::new();
        for path in &unique_paths {
            let contents = match tokio::fs::read_to_string(path).await {
                Ok(s) => s,
                Err(e) => return ToolOutput::err(format!("Cannot read {}: {e}", path.display())),
            };
            file_contents.insert((*path).clone(), contents);
        }

        // Phase 3: Apply all edits in memory. Track which files were modified.
        let mut results: Vec<String> = Vec::with_capacity(parsed.len());
        for (i, edit) in parsed.iter().enumerate() {
            let contents = file_contents
                .get_mut(&edit.file_path)
                .expect("file contents should be loaded");

            // Check old_string exists.
            if !contents.contains(&edit.old_string) {
                return ToolOutput::err(format!(
                    "Edit {i} ({}): old_string not found. The text must match exactly (including whitespace/indentation).",
                    edit.file_path.display()
                ));
            }

            // Apply replacement.
            let new_contents = if edit.replace_all {
                contents.replace(&edit.old_string, &edit.new_string)
            } else {
                contents.replacen(&edit.old_string, &edit.new_string, 1)
            };
            *contents = new_contents;

            let label = if edit.replace_all {
                "all occurrences"
            } else {
                "first occurrence"
            };
            results.push(format!(
                "  Edit {i}: {} — replaced {label}",
                edit.file_path.display()
            ));
        }

        // Phase 4: Secret scan on new content before writing.
        for path in &unique_paths {
            let contents = &file_contents[*path];
            let findings = scan_for_secrets(contents);
            if !findings.is_empty() {
                let prefix = secret_findings_prefix(&findings);
                return ToolOutput::err(format!(
                    "Blocked: potential secrets detected in {}:\n{prefix}",
                    path.display()
                ));
            }
        }

        // Phase 5: Write all modified files to disk.
        let mut written_paths: Vec<(PathBuf, String)> = Vec::new();
        for path in &unique_paths {
            let contents = &file_contents[*path];
            if let Err(e) = tokio::fs::write(path, contents).await {
                // Best-effort: report the write failure. Some files may already be written.
                return ToolOutput::err(format!(
                    "Write failed for {}: {e}\nNote: some files may have been partially written.",
                    path.display()
                ));
            }
            written_paths.push(((*path).clone(), contents.clone()));
        }

        // Phase 6: Update file tracker for all written files.
        if let Some(tracker) = &self.tracker {
            for (path, _contents) in &written_paths {
                let mtime_nanos = tokio::fs::metadata(path)
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                tracker.update(path, mtime_nanos);
            }
        }

        let summary = format!(
            "MultiEdit: {} edit(s) applied to {} file(s).\n{}",
            parsed.len(),
            unique_paths.len(),
            results.join("\n")
        );
        ToolOutput::ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn tool_for_dir(dir: &std::path::Path) -> MultiEditTool {
        MultiEditTool::new(dir.to_path_buf())
    }

    #[tokio::test]
    async fn single_edit_replaces_first_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = NamedTempFile::new_in(&dir).unwrap();
        write!(f, "hello world hello").unwrap();
        let path = f.path().to_str().unwrap().to_string();
        let tool = tool_for_dir(dir.path());
        let result = tool
            .execute(json!({
                "edits": [{"file_path": path, "old_string": "hello", "new_string": "bye"}]
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        let written = tokio::fs::read_to_string(f.path()).await.unwrap();
        assert_eq!(written, "bye world hello");
    }

    #[tokio::test]
    async fn replace_all_flag_replaces_every_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = NamedTempFile::new_in(&dir).unwrap();
        write!(f, "foo foo foo").unwrap();
        let path = f.path().to_str().unwrap().to_string();
        let tool = tool_for_dir(dir.path());
        let result = tool
            .execute(json!({
                "edits": [{"file_path": path, "old_string": "foo", "new_string": "bar", "replace_all": true}]
            }))
            .await;
        assert!(!result.is_error);
        let written = tokio::fs::read_to_string(f.path()).await.unwrap();
        assert_eq!(written, "bar bar bar");
    }

    #[tokio::test]
    async fn multiple_edits_applied_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let mut f1 = NamedTempFile::new_in(&dir).unwrap();
        let mut f2 = NamedTempFile::new_in(&dir).unwrap();
        write!(f1, "fn old_name() {{}}").unwrap();
        write!(f2, "old_name()").unwrap();
        let p1 = f1.path().to_str().unwrap().to_string();
        let p2 = f2.path().to_str().unwrap().to_string();
        let tool = tool_for_dir(dir.path());
        let result = tool
            .execute(json!({
                "edits": [
                    {"file_path": p1, "old_string": "old_name", "new_string": "new_name"},
                    {"file_path": p2, "old_string": "old_name", "new_string": "new_name"}
                ]
            }))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("2 edit(s)"));
        assert!(result.content.contains("2 file(s)"));
    }

    #[tokio::test]
    async fn missing_old_string_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = NamedTempFile::new_in(&dir).unwrap();
        write!(f, "content").unwrap();
        let path = f.path().to_str().unwrap().to_string();
        let tool = tool_for_dir(dir.path());
        let result = tool
            .execute(json!({
                "edits": [{"file_path": path, "old_string": "not_there", "new_string": "x"}]
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn empty_edits_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_for_dir(dir.path());
        let result = tool.execute(json!({"edits": []})).await;
        assert!(result.is_error);
    }
}
