# MCP Servers

clido supports the [Model Context Protocol](https://modelcontextprotocol.io/) (MCP), which allows external tool servers to be connected to the agent. MCP tools appear to the agent alongside built-in tools like Bash and Read.

## What MCP is

MCP is an open protocol for exposing tools to LLM agents over a stdio JSON-RPC 2.0 interface. An MCP server is any program that speaks the MCP protocol: it advertises a set of tools, accepts tool call requests, and returns results.

This lets you extend clido with tools that:

- Call external APIs (GitHub, Jira, Slack, databases)
- Provide domain-specific knowledge (documentation search, internal services)
- Bridge to other systems (CI/CD, deployment platforms)

## Configuration

Pass the path to an MCP config file with `--mcp-config`:

```bash
clido --mcp-config mcp-servers.json "create a GitHub issue for this bug"
```

You can also set a default MCP config in your profile (editing `config.toml` directly — this is not yet configurable via `clido config set`):

```toml
[profile.default]
provider    = "anthropic"
model       = "claude-sonnet-4-5"
api_key_env = "ANTHROPIC_API_KEY"
# mcp_config = ".clido/mcp.json"   # planned for a future version
```

## MCP config format

The MCP config is a JSON file listing the servers to start:

```json
{
  "servers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"
      }
    },
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp/scratch"]
    },
    "my-internal-tool": {
      "command": "/usr/local/bin/my-mcp-server",
      "args": ["--config", "/etc/my-tool/config.json"],
      "env": {
        "API_URL": "https://internal.example.com"
      }
    }
  }
}
```

Each server entry has:

| Field | Required | Description |
|-------|----------|-------------|
| `command` | Yes | Executable to run |
| `args` | No | Command-line arguments |
| `env` | No | Environment variables (supports `${VAR}` substitution from the host environment) |

## How MCP tools appear to the agent

clido initialises each MCP server at startup, retrieves its tool list, and registers those tools alongside the built-in tools. The agent sees a flat list of tool names and descriptions — it does not know which tools are built-in and which come from MCP servers.

For example, if the GitHub MCP server advertises a `create_issue` tool, the agent can call it directly:

```
[tool: create_issue]  repository=my-org/my-repo  title="Bug: parse error"  body="..."
```

## Server lifecycle

1. clido spawns each MCP server as a child process on startup
2. The MCP initialization handshake is performed over stdio
3. The tool list is fetched and merged into the registry
4. Tool calls are dispatched to the appropriate server over stdin/stdout
5. When clido exits, all MCP server processes are terminated

::: warning Startup latency
Each MCP server adds startup latency. Servers that download packages at start time (e.g. `npx -y ...`) can add several seconds to startup. Consider pre-installing servers to avoid this.
:::

## Supported MCP servers

Any server that implements the MCP stdio transport can be used. The MCP ecosystem includes servers for:

| Server | Package | What it provides |
|--------|---------|-----------------|
| GitHub | `@modelcontextprotocol/server-github` | Issues, PRs, repos, file contents |
| Filesystem | `@modelcontextprotocol/server-filesystem` | File read/write with path scoping |
| PostgreSQL | `@modelcontextprotocol/server-postgres` | SQL queries |
| Slack | `@modelcontextprotocol/server-slack` | Messages, channels, search |
| Brave Search | `@modelcontextprotocol/server-brave-search` | Web search |
| Fetch | `@modelcontextprotocol/server-fetch` | HTTP requests |

See the [MCP server registry](https://github.com/modelcontextprotocol/servers) for the full list.

## Troubleshooting

### Server fails to start

Check that the command exists and is executable:

```bash
which npx
npx -y @modelcontextprotocol/server-github --help
```

Enable verbose logging to see the full server stderr:

```bash
clido --verbose --mcp-config mcp-servers.json "test prompt"
```

### Tools not appearing

Verify the server starts and advertises tools:

```bash
CLIDO_LOG=debug clido --mcp-config mcp-servers.json --print "list available tools"
```

### Environment variables not set

The `${VAR}` substitution reads from the current process environment. Ensure the variables are exported:

```bash
export GITHUB_TOKEN=ghp_...
clido --mcp-config mcp-servers.json "create issue"
```
