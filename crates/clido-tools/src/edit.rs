//! Edit tool: replace old_string with new_string in file.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::path_guard;
use crate::{Tool, ToolOutput};

pub struct EditTool {
    pub workspace_root: PathBuf,
}

impl EditTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
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
                "file_path": { "type": "string" },
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" },
                "replace_all": { "type": "boolean", "default": false }
            },
            "required": ["old_string"]
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

        let path = match path_guard::resolve_and_check(path_str, &self.workspace_root) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e),
        };

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(e.to_string()),
        };

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
        ToolOutput::ok_with_meta(
            format!("The file {} has been updated successfully.", path.display()),
            path.display().to_string(),
            hash,
            mtime_nanos,
        )
    }
}
