//! Write tool: create or overwrite file.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::path_guard::PathGuard;
use crate::{Tool, ToolOutput};

pub struct WriteTool {
    guard: PathGuard,
}

impl WriteTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            guard: PathGuard::new(workspace_root),
        }
    }
    pub fn new_with_guard(guard: PathGuard) -> Self {
        Self { guard }
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
                "file_path": { "type": "string" },
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["content"]
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

        let path = match self.guard.resolve_for_write(path_str) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };

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
