//! Read tool: read file contents with optional offset/limit.

use async_trait::async_trait;
use std::path::PathBuf;

use crate::file_tracker::FileTracker;
use crate::path_guard::PathGuard;
use crate::{Tool, ToolOutput};

pub struct ReadTool {
    guard: PathGuard,
    tracker: Option<FileTracker>,
    read_cache: Option<clido_context::read_cache::ReadCache>,
}

impl ReadTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            guard: PathGuard::new(workspace_root),
            tracker: None,
            read_cache: None,
        }
    }
    pub fn new_with_guard(guard: PathGuard) -> Self {
        Self { guard, tracker: None, read_cache: None }
    }
    pub fn new_with_tracker(guard: PathGuard, tracker: FileTracker) -> Self {
        Self { guard, tracker: Some(tracker), read_cache: None }
    }
    pub fn new_with_cache(
        guard: PathGuard,
        tracker: FileTracker,
        read_cache: clido_context::read_cache::ReadCache,
    ) -> Self {
        Self { guard, tracker: Some(tracker), read_cache: Some(read_cache) }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "Read file contents. Optionally specify offset (1-based line) and limit (number of lines)."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to file (relative to cwd)" },
                "path": { "type": "string", "description": "Alias for file_path" },
                "offset": { "type": "integer", "description": "1-based line number to start" },
                "limit": { "type": "integer", "description": "Number of lines to return" }
            },
            "required": []
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let path_str = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if path_str.is_empty() {
            return ToolOutput::err("Missing required field: file_path or path".to_string());
        }

        let path = match self.guard.resolve_and_check(path_str) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };

        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    return ToolOutput::err(format!(
                        "File does not exist. Note: your current working directory is {}.",
                        cwd.display()
                    ));
                }
                return ToolOutput::err(e.to_string());
            }
        };

        if meta.is_dir() {
            return ToolOutput::err(format!(
                "EISDIR: illegal operation on a directory, read '{}'",
                path.display()
            ));
        }

        // Compute a lightweight hash of mtime+size for cache keying.
        let cache_hash = {
            use std::time::UNIX_EPOCH;
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{}-{}", mtime_ns, meta.len())
        };

        // Check the in-memory read cache before hitting disk.
        if let Some(ref cache) = self.read_cache {
            if let Some(cached) = cache.get(&path, &cache_hash) {
                let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if offset == 0 && limit == 0 {
                    // Full-file read: return cached directly.
                    if let Some(ref tracker) = self.tracker {
                        let mtime = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_nanos() as u64)
                            .unwrap_or(0);
                        tracker.record(&path, mtime);
                    }
                    // Re-format with line numbers.
                    let lines: Vec<&str> = cached.lines().collect();
                    let out: String = lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| format!("{:>6}\u{2192}{}", i + 1, line))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return ToolOutput::ok(out);
                }
            }
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(e.to_string()),
        };

        // Populate cache for full-file reads.
        if let Some(ref cache) = self.read_cache {
            cache.insert(path.clone(), cache_hash, content.clone());
        }

        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let (start, end) = if offset >= 1 {
            let start = (offset - 1).min(total);
            let end = if limit > 0 {
                (start + limit).min(total)
            } else {
                total
            };
            (start, end)
        } else {
            (0, total)
        };

        let selected: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}→{}", start + i + 1, line))
            .collect();
        let out = selected.join("\n");
        if out.is_empty() && !content.is_empty() && (offset > total || (offset >= 1 && limit == 0))
        {
            return ToolOutput::err(format!(
                "File has {} lines; offset {} is out of range.",
                total, offset
            ));
        }

        // Record mtime so Edit/Write can detect external modifications later.
        if let Some(ref tracker) = self.tracker {
            let mtime = tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            tracker.record(&path, mtime);
        }

        ToolOutput::ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_missing_file() {
        let root = std::env::temp_dir();
        let t = ReadTool::new(root);
        let out = t
            .execute(serde_json::json!({ "file_path": "nonexistent_xyz_123" }))
            .await;
        assert!(out.is_error);
        assert!(!out.content.is_empty());
    }

    #[tokio::test]
    async fn read_offset_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "a\nb\nc\nd\ne\n").unwrap();
        let t = ReadTool::new(dir.path().to_path_buf());
        let out = t
            .execute(serde_json::json!({ "path": "f.txt", "offset": 2, "limit": 2 }))
            .await;
        assert!(!out.is_error);
        assert!(out.content.contains("2→b"));
        assert!(out.content.contains("3→c"));
    }

    #[tokio::test]
    async fn read_offset_only_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
        let t = ReadTool::new(dir.path().to_path_buf());
        let out = t
            .execute(serde_json::json!({ "path": "f.txt", "offset": 2 }))
            .await;
        assert!(!out.is_error);
        assert!(out.content.contains("line2"));
        assert!(out.content.contains("line3"));
    }
}
