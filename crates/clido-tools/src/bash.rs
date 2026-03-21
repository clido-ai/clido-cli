//! Bash tool: execute shell commands.

use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;

use crate::{Tool, ToolOutput};

/// Execute shell commands via sh -c.
#[derive(Default)]
pub struct BashTool {
    blocked: Vec<PathBuf>,
    /// When true, wrap command in a sandbox (sandbox-exec on macOS, bwrap on Linux).
    sandbox: bool,
}

impl BashTool {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn new_with_blocked(blocked: Vec<PathBuf>) -> Self {
        Self { blocked, sandbox: false }
    }
    /// Create a sandboxed Bash tool.
    pub fn new_sandboxed(blocked: Vec<PathBuf>) -> Self {
        Self { blocked, sandbox: true }
    }
}

fn default_timeout_ms() -> u64 {
    30_000
}

/// Plain (unsandboxed) command execution.
async fn build_plain_command(command: &str) -> std::io::Result<std::process::Output> {
    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await
}

/// Sandboxed command execution.
///
/// On macOS uses `sandbox-exec` with a restrictive profile.
/// On Linux uses `bwrap` if available, otherwise falls back to a plain run with a warning
/// emitted to stderr (bwrap unavailable in many CI environments).
async fn build_sandboxed_command(command: &str) -> std::io::Result<std::process::Output> {
    #[cfg(target_os = "macos")]
    {
        // macOS sandbox-exec profile: deny everything except process exec,
        // file reads, and writes to /tmp.
        const PROFILE: &str = concat!(
            "(version 1)",
            "(deny default)",
            "(allow process-exec)",
            "(allow file-read*)",
            "(allow file-write* (subpath \"/tmp\"))",
            "(allow network-outbound)",
            "(allow signal (target self))",
        );
        tokio::process::Command::new("sandbox-exec")
            .arg("-p")
            .arg(PROFILE)
            .arg("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
    }

    #[cfg(target_os = "linux")]
    {
        // Check if bwrap is available.
        let bwrap_check = tokio::process::Command::new("which")
            .arg("bwrap")
            .output()
            .await;
        if bwrap_check.map(|o| o.status.success()).unwrap_or(false) {
            tokio::process::Command::new("bwrap")
                .args([
                    "--ro-bind", "/", "/",
                    "--tmpfs", "/tmp",
                    "--unshare-net",
                    "--die-with-parent",
                    "sh", "-c", command,
                ])
                .output()
                .await
        } else {
            // bwrap not available — fall back to unsandboxed with a warning.
            tracing::warn!(
                "sandbox requested but bwrap not found; running unsandboxed"
            );
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
                .await
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        // Unsupported platform — run unsandboxed.
        tracing::warn!("sandbox not supported on this platform; running unsandboxed");
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return stdout/stderr."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to run" },
                "timeout": { "type": "integer", "description": "Timeout in milliseconds" }
            },
            "required": ["command"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let command = match input.get("command").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return ToolOutput::err("Missing required field: command".to_string()),
        };

        // Refuse commands that reference any blocked path.
        for blocked in &self.blocked {
            let blocked_str = blocked.to_string_lossy();
            if command.contains(blocked_str.as_ref()) {
                return ToolOutput::err(
                    "Access denied: command references a protected file.".to_string(),
                );
            }
        }
        let timeout_ms = input
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(default_timeout_ms);

        let run_result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
            if self.sandbox {
                build_sandboxed_command(&command).await
            } else {
                build_plain_command(&command).await
            }
        })
        .await;

        let result = match run_result {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return ToolOutput::err(format!("Failed to execute: {}", e)),
            Err(_) => return ToolOutput::err("Command timed out".to_string()),
        };

        let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();

        if result.status.success() {
            ToolOutput::ok(stdout)
        } else {
            let code = result.status.code().unwrap_or(-1);
            ToolOutput::err(format!("Exit code {}\n{}", code, stderr))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_hello() {
        let tool = BashTool::new();
        let out = tool
            .execute(serde_json::json!({ "command": "echo hello" }))
            .await;
        assert!(!out.is_error);
        assert!(out.content.trim() == "hello");
    }

    #[tokio::test]
    async fn exit_nonzero() {
        let tool = BashTool::new();
        let out = tool
            .execute(serde_json::json!({ "command": "exit 1" }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Exit code 1"));
    }

    #[tokio::test]
    async fn missing_command_returns_error() {
        let tool = BashTool::new();
        let out = tool.execute(serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("Missing"));
    }

    #[tokio::test]
    async fn blocked_path_in_command_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("secrets.txt");
        std::fs::write(&blocked, "secret").unwrap();
        let tool = BashTool::new_with_blocked(vec![blocked.clone()]);
        let out = tool
            .execute(serde_json::json!({
                "command": format!("cat {}", blocked.display())
            }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("protected"));
    }

    #[tokio::test]
    async fn stderr_on_error_included() {
        let tool = BashTool::new();
        let out = tool
            .execute(serde_json::json!({ "command": "echo errout >&2; exit 2" }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("errout") || out.content.contains("Exit code 2"));
    }
}
