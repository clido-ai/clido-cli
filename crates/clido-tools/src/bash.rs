//! Bash tool: execute shell commands.

use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;

use crate::{Tool, ToolOutput};

/// Execute shell commands via sh -c.
#[derive(Default)]
pub struct BashTool {
    blocked: Vec<PathBuf>,
}

impl BashTool {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn new_with_blocked(blocked: Vec<PathBuf>) -> Self {
        Self { blocked }
    }
}

fn default_timeout_ms() -> u64 {
    30_000
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

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .output();

        let result = match tokio::time::timeout(Duration::from_millis(timeout_ms), output).await {
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
}
