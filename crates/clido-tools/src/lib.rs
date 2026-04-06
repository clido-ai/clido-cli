//! Tools: trait, registry, and implementations (Bash, Read, Write, Glob, Grep, etc.).

mod apply_patch_tool;
mod bash;
mod diagnostics;
mod edit;
mod exit_plan_mode;
pub mod file_tracker;
mod git_tool;
mod glob_tool;
mod grep_tool;
mod ls_tool;
pub mod mcp;
mod multi_edit;
mod path_guard;
mod read;
mod registry;
pub mod secrets;
mod semantic_search;
mod test_loop;
pub mod test_runner;
mod todo_write;
pub mod truncate;
pub mod web_fetch;
pub mod web_search;
mod write;

use std::path::PathBuf;

use async_trait::async_trait;

pub use apply_patch_tool::ApplyPatchTool;
pub use bash::BashTool;
pub use diagnostics::DiagnosticsTool;
pub use edit::EditTool;
pub use exit_plan_mode::ExitPlanModeTool;
pub use file_tracker::FileTracker;
pub use git_tool::GitTool;
pub use glob_tool::GlobTool;
pub use grep_tool::GrepTool;
pub use ls_tool::LsTool;
pub use mcp::{
    load_mcp_config, McpClient, McpConfig, McpHttpClient, McpServerConfig, McpTool, McpToolDef,
    McpTransport, McpTransportClient,
};
pub use multi_edit::MultiEditTool;
pub use path_guard::{PathGuard, ACCESS_DENIED_OUTSIDE_WORKSPACE};
pub use read::ReadTool;
pub use registry::ToolRegistry;
pub use semantic_search::SemanticSearchTool;
pub use test_loop::TestLoopTool;
pub use todo_write::{TodoItem, TodoPriority, TodoStatus, TodoWriteTool};
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use write::WriteTool;

/// Build default V1 tool registry with workspace root (e.g. env::current_dir()).
pub fn default_registry(workspace_root: PathBuf) -> ToolRegistry {
    default_registry_with_blocked(workspace_root, Vec::new())
}

/// Build registry with allowed external paths (outside workspace_root).
pub fn default_registry_with_allowed_paths(
    workspace_root: PathBuf,
    allowed_external: Vec<PathBuf>,
) -> ToolRegistry {
    default_registry_with_options_and_allowed_paths(
        workspace_root,
        Vec::new(),
        false,
        allowed_external,
    )
    .0
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
    default_registry_with_todo_store(workspace_root, blocked, sandbox).0
}

/// Build registry with allowed external paths.
pub fn default_registry_with_options_and_allowed_paths(
    workspace_root: PathBuf,
    blocked: Vec<PathBuf>,
    sandbox: bool,
    allowed_external: Vec<PathBuf>,
) -> (
    ToolRegistry,
    std::sync::Arc<std::sync::Mutex<Vec<TodoItem>>>,
) {
    let guard = PathGuard::new(workspace_root.clone())
        .with_blocked(blocked.clone())
        .with_allowed_external(allowed_external);
    let tracker = FileTracker::new();
    let read_cache = clido_context::read_cache::ReadCache::new();
    let mut r = ToolRegistry::new();
    r.register(ExitPlanModeTool);
    if sandbox {
        r.register(BashTool::new_sandboxed(blocked).with_workspace(workspace_root.clone()));
    } else {
        r.register(BashTool::new_with_blocked(blocked).with_workspace(workspace_root.clone()));
    }
    r.register(ReadTool::new_with_cache(
        guard.clone(),
        tracker.clone(),
        read_cache,
    ));
    r.register(WriteTool::new_with_tracker(guard.clone(), tracker.clone()));
    r.register(EditTool::new_with_tracker(guard.clone(), tracker.clone()));
    r.register(MultiEditTool::new_with_tracker(
        guard.clone(),
        tracker.clone(),
    ));
    let todo_tool = TodoWriteTool::new();
    let todo_store = todo_tool.store();
    r.register(todo_tool);
    r.register(GlobTool::new_with_guard(guard.clone()));
    r.register(LsTool::new_with_guard(guard.clone()));
    r.register(ApplyPatchTool::new(guard.clone()));
    r.register(GrepTool::new_with_guard(guard));
    r.register(GitTool::new(workspace_root.clone()));
    r.register(SemanticSearchTool::new(workspace_root.clone()));
    r.register(WebFetchTool::new());
    r.register(WebSearchTool::new());
    r.register(DiagnosticsTool::new());
    r.register(TestLoopTool::new(workspace_root));
    r.register(truncate::TruncateTool::new());
    (r, todo_store)
}

