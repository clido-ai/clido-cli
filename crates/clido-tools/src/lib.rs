//! Tools: trait, registry, and implementations (Bash, Read, Write, Glob, Grep, etc.).

mod bash;
mod edit;
mod exit_plan_mode;
mod glob_tool;
mod grep_tool;
mod path_guard;
mod read;
mod registry;
mod write;

use std::path::PathBuf;

use async_trait::async_trait;

pub use bash::BashTool;
pub use edit::EditTool;
pub use exit_plan_mode::ExitPlanModeTool;
pub use glob_tool::GlobTool;
pub use grep_tool::GrepTool;
pub use path_guard::PathGuard;
pub use read::ReadTool;
pub use registry::ToolRegistry;
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
    let guard = PathGuard::new(workspace_root).with_blocked(blocked.clone());
    let mut r = ToolRegistry::new();
    r.register(ExitPlanModeTool);
    r.register(BashTool::new_with_blocked(blocked));
    r.register(ReadTool::new_with_guard(guard.clone()));
    r.register(WriteTool::new_with_guard(guard.clone()));
    r.register(EditTool::new_with_guard(guard.clone()));
    r.register(GlobTool::new_with_guard(guard.clone()));
    r.register(GrepTool::new_with_guard(guard));
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
}

impl ToolOutput {
    pub fn ok(content: String) -> Self {
        Self {
            content,
            is_error: false,
            path: None,
            content_hash: None,
            mtime_nanos: None,
        }
    }
    pub fn err(content: String) -> Self {
        Self {
            content,
            is_error: true,
            path: None,
            content_hash: None,
            mtime_nanos: None,
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
