# Adding Tools

This page explains how to add a new tool to clido. Tools are the building blocks the agent uses to interact with the filesystem, shell, and external services.

## The `Tool` trait

All tools implement the `Tool` trait defined in `clido-tools/src/lib.rs`:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    /// The name the LLM uses to call this tool. Must be unique in the registry.
    fn name(&self) -> &str;

    /// Human-readable description sent to the LLM in the tool list.
    fn description(&self) -> &str;

    /// JSON Schema for the input object. Used to validate calls and inform the LLM.
    fn schema(&self) -> serde_json::Value;

    /// Execute the tool. Returns a `ToolOutput` with the result text and any metadata.
    async fn execute(
        &self,
        input: serde_json::Value,
        workspace_root: &std::path::Path,
    ) -> anyhow::Result<ToolOutput>;
}

pub struct ToolOutput {
    /// Text returned to the LLM (and stored in the session).
    pub content: String,
    /// Whether this is an error (affects how the LLM interprets the result).
    pub is_error: bool,
    /// For Edit operations: the unified diff (used by TUI for display).
    pub diff: Option<String>,
}
```

## Worked example: adding a `FetchUrl` tool

Here is a complete example of adding a tool that fetches the content of a URL.

### Step 1: Create the tool file

Create `crates/clido-tools/src/fetch_url.rs`:

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

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

    async fn execute(
        &self,
        input: Value,
        _workspace_root: &Path,
    ) -> anyhow::Result<ToolOutput> {
        let url = input["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'url' field"))?;

        let max_bytes = input["max_bytes"]
            .as_u64()
            .unwrap_or(50_000) as usize;

        // Validate URL scheme
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(ToolOutput {
                content: format!("Error: URL must use http:// or https://, got: {}", url),
                is_error: true,
                diff: None,
            });
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        let response = client.get(url).send().await?;
        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            return Ok(ToolOutput {
                content: format!("HTTP {}: {}", status, &text[..text.len().min(500)]),
                is_error: true,
                diff: None,
            });
        }

        let truncated = &text[..text.len().min(max_bytes)];
        let content = if truncated.len() < text.len() {
            format!("{}\n\n[truncated at {} bytes]", truncated, max_bytes)
        } else {
            truncated.to_string()
        };

        Ok(ToolOutput {
            content,
            is_error: false,
            diff: None,
        })
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
    use serde_json::json;

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let tool = FetchUrlTool;
        let result = tool
            .execute(json!({"url": "ftp://example.com"}), std::path::Path::new("/"))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("must use http"));
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn fetches_real_url() {
        let tool = FetchUrlTool;
        let result = tool
            .execute(
                json!({"url": "https://httpbin.org/get", "max_bytes": 1000}),
                std::path::Path::new("/"),
            )
            .await
            .unwrap();
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

- Always validate required fields and return `ToolOutput { is_error: true, ... }` for invalid input (not `Err(...)`) — the LLM needs to see the error to self-correct
- Use `anyhow::Result` for genuine I/O errors that are not recoverable
- Truncate long outputs (50,000 bytes is a reasonable ceiling)

## Schema best practices

The JSON Schema `description` fields are read by the LLM to understand how to call your tool. Write them clearly:

- Describe what each field does from the LLM's perspective
- Note constraints (URL schemes, file path restrictions, line count limits)
- Include examples in the description where helpful
- Use `"required"` for fields that must be present

## Read-only vs state-changing tools

The permission system determines whether `AskUser::ask()` is called before the tool. Currently, the set of state-changing tools is hardcoded in the agent loop as: `Bash`, `Write`, `Edit`.

To mark your tool as state-changing (so the user is prompted before execution in `default` permission mode), you need to add its name to the list in `crates/clido-agent/src/agent_loop.rs`:

```rust
fn is_state_changing(tool_name: &str) -> bool {
    matches!(tool_name, "Bash" | "Write" | "Edit" | "FetchUrl")
}
```

## Registering MCP tools

MCP tools are loaded dynamically at runtime and do not need to be registered in `default_registry`. See [MCP Servers](/docs/guide/mcp) for the user-facing guide and `crates/clido-tools/src/mcp.rs` for the implementation.