/// Build registry and return both the registry and the agent's shared todo store.
/// The todo store can be read by the TUI to display agent task progress.
pub fn default_registry_with_todo_store(
    workspace_root: PathBuf,
    blocked: Vec<PathBuf>,
    sandbox: bool,
) -> (
    ToolRegistry,
    std::sync::Arc<std::sync::Mutex<Vec<TodoItem>>>,
) {
    let guard = PathGuard::new(workspace_root.clone()).with_blocked(blocked.clone());
    let tracker = FileTracker::new();
    let read_cache = clido_context::read_cache::ReadCache::new();
    let mut r = ToolRegistry::new();
    r.register(ExitPlanModeTool);
    if sandbox {
        r.register(BashTool::new_sandboxed(blocked).with_workspace(workspace_root.clone()));
    } else {
        r.register(BashTool::new_with_blocked(blocked).with_workspace(workspace_root.clone()));
    }
    r.register(ReadTool::new_with_cache(
        guard.clone(),
        tracker.clone(),
        read_cache,
    ));
    r.register(WriteTool::new_with_tracker(guard.clone(), tracker.clone()));
    r.register(EditTool::new_with_tracker(guard.clone(), tracker.clone()));
    r.register(MultiEditTool::new_with_tracker(
        guard.clone(),
        tracker.clone(),
    ));
    let todo_tool = TodoWriteTool::new();
    let todo_store = todo_tool.store();
    r.register(todo_tool);
    r.register(GlobTool::new_with_guard(guard.clone()));
    r.register(LsTool::new_with_guard(guard.clone()));
    r.register(ApplyPatchTool::new(guard.clone()));
    r.register(GrepTool::new_with_guard(guard));
    r.register(GitTool::new(workspace_root.clone()));
    r.register(SemanticSearchTool::new(workspace_root.clone()));
    r.register(WebFetchTool::new());
    r.register(WebSearchTool::new());
    r.register(DiagnosticsTool::new());
    r.register(TestLoopTool::new(workspace_root));
    r.register(truncate::TruncateTool::new());
    (r, todo_store)
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
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            path: None,
            content_hash: None,
            mtime_nanos: None,
            diff: None,
        }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            path: None,
            content_hash: None,
            mtime_nanos: None,
            diff: None,
        }
    }
    pub fn ok_with_meta(
        content: impl Into<String>,
        path: impl Into<String>,
        content_hash: impl Into<String>,
        mtime_nanos: u64,
    ) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            path: Some(path.into()),
            content_hash: Some(content_hash.into()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// A tool that uses the default `is_read_only` (returns false).
    struct DefaultReadOnlyTool;

    #[async_trait]
    impl Tool for DefaultReadOnlyTool {
        fn name(&self) -> &str {
            "DefaultReadOnly"
        }
        fn description(&self) -> &str {
            "test"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        // does NOT override is_read_only — uses the default (false)
        async fn execute(&self, _input: serde_json::Value) -> ToolOutput {
            ToolOutput::ok("ok".to_string())
        }
    }

    /// Lines 151-152: default is_read_only returns false.
    #[test]
    fn default_is_read_only_returns_false() {
        let tool = DefaultReadOnlyTool;
        assert!(!tool.is_read_only());
    }

    #[test]
    fn tool_output_ok() {
        let out = ToolOutput::ok("hello");
        assert!(!out.is_error);
        assert_eq!(out.content, "hello");
    }

    #[test]
    fn tool_output_err() {
        let out = ToolOutput::err("bad");
        assert!(out.is_error);
    }

    #[test]
    fn tool_output_ok_with_meta() {
        let out = ToolOutput::ok_with_meta("done", "/a/b", "abc123", 42);
        assert!(!out.is_error);
        assert_eq!(out.path, Some("/a/b".to_string()));
        assert_eq!(out.content_hash, Some("abc123".to_string()));
        assert_eq!(out.mtime_nanos, Some(42));
    }

    #[test]
    fn default_registry_builds_without_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = default_registry(tmp.path().to_path_buf());
        let schemas = reg.schemas();
        assert!(!schemas.is_empty());
    }

    /// Line 70: sandbox branch in default_registry_with_options.
    #[test]
    fn default_registry_with_sandbox_builds_without_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = default_registry_with_options(tmp.path().to_path_buf(), vec![], true);
        let schemas = reg.schemas();
        assert!(!schemas.is_empty());
    }
}
