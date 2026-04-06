# Adding Tools

This page explains how to add a new tool to clido. Tools are the building blocks the agent uses to interact with the filesystem, shell, and external services.

## The `Tool` trait

All tools implement the `Tool` trait defined in `clido-tools/src/lib.rs`:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema for the input object — compiled by the agent before every call.
    fn schema(&self) -> serde_json::Value;
    /// Return `true` for read-only tools (skips write permission prompts; used for plan-mode filtering).
    fn is_read_only(&self) -> bool {
        false
    }
    async fn execute(&self, input: serde_json::Value) -> ToolOutput;
}

pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Stable failure class for retry policy; `None` means "unknown" (heuristics apply).
    pub failure_kind: Option<clido_core::ToolFailureKind>,
    pub path: Option<String>,
    pub content_hash: Option<String>,
    pub mtime_nanos: Option<u64>,
    pub diff: Option<String>,
}
```

Use `ToolOutput::ok`, `ToolOutput::err`, and `ToolOutput::err_kind(message, kind)` helpers in `clido-tools`. The agent validates `input` against `schema()` **before** `execute`; your tool should still return clear `is_error` messages for logical mistakes the schema cannot express.

For recoverable vs permanent failures from the model’s perspective, set `failure_kind` when possible (see `clido_core::tool_failure::ToolFailureKind`).

## Worked example: adding a `FetchUrl` tool

Here is a complete example of adding a tool that fetches the content of a URL.

### Step 1: Create the tool file

Create `crates/clido-tools/src/fetch_url.rs`:

```rust
use async_trait::async_trait;
use clido_core::ToolFailureKind;
use serde_json::{json, Value};

use crate::{Tool, ToolOutput};

pub struct FetchUrlTool;

#[async_trait]
impl Tool for FetchUrlTool {
    fn name(&self) -> &str {
        "FetchUrl"
    }

    fn description(&self) -> &str {
        "Fetch the content of a URL and return it as text. \
         Use this to retrieve documentation, API responses, or web pages."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch (must use https:// or http://)."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes to return (default: 50000).",
                    "default": 50000
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, input: Value) -> ToolOutput {
        let Some(url) = input["url"].as_str() else {
            return ToolOutput::err_kind(
                "missing or invalid 'url' field".into(),
                ToolFailureKind::Logical,
            );
        };

        let max_bytes = input["max_bytes"].as_u64().unwrap_or(50_000) as usize;

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ToolOutput::err_kind(
                format!("Error: URL must use http:// or https://, got: {}", url),
                ToolFailureKind::Logical,
            );
        }

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolOutput::err_kind(
                    format!("Error: build HTTP client: {e}"),
                    ToolFailureKind::Transport,
                );
            }
        };

        let response = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                return ToolOutput::err_kind(
                    format!("Error: request failed: {e}"),
                    ToolFailureKind::Transport,
                );
            }
        };

        let status = response.status();
        let text = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                return ToolOutput::err_kind(
                    format!("Error: read response body: {e}"),
                    ToolFailureKind::Transport,
                );
            }
        };

        if !status.is_success() {
            return ToolOutput::err_kind(
                format!(
                    "HTTP {}: {}",
                    status,
                    &text[..text.len().min(500)]
                ),
                ToolFailureKind::Transport,
            );
        }

        let truncated = &text[..text.len().min(max_bytes)];
        let content = if truncated.len() < text.len() {
            format!("{}\n\n[truncated at {} bytes]", truncated, max_bytes)
        } else {
            truncated.to_string()
        };

        ToolOutput::ok(content)
    }
}
```

### Step 2: Export from the crate

In `crates/clido-tools/src/lib.rs`, add the module and re-export:

```rust
mod fetch_url;
pub use fetch_url::FetchUrlTool;
```

### Step 3: Register in `default_registry`

In `default_registry_with_options()` in `crates/clido-tools/src/lib.rs`:

```rust
pub fn default_registry_with_options(
    workspace_root: PathBuf,
    blocked: Vec<PathBuf>,
    sandbox: bool,
) -> ToolRegistry {
    // ... existing code ...
    r.register(FetchUrlTool);  // add this line
    r
}
```

### Step 4: Add the dependency

If your tool needs a new external crate (e.g. `reqwest`), add it to `crates/clido-tools/Cargo.toml`:

```toml
[dependencies]
reqwest = { version = "0.12", features = ["json"] }
```

### Step 5: Write tests

Add a test module at the bottom of `fetch_url.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clido_core::ToolFailureKind;
    use serde_json::json;

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let tool = FetchUrlTool;
        let result = tool.execute(json!({"url": "ftp://example.com"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("must use http"));
        assert_eq!(result.failure_kind, Some(ToolFailureKind::Logical));
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn fetches_real_url() {
        let tool = FetchUrlTool;
        let result = tool
            .execute(json!({"url": "https://httpbin.org/get", "max_bytes": 1000}))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("url"));
    }
}
```

Run the tests:

```bash
cargo test -p clido-tools -- fetch_url
```

## Input validation guidelines

- The agent rejects calls that violate your JSON Schema before `execute` runs; still validate domain rules inside the tool and return `ToolOutput::err` / `err_kind` so the model can self-correct.
- Prefer returning `ToolOutput` over panicking; `execute` does not use `Result`.
- Truncate long outputs (50,000 bytes is a reasonable ceiling).

## Schema best practices

The JSON Schema `description` fields are read by the LLM to understand how to call your tool. Write them clearly:

- Describe what each field does from the LLM's perspective
- Note constraints (URL schemes, file path restrictions, line count limits)
- Include examples in the description where helpful
- Use `"required"` for fields that must be present

## Read-only vs state-changing tools

The permission layer uses `Tool::is_read_only()`. Override it to return `true` only for tools that never mutate disk or run shell commands without gating. Any tool with the default `false` is treated as state-changing: in `default` permission mode the TUI prompts before execution (when `AskUser` is wired).

## Registering MCP tools

MCP tools are loaded dynamically at runtime and do not need to be registered in `default_registry`. See [MCP Servers](/docs/guide/mcp) for the user-facing guide and `crates/clido-tools/src/mcp.rs` for the implementation.
