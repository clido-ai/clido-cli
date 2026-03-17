//! Glob tool: list files matching pattern.

use async_trait::async_trait;
use glob::Pattern;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

use crate::path_guard;
use crate::{Tool, ToolOutput};

pub struct GlobTool {
    pub workspace_root: PathBuf,
}

impl GlobTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "List files matching a glob pattern. Pattern and optional path (default cwd)."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern (e.g. **/*.rs)" },
                "path": { "type": "string", "description": "Directory to search (default: cwd)" }
            },
            "required": ["pattern"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let path_str = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        if pattern.is_empty() {
            return ToolOutput::err("Missing required field: pattern".to_string());
        }

        let base = if path_str == "." || path_str.is_empty() {
            self.workspace_root.clone()
        } else {
            match path_guard::resolve_and_check(path_str, &self.workspace_root) {
                Ok(p) => p,
                Err(e) => return ToolOutput::err(e),
            }
        };

        if !base.is_dir() {
            return ToolOutput::err(format!("Path is not a directory: {}", base.display()));
        }

        let pattern = match Pattern::new(pattern) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(e.to_string()),
        };

        let mut entries: Vec<PathBuf> = Vec::new();
        for result in WalkBuilder::new(&base).build() {
            match result {
                Ok(entry) => {
                    let path = entry.path();
                    if path.is_file() {
                        entries.push(path.to_path_buf());
                    }
                }
                Err(e) => return ToolOutput::err(e.to_string()),
            }
        }

        let mut matched: Vec<String> = entries
            .into_iter()
            .filter_map(|p| {
                let rel = p.strip_prefix(&base).ok()?;
                let s = rel.to_string_lossy();
                if pattern.matches_path(Path::new(&*s)) {
                    Some(s.into_owned())
                } else {
                    None
                }
            })
            .collect();
        matched.sort();

        ToolOutput::ok(matched.join("\n"))
    }
}
