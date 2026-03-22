//! SemanticSearch tool: auto-builds/refreshes the repo index, then queries it.
//!
//! The index is stored at `<workspace>/.clido/index.db`. On first use it is built
//! automatically. If it is older than INDEX_MAX_AGE_SECS it is rebuilt in-place
//! before querying so results are always fresh without any manual step.

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use serde_json::Value;

use crate::{Tool, ToolOutput};

/// Rebuild the index if it is older than 1 hour.
const INDEX_MAX_AGE_SECS: u64 = 3600;

/// File extensions indexed by default.
/// Web3/smart-contract languages are listed first so they get priority in results.
const DEFAULT_EXTENSIONS: &[&str] = &[
    // Web3 / smart contracts
    "sol",   // Solidity (Ethereum, EVM)
    "move",  // Move (Aptos, Sui)
    "vy",    // Vyper
    "fe",    // Fe (Ethereum)
    "yul",   // Yul / Yul+ (EVM assembly IR)
    "rell",  // Rell (Chromia)
    "cairo", // Cairo (StarkNet)
    // General-purpose
    "rs", "py", "js", "ts", "go", "java", "c", "cpp", "h", "md",
];

pub struct SemanticSearchTool {
    workspace_root: std::path::PathBuf,
}

impl SemanticSearchTool {
    pub fn new(workspace_root: std::path::PathBuf) -> Self {
        Self { workspace_root }
    }

    /// Return the index DB path, creating the .clido dir if needed.
    fn index_path(&self) -> std::path::PathBuf {
        self.workspace_root.join(".clido").join("index.db")
    }

    /// Age of the index in seconds. Returns None if no index exists yet.
    fn index_age_secs(index_path: &std::path::Path) -> Option<u64> {
        let meta = std::fs::metadata(index_path).ok()?;
        let modified = meta.modified().ok()?;
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO);
        Some(age.as_secs())
    }

    /// Build (or rebuild) the index, returning a human-readable status note.
    fn ensure_index(&self) -> String {
        let db_path = self.index_path();
        let age = Self::index_age_secs(&db_path);

        let needs_build = match age {
            None => true,                      // doesn't exist yet
            Some(s) => s > INDEX_MAX_AGE_SECS, // stale
        };

        if !needs_build {
            return String::new();
        }

        // Create .clido dir if needed.
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let label = if age.is_none() {
            "Building"
        } else {
            "Refreshing"
        };

        match clido_index::RepoIndex::open(&db_path) {
            Err(e) => format!("(Index unavailable: {})\n", e),
            Ok(mut idx) => match idx.build(&self.workspace_root, DEFAULT_EXTENSIONS) {
                Ok(n) => {
                    let (_, sym_count) = idx.stats().unwrap_or((0, 0));
                    format!(
                        "({} repo index: {} files, {} symbols)\n",
                        label, n, sym_count
                    )
                }
                Err(e) => format!("(Index build failed: {})\n", e),
            },
        }
    }
}

#[async_trait]
impl Tool for SemanticSearchTool {
    fn name(&self) -> &str {
        "SemanticSearch"
    }

    fn description(&self) -> &str {
        "Search the repository for files, symbols, and long-term memories relevant to a query. \
         The index is built and kept fresh automatically — no manual setup needed. \
         Use for navigating large codebases, finding where a function is defined, or \
         recalling past context."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query (function name, concept, file pattern, etc.)"
                },
                "target_directory": {
                    "type": "string",
                    "description": "Limit search to this subdirectory (optional)."
                },
                "num_results": {
                    "type": "integer",
                    "description": "Max results per source (default: 5, max: 20)."
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
            Some(q) if !q.trim().is_empty() => q.to_string(),
            _ => return ToolOutput::err("Missing required field: query".to_string()),
        };
        let num_results = input
            .get("num_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(20) as usize;

        // Auto-build/refresh index (no-op when fresh, fast when up-to-date).
        let index_note = self.ensure_index();

        let mut output_parts = Vec::new();
        if !index_note.is_empty() {
            output_parts.push(index_note);
        }

        // ── Repo index search ──────────────────────────────────────────────
        let index_db = self.index_path();
        if index_db.exists() {
            match clido_index::RepoIndex::open(&index_db) {
                Ok(idx) => {
                    // Symbols
                    if let Ok(syms) = idx.search_symbols(&query) {
                        if !syms.is_empty() {
                            let mut part = format!("## Symbols matching '{}'\n", query);
                            for s in syms.iter().take(num_results) {
                                part.push_str(&format!(
                                    "  {} {} — {} line {}\n",
                                    s.kind, s.name, s.path, s.line
                                ));
                            }
                            output_parts.push(part);
                        }
                    }
                    // Files
                    if let Ok(files) = idx.search_files(&query) {
                        let files: Vec<_> =
                            match input.get("target_directory").and_then(|v| v.as_str()) {
                                Some(dir) => {
                                    files.into_iter().filter(|f| f.path.contains(dir)).collect()
                                }
                                None => files,
                            };
                        if !files.is_empty() {
                            let mut part = format!("## Files matching '{}'\n", query);
                            for f in files.iter().take(num_results) {
                                part.push_str(&format!("  {}\n", f.path));
                            }
                            output_parts.push(part);
                        }
                    }
                }
                Err(e) => {
                    output_parts.push(format!("(Index error: {})\n", e));
                }
            }
        }

        // ── Memory search ──────────────────────────────────────────────────
        if let Some(dirs) = directories::ProjectDirs::from("", "", "clido") {
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
                                part.push_str(&format!(
                                    "  {}{} {}\n",
                                    m.created_at, tags, m.content
                                ));
                            }
                            output_parts.push(part);
                        }
                    }
                }
            }
        }

        if output_parts.is_empty() {
            ToolOutput::ok(format!("No results found for '{}'.", query))
        } else {
            ToolOutput::ok(output_parts.join("\n"))
        }
    }
}
