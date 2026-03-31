# config.toml Reference

Full reference for all configuration keys in `config.toml`.

See [Configuration guide](/guide/configuration) for a conceptual overview and instructions on changing config values.

## File location

| Platform | Default path |
|----------|-------------|
| Linux | `~/.config/clido/config.toml` |
| macOS | `~/Library/Application Support/clido/config.toml` or `~/.config/clido/config.toml` |

Override with `CLIDO_CONFIG` environment variable. Project-level config at `.clido/config.toml` is merged on top.

## Complete annotated example

```toml
# ─────────────────────────────────────────────────────────────────────────────
# Top-level keys
# ─────────────────────────────────────────────────────────────────────────────

# The profile to use when --profile is not specified.
# Must match a key in the [profile.*] table.
# Type: string  Default: "default"
default_profile = "default"

# ─────────────────────────────────────────────────────────────────────────────
# [profile.<name>]
# Each profile defines one provider + model combination.
# ─────────────────────────────────────────────────────────────────────────────

[profile.default]
# Provider name. Required.
# Valid values: "anthropic", "openai", "openrouter", "minimax", "alibabacloud", "local"
provider = "anthropic"

# Model name as recognised by the provider. Required.
model = "claude-3-5-sonnet-20241022"

# Name of the environment variable holding the API key. Recommended.
# If both api_key and api_key_env are set, api_key takes precedence.
api_key_env = "ANTHROPIC_API_KEY"

# API key stored directly in the config file. Not recommended for shared machines.
# api_key = "sk-ant-..."

# Custom base URL (for local models, Azure, or self-hosted endpoints).
# Default: provider's official endpoint.
# base_url = "http://localhost:11434"

[profile.fast]
provider    = "anthropic"
model       = "claude-3-haiku-20240307"
api_key_env = "ANTHROPIC_API_KEY"

[profile.openrouter]
provider    = "openrouter"
model       = "anthropic/claude-3-5-sonnet"
api_key_env = "OPENROUTER_API_KEY"

[profile.local]
provider = "local"
model    = "llama3.2"
base_url = "http://localhost:11434"

[profile.minimax]
provider    = "minimax"
model       = "MiniMax-M2.7"
api_key_env = "MINIMAX_API_KEY"

[profile.alibaba]
provider    = "alibabacloud"
model       = "qwen-max"
api_key_env = "DASHSCOPE_API_KEY"

# ─────────────────────────────────────────────────────────────────────────────
# [profile.<name>.fast]
# Optional fast/cheap provider for utility tasks (titles, summaries, commit
# messages, prompt enhancement). Falls back to the main provider when not set.
# ─────────────────────────────────────────────────────────────────────────────

[profile.default.fast]
provider = "openai"
model    = "gpt-4o-mini"

# ─────────────────────────────────────────────────────────────────────────────
# [roles]
# Map role names to model IDs for use by /fast and /smart in the TUI.
# ─────────────────────────────────────────────────────────────────────────────

[roles]
fast      = "claude-haiku-4-5-20251001"
reasoning = "claude-opus-4-6"

# ─────────────────────────────────────────────────────────────────────────────
# [agent]
# ─────────────────────────────────────────────────────────────────────────────

[agent]
# Maximum number of agent turns per session.
# Type: integer  Default: 50
max-turns = 50

# Maximum spend per session in USD. Null = no budget limit.
# Type: float or null  Default: 5.0
max-budget-usd = 5.0

# Maximum number of concurrent read-only tool calls.
# Type: integer  Default: 4
max-concurrent-tools = 4

# ─────────────────────────────────────────────────────────────────────────────
# [context]
# ─────────────────────────────────────────────────────────────────────────────

[context]
# Compact conversation history when token usage exceeds this fraction of the
# context window. Range: 0.0–1.0  Default: 0.75
compaction-threshold = 0.75

# Override the maximum context window size.
# Default: model-specific value from pricing table (e.g. 200000 for Claude 3.5 Sonnet).
# max-context-tokens = 180000

# ─────────────────────────────────────────────────────────────────────────────
# [tools]
# ─────────────────────────────────────────────────────────────────────────────

[tools]
# Restrict the agent to only these tools. Empty list = all tools allowed.
# Type: list of strings  Default: []
allowed = []

# Always disallow these tools, even if they appear in `allowed`.
# Type: list of strings  Default: []
disallowed = []

# ─────────────────────────────────────────────────────────────────────────────
# [workflows]
# ─────────────────────────────────────────────────────────────────────────────

[workflows]
# Directory where workflow YAML files are looked up by name.
# Type: string  Default: ".clido/workflows"
directory = ".clido/workflows"

# ─────────────────────────────────────────────────────────────────────────────
# [hooks]
# Shell commands run around each tool call.
# Available environment variables:
#   CLIDO_TOOL_NAME       — name of the tool
#   CLIDO_TOOL_INPUT      — JSON-encoded tool input
#   CLIDO_TOOL_OUTPUT     — tool output (post_tool_use only)
#   CLIDO_TOOL_IS_ERROR   — "true" or "false" (post_tool_use only)
#   CLIDO_TOOL_DURATION_MS — duration in ms (post_tool_use only)
# ─────────────────────────────────────────────────────────────────────────────

[hooks]
# Run before each tool call. Non-zero exit code blocks the tool call.
# Type: string (shell command)  Default: ""
pre_tool_use  = ""

# Run after each tool call. Exit code is ignored.
# Type: string (shell command)  Default: ""
post_tool_use = ""
```

