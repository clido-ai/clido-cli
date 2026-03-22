//! Tools: trait, registry, and implementations (Bash, Read, Write, Glob, Grep, etc.).

mod bash;
mod diagnostics;
mod edit;
mod exit_plan_mode;
pub mod file_tracker;
mod git_tool;
mod glob_tool;
mod grep_tool;
pub mod mcp;
mod path_guard;
mod read;
mod registry;
pub mod secrets;
mod semantic_search;
mod test_loop;
pub mod test_runner;
pub mod web_fetch;
pub mod web_search;
mod write;

use std::path::PathBuf;

use async_trait::async_trait;

pub use bash::BashTool;
pub use diagnostics::DiagnosticsTool;
pub use edit::EditTool;
pub use exit_plan_mode::ExitPlanModeTool;
pub use file_tracker::FileTracker;
pub use git_tool::GitTool;
pub use glob_tool::GlobTool;
pub use grep_tool::GrepTool;
pub use mcp::{load_mcp_config, McpClient, McpConfig, McpServerConfig, McpTool, McpToolDef};
pub use path_guard::PathGuard;
pub use read::ReadTool;
pub use registry::ToolRegistry;
pub use semantic_search::SemanticSearchTool;
pub use test_loop::TestLoopTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use write::WriteTool;

/// Build default V1 tool registry with workspace root (e.g. env::current_dir()).
pub fn default_registry(workspace_root: PathBuf) -> ToolRegistry {
    default_registry_with_blocked(workspace_root, Vec::new())
}

/// Build registry with blocked paths excluded from all file tools and Bash.
pub fn default_registry_with_blocked(
    workspace_root: PathBuf,
    blocked: Vec<PathBuf>,
) -> ToolRegistry {
    default_registry_with_options(workspace_root, blocked, false)
}

/// Build registry with optional Bash sandboxing.
pub fn default_registry_with_options(
    workspace_root: PathBuf,
    blocked: Vec<PathBuf>,
    sandbox: bool,
) -> ToolRegistry {
    let guard = PathGuard::new(workspace_root.clone()).with_blocked(blocked.clone());
    let tracker = FileTracker::new();
    let read_cache = clido_context::read_cache::ReadCache::new();
    let mut r = ToolRegistry::new();
    r.register(ExitPlanModeTool);
    if sandbox {
        r.register(BashTool::new_sandboxed(blocked));
    } else {
        r.register(BashTool::new_with_blocked(blocked));
    }
    r.register(ReadTool::new_with_cache(
        guard.clone(),
        tracker.clone(),
        read_cache,
    ));
    r.register(WriteTool::new_with_tracker(guard.clone(), tracker.clone()));
    r.register(EditTool::new_with_tracker(guard.clone(), tracker.clone()));
    r.register(GlobTool::new_with_guard(guard.clone()));
    r.register(GrepTool::new_with_guard(guard));
    r.register(GitTool::new(workspace_root.clone()));
    r.register(SemanticSearchTool::new(workspace_root.clone()));
    r.register(WebFetchTool::new());
    r.register(WebSearchTool::new());
    r.register(DiagnosticsTool::new());
    r.register(TestLoopTool::new(workspace_root));
    r
}

/// Output of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// For Write/Edit: path written (for session stale-file detection).
    pub path: Option<String>,
    /// For Write/Edit: content hash after write (for session stale-file detection).
    pub content_hash: Option<String>,
    /// For Write/Edit: mtime in nanos (for session stale-file detection).
    pub mtime_nanos: Option<u64>,
    /// For Edit: unified diff of the change (for TUI display).
    pub diff: Option<String>,
}

impl ToolOutput {
    pub fn ok(content: String) -> Self {
        Self {
            content,
            is_error: false,
            path: None,
            content_hash: None,
            mtime_nanos: None,
            diff: None,
        }
    }
    pub fn err(content: String) -> Self {
        Self {
            content,
            is_error: true,
            path: None,
            content_hash: None,
            mtime_nanos: None,
            diff: None,
        }
    }
    pub fn ok_with_meta(
        content: String,
        path: String,
        content_hash: String,
        mtime_nanos: u64,
    ) -> Self {
        Self {
            content,
            is_error: false,
            path: Some(path),
            content_hash: Some(content_hash),
            mtime_nanos: Some(mtime_nanos),
            diff: None,
        }
    }
}

/// Tool interface: name, schema, execute.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> serde_json::Value;
    fn is_read_only(&self) -> bool {
        false
    }
    async fn execute(&self, input: serde_json::Value) -> ToolOutput;
}
