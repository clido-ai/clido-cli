//! SemanticSearch tool: query repo index + memory store for relevant content.

use async_trait::async_trait;
use serde_json::Value;

use crate::{Tool, ToolOutput};

/// SemanticSearch tool for the agent: queries the repo index and memory store.
pub struct SemanticSearchTool {
    workspace_root: std::path::PathBuf,
}

impl SemanticSearchTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait]
impl Tool for SemanticSearchTool {
    fn name(&self) -> &str {
        "SemanticSearch"
    }

    fn description(&self) -> &str {
        "Search the repository index and long-term memory for content relevant to a query. \
         Returns matching files, symbols, and memories. Use for navigating large codebases \
         or recalling past context."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query string."
                },
                "target_directory": {
                    "type": "string",
                    "description": "Optional subdirectory to limit file/symbol search."
                },
                "num_results": {
                    "type": "integer",
                    "description": "Maximum number of results per source (default: 5)."
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolOutput {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.to_string(),
            None => return ToolOutput::err("Missing required field: query".to_string()),
        };
        let num_results = input
            .get("num_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;
        let target_dir = input
            .get("target_directory")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut output_parts = Vec::new();

        // Search repo index
        let _root = if let Some(ref sub) = target_dir {
            self.workspace_root.join(sub)
        } else {
            self.workspace_root.clone()
        };

        // Try opening index DB
        let index_db = self.workspace_root.join(".clido").join("index.db");
        if index_db.exists() {
            match clido_index::RepoIndex::open(&index_db) {
                Ok(idx) => {
                    // Search symbols
                    if let Ok(syms) = idx.search_symbols(&query) {
                        if !syms.is_empty() {
                            let mut part = format!("## Symbols matching '{}'\n", query);
                            for s in syms.iter().take(num_results) {
                                part.push_str(&format!(
                                    "  {} {} in {} (line {})\n",
                                    s.kind, s.name, s.path, s.line
                                ));
                            }
                            output_parts.push(part);
                        }
                    }
                    // Search files
                    if let Ok(files) = idx.search_files(&query) {
                        if !files.is_empty() {
                            let mut part = format!("## Files matching '{}'\n", query);
                            for f in files.iter().take(num_results) {
                                part.push_str(&format!("  {} ({} bytes)\n", f.path, f.size_bytes));
                            }
                            output_parts.push(part);
                        }
                    }
                }
                Err(e) => {
                    output_parts.push(format!("(Index unavailable: {})\n", e));
                }
            }
        } else {
            output_parts.push(format!(
                "(No repo index found at {}. Run `clido index build` to create one.)\n",
                index_db.display()
            ));
        }

        // Search memory store
        if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") as Option<directories::ProjectDirs> {
            let memory_db = dirs.data_dir().join("memory.db");
            if memory_db.exists() {
                if let Ok(store) = clido_memory::MemoryStore::open(&memory_db) {
                    if let Ok(memories) = store.search_keyword(&query, num_results) {
                        if !memories.is_empty() {
                            let mut part = format!("## Memories matching '{}'\n", query);
                            for m in &memories {
                                let tags = if m.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", m.tags.join(", "))
                                };
                                part.push_str(&format!("  [{}]{} {}\n", m.created_at, tags, m.content));
                            }
                            output_parts.push(part);
                        }
                    }
                }
            }
        }

        if output_parts.is_empty() {
            ToolOutput::ok(format!("No results found for query: '{}'", query))
        } else {
            ToolOutput::ok(output_parts.join("\n"))
        }
    }
}