## Key reference

### Top-level

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `default_profile` | string | `"default"` | Name of the default profile |

### `[profile.<name>]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `provider` | string | Yes | Provider: `anthropic`, `openai`, `openrouter`, `minimax`, `alibabacloud`, `local` |
| `model` | string | Yes | Model name |
| `api_key` | string | No | API key (stored in plain text) |
| `api_key_env` | string | No | Environment variable name for API key |
| `base_url` | string | No | Custom endpoint URL |

### `[profile.<name>.fast]`

Optional fast/cheap provider for utility tasks (title generation, summaries, commit messages, prompt enhancement). Falls back to the main profile provider when not set.

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `provider` | string | Yes | Provider identifier |
| `model` | string | Yes | Model name |
| `api_key` | string | No | API key (stored in plain text) |
| `api_key_env` | string | No | Environment variable name for API key |
| `base_url` | string | No | Custom endpoint URL |

### `[agent]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `max-turns` | integer | `50` | Maximum turns per session |
| `max-budget-usd` | float | `5.0` | Maximum cost per session |
| `max-concurrent-tools` | integer | `4` | Max parallel tool calls |

### `[context]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `compaction-threshold` | float | `0.75` | Context compaction trigger fraction |
| `max-context-tokens` | integer | model default | Context window override |

### `[tools]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `allowed` | list | `[]` | Allowed tool names (empty = all) |
| `disallowed` | list | `[]` | Disallowed tool names |

### `[workflows]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `directory` | string | `.clido/workflows` | Workflow search directory |

### `[hooks]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `pre_tool_use` | string | `""` | Shell command before each tool call |
| `post_tool_use` | string | `""` | Shell command after each tool call |

### `[roles]`

Maps role names to model IDs. Used by `/fast` and `/smart` TUI commands.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `fast` | string | `claude-haiku-4-5-20251001` | Model for `/fast` |
| `reasoning` | string | `claude-opus-4-6` | Model for `/smart` |

```toml
[roles]
fast      = "claude-haiku-4-5-20251001"
reasoning = "claude-opus-4-6"
```

User-level model favorites/recency are stored in `~/.config/clido/model_prefs.json` and managed through the TUI (`/fav`, `/models`).

## `permission_mode` values

The permission mode can be set via `--permission-mode` flag or environment variable. Config-level default is not currently supported but planned.

| Value | Description |
|-------|-------------|
| `default` | Prompt in TUI, allow automatically in non-TTY |
| `accept-all` | Allow all tool calls without prompting |
| `plan` | No tool calls; agent responds with text only |

## Precedence

Values are resolved in this order (later values override earlier ones):

1. Built-in defaults (in `clido-core`)
2. Global `~/.config/clido/config.toml`
3. Project `.clido/config.toml`
4. Environment variables
5. Command-line flags

See [Environment Variables](/reference/env-vars) for the full variable list.
