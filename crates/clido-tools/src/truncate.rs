//! TruncateTool: captures large outputs and stores them, preventing LLM context overflow.
//!
//! When a command produces more than MAX_LINES lines or MAX_BYTES bytes of output,
//! the full output is saved to a temp file and a preview + path is returned.
//! The agent can then read specific parts with the Read tool if needed.

use async_trait::async_trait;

use crate::{Tool, ToolOutput};

const MAX_LINES: usize = 500;
const MAX_BYTES: usize = 100_000; // 100 KB

/// Truncate a large string: if it exceeds limits, store the full content and return a preview.
///
/// Returns `(content_to_return, was_truncated)`.
pub fn truncate_large_output(content: &str, label: &str) -> (String, bool) {
    let line_count = content.lines().count();
    let byte_count = content.len();

    if line_count <= MAX_LINES && byte_count <= MAX_BYTES {
        return (content.to_string(), false);
    }

    // Save full output to a temp file.
    let id = format!("{:x}", rand_id());
    let dir = std::env::temp_dir().join("clido_truncation");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{}.txt", id));

    if std::fs::write(&path, content).is_err() {
        // If we can't write, just truncate in-place and warn.
        let truncated: String = content
            .lines()
            .take(MAX_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        return (
            format!(
                "{}\n\n[... {} lines truncated — could not save full output ...]",
                truncated,
                line_count.saturating_sub(MAX_LINES)
            ),
            true,
        );
    }

    let preview: String = content.lines().take(50).collect::<Vec<_>>().join("\n");
    let summary = format!(
        "{}\n\n[OUTPUT TRUNCATED]\n\
        Full output ({line_count} lines, {byte_count} bytes) saved to: {}\n\
        Source: {label}\n\
        Use the Read tool to inspect specific sections:\n  Read {{ \"file_path\": \"{}\" }}",
        preview,
        path.display(),
        path.display(),
    );
    (summary, true)
}

/// Simple non-crypto pseudo-random for temp file naming (no external dep needed).
fn rand_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(12345);
    // Mix with process ID for uniqueness across rapid calls.
    let pid = std::process::id() as u64;
    (nanos as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(pid)
}

/// Tool that wraps any large text output with truncation + file storage.
///
/// Not directly called by the LLM — instead used internally by BashTool and WebFetchTool
/// via the `truncate_large_output` helper above. Exposed as a tool so agents can also
/// explicitly truncate output they've already received.
pub struct TruncateTool;

impl TruncateTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TruncateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TruncateTool {
    fn name(&self) -> &str {
        "Truncate"
    }

    fn description(&self) -> &str {
        "Save large text content to a temp file and return a preview with the file path. \
         Use when you have very large output (>500 lines or >100KB) that would overflow context. \
         Returns the first 50 lines as preview plus the path to read the rest."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "The large text to truncate and store" },
                "label":   { "type": "string", "description": "Optional label describing what this content is" }
            },
            "required": ["content"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolOutput::err("Missing required field: content"),
        };
        let label = input
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("agent output");

        let (result, truncated) = truncate_large_output(content, label);
        if truncated {
            ToolOutput::ok(result)
        } else {
            ToolOutput::ok(format!(
                "{}\n\n[Content fits within limits — no truncation needed]",
                result
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn small_content_not_truncated() {
        let (out, truncated) = truncate_large_output("hello\nworld", "test");
        assert!(!truncated);
        assert_eq!(out, "hello\nworld");
    }

    #[test]
    fn large_line_count_triggers_truncation() {
        let content: String = (0..600).map(|i| format!("line {}\n", i)).collect();
        let (out, truncated) = truncate_large_output(&content, "test");
        assert!(truncated);
        assert!(out.contains("OUTPUT TRUNCATED"));
        assert!(out.contains("clido_truncation"));
    }

    #[test]
    fn large_byte_count_triggers_truncation() {
        // 200KB of content in one line triggers byte limit even with 1 line
        let content = "x".repeat(200_001);
        let (out, truncated) = truncate_large_output(&content, "test");
        assert!(truncated);
        assert!(out.contains("OUTPUT TRUNCATED"));
    }

    #[tokio::test]
    async fn tool_execute_large_content() {
        let tool = TruncateTool::new();
        let content: String = (0..600).map(|i| format!("line {}\n", i)).collect();
        let result = tool
            .execute(json!({"content": content, "label": "test output"}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("OUTPUT TRUNCATED"));
    }

    #[tokio::test]
    async fn tool_execute_small_content() {
        let tool = TruncateTool::new();
        let result = tool.execute(json!({"content": "just a few lines"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("no truncation needed"));
    }
}
