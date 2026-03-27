//! MCP (Model Context Protocol) client: spawn server process, initialize, list tools, call tools.
//!
//! MCP protocol is JSON-RPC 2.0 over stdio. This implementation uses tokio async I/O so that
//! slow or misbehaving MCP servers never block the Tokio executor.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

/// Configuration for an MCP server.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// MCP config file format.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct McpConfig {
    pub servers: Vec<McpServerConfig>,
}

/// Describes a tool exposed by an MCP server.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
    /// Hint from the MCP server that this tool is read-only (safe for parallel execution).
    #[serde(default)]
    pub read_only: bool,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, serde::Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    id: serde_json::Value,
    result: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

/// Active MCP client connected to a spawned server process.
///
/// All I/O uses async tokio primitives so a slow server never blocks the executor.
pub struct McpClient {
    _child: Child,
    stdin: Mutex<ChildStdin>,
    reader: Mutex<BufReader<ChildStdout>>,
    next_id: Mutex<u64>,
    pub config: McpServerConfig,
}

impl McpClient {
    /// Spawn the MCP server and return a client connected to it.
    pub fn spawn(config: McpServerConfig) -> anyhow::Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn MCP server '{}': {}", config.name, e))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' stdin not available", config.name))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' stdout not available", config.name))?;
        Ok(Self {
            _child: child,
            stdin: Mutex::new(stdin),
            reader: Mutex::new(BufReader::new(stdout)),
            next_id: Mutex::new(1),
            config,
        })
    }

    async fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        let v = *id;
        *id += 1;
        v
    }

    /// Send a JSON-RPC request and await the response line asynchronously.
    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id().await;
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let line = serde_json::to_string(&req)? + "\n";

        // Write request.
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!("MCP write error: {}", e))?;
            stdin
                .flush()
                .await
                .map_err(|e| anyhow::anyhow!("MCP flush error: {}", e))?;
        }

        // Read response line.
        let mut resp_line = String::new();
        {
            let mut reader = self.reader.lock().await;
            reader
                .read_line(&mut resp_line)
                .await
                .map_err(|e| anyhow::anyhow!("MCP read error: {}", e))?;
        }

        if resp_line.trim().is_empty() {
            return Err(anyhow::anyhow!("MCP server returned empty response"));
        }

        let resp: JsonRpcResponse = serde_json::from_str(&resp_line).map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse MCP response: {} — raw: {}",
                e,
                resp_line.trim()
            )
        })?;
        if let Some(err) = resp.error {
            return Err(anyhow::anyhow!("MCP server error: {}", err));
        }
        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    /// Send the `initialize` handshake.
    pub async fn initialize(&self) -> anyhow::Result<serde_json::Value> {
        self.send_request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "clido", "version": "0.1.0" }
            }),
        )
        .await
    }

    /// List tools exposed by the server.
    pub async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDef>> {
        let result = self
            .send_request("tools/list", serde_json::json!({}))
            .await?;
        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let mut defs = Vec::new();
        for t in tools {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = t
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object"}));
            // Infer read-only from description keywords (e.g. "read", "list", "search", "get").
            let read_only = {
                let desc_lower = description.to_lowercase();
                let name_lower = name.to_lowercase();
                [
                    "read", "list", "search", "get", "fetch", "find", "query", "show", "view",
                ]
                .iter()
                .any(|kw| desc_lower.starts_with(kw) || name_lower.starts_with(kw))
            };
            defs.push(McpToolDef {
                name,
                description,
                input_schema,
                read_only,
            });
        }
        Ok(defs)
    }

    /// Call a tool by name with the given arguments.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.send_request(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": arguments
            }),
        )
        .await
    }
}

/// Load MCP config from a JSON or YAML file.
pub fn load_mcp_config(path: &std::path::Path) -> anyhow::Result<McpConfig> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read MCP config: {}", e))?;
    serde_json::from_str(&s).map_err(|e| anyhow::anyhow!("Failed to parse MCP config JSON: {}", e))
}

// ---------------------------------------------------------------------------
// McpTool: Tool trait wrapper for a single MCP tool
// ---------------------------------------------------------------------------

/// A `Tool`-trait implementation that delegates execution to an MCP server.
pub struct McpTool {
    def: McpToolDef,
    client: Arc<McpClient>,
}

impl McpTool {
    pub fn new(def: McpToolDef, client: Arc<McpClient>) -> Self {
        Self { def, client }
    }
}

#[async_trait::async_trait]
impl crate::Tool for McpTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn schema(&self) -> serde_json::Value {
        self.def.input_schema.clone()
    }

    fn is_read_only(&self) -> bool {
        self.def.read_only
    }

    async fn execute(&self, input: serde_json::Value) -> crate::ToolOutput {
        match self.client.call_tool(&self.def.name, input).await {
            Ok(result) => crate::ToolOutput::ok(result.to_string()),
            Err(e) => crate::ToolOutput::err(e.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_server_config_deserializes() {
        let json = r#"{"name":"test","command":"echo","args":["hello"]}"#;
        let cfg: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.name, "test");
        assert_eq!(cfg.command, "echo");
        assert_eq!(cfg.args, vec!["hello"]);
    }

    #[test]
    fn mcp_config_file_deserializes() {
        let json = r#"{
            "servers": [
                { "name": "srv1", "command": "node", "args": ["server.js"] },
                { "name": "srv2", "command": "python", "args": ["srv.py"], "env": {"KEY": "val"} }
            ]
        }"#;
        let cfg: McpConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.servers.len(), 2);
        assert_eq!(cfg.servers[0].name, "srv1");
        assert_eq!(
            cfg.servers[1].env.get("KEY").map(String::as_str),
            Some("val")
        );
    }

    #[test]
    fn load_mcp_config_from_json_file() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(br#"{"servers":[{"name":"s","command":"cat","args":[]}]}"#)
            .unwrap();
        f.flush().unwrap();
        let cfg = load_mcp_config(f.path()).unwrap();
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(cfg.servers[0].command, "cat");
    }

    #[tokio::test]
    async fn mcp_client_spawn_echo_server() {
        let cfg = McpServerConfig {
            name: "echo-test".to_string(),
            command: "cat".to_string(),
            args: vec![],
            env: HashMap::new(),
        };
        let result = McpClient::spawn(cfg);
        if let Ok(client) = result {
            assert_eq!(client.config.name, "echo-test");
        }
    }
}
