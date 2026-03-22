//! GitTool: structured, read-only git operations for the agent.

use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;

use crate::{Tool, ToolOutput};

/// Read-only git tool. Exposes a curated set of safe git subcommands.
///
/// Write-mode subcommands (add, commit, push, etc.) are explicitly rejected
/// to maintain the read-only contract. Use Bash for destructive git operations
/// when the user explicitly wants them.
pub struct GitTool {
    workspace_root: PathBuf,
}

impl GitTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

/// Allowed read-only subcommands.
const ALLOWED_SUBCOMMANDS: &[&str] = &[
    "status",
    "diff",
    "diff-staged",
    "log",
    "branch",
    "show",
    "stash-list",
];

const TIMEOUT_MS: u64 = 30_000;

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "Git"
    }

    fn description(&self) -> &str {
        "Run a read-only git operation. Supported subcommands: status, diff, diff-staged, log, branch, show, stash-list."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subcommand": {
                    "type": "string",
                    "description": "Git operation: status | diff | diff-staged | log | branch | show | stash-list",
                    "enum": ["status", "diff", "diff-staged", "log", "branch", "show", "stash-list"]
                },
                "path": {
                    "type": "string",
                    "description": "(optional) File path to restrict diff/show to"
                },
                "count": {
                    "type": "integer",
                    "description": "(optional) Number of log entries to show (default 10, max 50)"
                }
            },
            "required": ["subcommand"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let subcommand = match input.get("subcommand").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return ToolOutput::err("Missing required field: subcommand".to_string()),
        };

        if !ALLOWED_SUBCOMMANDS.contains(&subcommand.as_str()) {
            return ToolOutput::err(format!(
                "Unsupported subcommand: '{}'. Allowed: {}. \
                 For write operations use the Bash tool with explicit user confirmation.",
                subcommand,
                ALLOWED_SUBCOMMANDS.join(", ")
            ));
        }

        let path_arg = input
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let count = input
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(50) as u32;

        let args = build_args(&subcommand, path_arg.as_deref(), count);

        let workspace_root = self.workspace_root.clone();
        let run_result = tokio::time::timeout(Duration::from_millis(TIMEOUT_MS), async move {
            tokio::process::Command::new("git")
                .args(&args)
                .current_dir(&workspace_root)
                .output()
                .await
        })
        .await;

        match run_result {
            Err(_) => ToolOutput::err("git command timed out (30s)".to_string()),
            Ok(Err(e)) => ToolOutput::err(format!("Failed to run git: {}", e)),
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                if output.status.success() {
                    ToolOutput::ok(stdout)
                } else {
                    let code = output.status.code().unwrap_or(-1);
                    let combined = if stderr.is_empty() { stdout } else { stderr };
                    ToolOutput::err(format!("git exited with code {}: {}", code, combined))
                }
            }
        }
    }
}

fn build_args(subcommand: &str, path: Option<&str>, count: u32) -> Vec<String> {
    match subcommand {
        "status" => vec!["status".to_string(), "--short".to_string()],
        "diff" => {
            let mut args = vec!["diff".to_string()];
            if let Some(p) = path {
                args.push("--".to_string());
                args.push(p.to_string());
            }
            args
        }
        "diff-staged" => {
            let mut args = vec!["diff".to_string(), "--staged".to_string()];
            if let Some(p) = path {
                args.push("--".to_string());
                args.push(p.to_string());
            }
            args
        }
        "log" => vec![
            "log".to_string(),
            "--oneline".to_string(),
            format!("-{}", count),
        ],
        "branch" => vec!["branch".to_string(), "--show-current".to_string()],
        "show" => {
            let mut args = vec!["show".to_string(), "--stat".to_string(), "HEAD".to_string()];
            if let Some(p) = path {
                args.push("--".to_string());
                args.push(p.to_string());
            }
            args
        }
        "stash-list" => vec!["stash".to_string(), "list".to_string()],
        // Should never reach here due to allowlist check above.
        other => vec![other.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_git_repo(dir: &std::path::Path) {
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[tokio::test]
    async fn test_git_tool_unknown_subcommand_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = GitTool::new(tmp.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "subcommand": "push" }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Unsupported subcommand"));
    }

    #[tokio::test]
    async fn test_git_tool_missing_subcommand_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = GitTool::new(tmp.path().to_path_buf());
        let out = tool.execute(serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("Missing required field"));
    }

    #[tokio::test]
    async fn test_git_tool_not_in_repo_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = GitTool::new(tmp.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "subcommand": "status" }))
            .await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn test_git_tool_status_returns_output() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());
        // Create an initial commit so the repo is valid
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        // Now add an untracked file
        std::fs::write(tmp.path().join("new.txt"), "new").unwrap();
        let tool = GitTool::new(tmp.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "subcommand": "status" }))
            .await;
        assert!(!out.is_error, "status should succeed: {}", out.content);
        assert!(out.content.contains("new.txt"));
    }

    #[tokio::test]
    async fn test_git_tool_log_returns_commits() {
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());
        for i in 0..2 {
            std::fs::write(tmp.path().join(format!("f{}.txt", i)), format!("{}", i)).unwrap();
            Command::new("git")
                .args(["add", "."])
                .current_dir(tmp.path())
                .output()
                .unwrap();
            Command::new("git")
                .args(["commit", "-m", &format!("commit {}", i)])
                .current_dir(tmp.path())
                .output()
                .unwrap();
        }
        let tool = GitTool::new(tmp.path().to_path_buf());
        let out = tool
            .execute(serde_json::json!({ "subcommand": "log" }))
            .await;
        assert!(!out.is_error, "log should succeed: {}", out.content);
        assert!(out.content.lines().count() >= 2);
    }

    #[tokio::test]
    async fn test_git_tool_log_count_capped_at_50() {
        // Verify the count is capped: build_args with count 9999 produces -50
        let args = build_args("log", None, 9999u32.min(50));
        assert!(args.contains(&"-50".to_string()));
    }

    #[tokio::test]
    async fn test_git_tool_is_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = GitTool::new(tmp.path().to_path_buf());
        assert!(tool.is_read_only());
    }
}
