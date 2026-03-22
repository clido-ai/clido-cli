//! TestLoop tool: run tests and return structured results for the agent to act on.
//!
//! This tool is intentionally stateless per call. The agent loop handles iteration
//! by reading the structured output, applying fixes via other tools (Edit, Write, Bash),
//! and then calling TestLoop again. This keeps the tool simple and correct.

use async_trait::async_trait;
use std::path::PathBuf;

use crate::test_runner::{run_tests, TestFailure, TestResult};
use crate::{Tool, ToolOutput};

/// Run the test suite in the given working directory and return structured results.
///
/// Parameters (all optional):
/// - `command`: Override the test command. If absent, auto-detect from workspace files.
/// - `workdir`: Working directory to run tests in. Defaults to current directory.
/// - `max_iterations`: Informational — included in output so the agent knows the limit.
pub struct TestLoopTool {
    /// Workspace root used as the default working directory.
    workspace_root: PathBuf,
}

impl TestLoopTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait]
impl Tool for TestLoopTool {
    fn name(&self) -> &str {
        "TestLoop"
    }

    fn description(&self) -> &str {
        "Run the test suite and report results. Call this tool repeatedly: read the failures, fix the code with Edit/Write, then call TestLoop again to verify. The tool auto-detects the runner (cargo, pytest, npm, go) from workspace files."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Override the test command (e.g. 'cargo test', 'pytest', 'go test ./...'). Auto-detected from workspace if omitted."
                },
                "workdir": {
                    "type": "string",
                    "description": "Directory to run tests in. Defaults to the workspace root."
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Maximum number of times the agent should call this tool before giving up. Default 5. Used as a hint in the output.",
                    "default": 5
                }
            },
            "required": []
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        // Extract all values from `input` before moving into the closure.
        let command: Option<String> = input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let max_iterations = input
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .unwrap_or(5);

        let workdir = if let Some(wd) = input.get("workdir").and_then(|v| v.as_str()) {
            PathBuf::from(wd)
        } else {
            self.workspace_root.clone()
        };

        if !workdir.exists() {
            return ToolOutput::err(format!(
                "Working directory does not exist: {}",
                workdir.display()
            ));
        }

        // Run tests synchronously (blocking) — acceptable because this is a
        // long-running operation and the caller expects to wait.
        let result =
            tokio::task::spawn_blocking(move || run_tests(&workdir, command.as_deref())).await;

        let result = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return ToolOutput::err(e),
            Err(e) => return ToolOutput::err(format!("Test runner panicked: {}", e)),
        };

        ToolOutput::ok(format_result(&result, max_iterations))
    }
}

/// Format a `TestResult` into a concise, agent-readable string.
fn format_result(result: &TestResult, max_iterations: u64) -> String {
    if result.all_pass() {
        return format!(
            "All tests passing. {} passed, {} skipped. Duration: {}ms.\n\nNo action needed.",
            result.passed, result.skipped, result.duration_ms
        );
    }

    let mut out = String::new();

    out.push_str(&format!(
        "Test results: {} passed, {} failed, {} skipped. Duration: {}ms.\n",
        result.passed, result.failed, result.skipped, result.duration_ms
    ));
    out.push_str(&format!(
        "Hint: fix the failures below, then call TestLoop again (up to {} total attempts).\n\n",
        max_iterations
    ));
    out.push_str("Failing tests:\n");

    for (i, failure) in result.failures.iter().enumerate() {
        out.push_str(&format_failure(i + 1, failure));
    }

    out
}

fn format_failure(index: usize, failure: &TestFailure) -> String {
    let mut s = format!("{}. {}\n", index, failure.name);
    if !failure.output.is_empty() {
        for line in failure.output.lines() {
            s.push_str(&format!("   {}\n", line));
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_runner::TestFailure;

    fn make_result(passed: u32, failed: u32, failures: Vec<TestFailure>) -> TestResult {
        TestResult {
            passed,
            failed,
            skipped: 0,
            duration_ms: 42,
            failures,
        }
    }

    #[test]
    fn format_all_pass() {
        let r = make_result(5, 0, vec![]);
        let s = format_result(&r, 5);
        assert!(s.contains("All tests passing"));
        assert!(s.contains("5 passed"));
    }

    #[test]
    fn format_with_failures() {
        let r = make_result(
            3,
            1,
            vec![TestFailure {
                name: "foo::bar::test_thing".to_string(),
                output: "assertion failed: left == right".to_string(),
            }],
        );
        let s = format_result(&r, 5);
        assert!(s.contains("1 failed"));
        assert!(s.contains("foo::bar::test_thing"));
        assert!(s.contains("assertion failed"));
        assert!(s.contains("TestLoop again"));
    }

    #[tokio::test]
    async fn tool_missing_workdir_returns_error() {
        let tool = TestLoopTool::new(PathBuf::from("/nonexistent/path/xyz"));
        let out = tool
            .execute(serde_json::json!({
                "workdir": "/nonexistent/path/xyz"
            }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("does not exist"));
    }

    #[tokio::test]
    async fn tool_runs_in_temp_cargo_workspace() {
        // Create a minimal Cargo workspace with one passing test.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "test-proj"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            r#"
#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
"#,
        )
        .unwrap();

        let tool = TestLoopTool::new(dir.path().to_path_buf());
        let out = tool.execute(serde_json::json!({})).await;
        // cargo may or may not be available in CI, so only check non-panic.
        // If cargo is available the result should show passing tests.
        if !out.is_error {
            assert!(out.content.contains("passing") || out.content.contains("passed"));
        }
    }

    #[test]
    fn tool_name_and_description() {
        let tool = TestLoopTool::new(PathBuf::from("."));
        assert_eq!(tool.name(), "TestLoop");
        assert!(!tool.description().is_empty());
        assert!(!tool.is_read_only());
    }

    #[test]
    fn schema_has_expected_properties() {
        let tool = TestLoopTool::new(PathBuf::from("."));
        let schema = tool.schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("command"));
        assert!(props.contains_key("workdir"));
        assert!(props.contains_key("max_iterations"));
    }
}
