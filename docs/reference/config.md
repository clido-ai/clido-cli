# config.toml Reference

Full reference for all configuration keys in `config.toml`.

See [Configuration guide](/docs/guide/configuration) for a conceptual overview and instructions on changing config values.

## File location

| Platform | Default path |
|----------|-------------|
| Linux | `~/.config/clido/config.toml` |
| macOS | `~/Library/Application Support/clido/config.toml` or `~/.config/clido/config.toml` |

Override with `CLIDO_CONFIG` environment variable. Project-level config at `.clido/config.toml` is merged on top.

## Credentials file

API keys are stored separately from `config.toml` in a `credentials` file in the same directory:

| Platform | Default path |
|----------|-------------|
| Linux | `~/.config/clido/credentials` |
| macOS | `~/Library/Application Support/clido/credentials` or `~/.config/clido/credentials` |

The credentials file uses TOML format:

```toml
[keys]
anthropic = "sk-ant-..."
openrouter = "sk-or-..."
```

This file is created automatically during setup with chmod 600 permissions. API keys are resolved in this order:

1. Environment variable (e.g. `ANTHROPIC_API_KEY`)
2. Credentials file (`credentials` alongside config.toml)
3. Inline `api_key` in config.toml (legacy, not recommended)

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
# Valid values: "anthropic", "openai", "openrouter", "gemini", "deepseek", "mistral", "xai", "groq", "togetherai", "fireworks", "cerebras", "perplexity", "minimax", "alibabacloud", "kimi", "kimi-code", "local"
provider = "anthropic"

# Model name as recognised by the provider. Required.
model = "claude-sonnet-4-5"

# Name of the environment variable holding the API key. Recommended.
# If both api_key and api_key_env are set, api_key takes precedence.
api_key_env = "ANTHROPIC_API_KEY"

# Legacy fallback. The setup wizard stores keys in the credentials file instead.
# api_key = "sk-ant-..."

# Custom base URL (for local models, Azure, or self-hosted endpoints).
# Default: provider's official endpoint.
# base_url = "http://localhost:11434"

[profile.fast]
provider    = "anthropic"
model       = "claude-haiku-4-5"
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
# [roles]  (legacy — parsed for backwards compatibility; prefer [profile.*.fast])
# Optional hints for /fast and /smart in the TUI when present.
# ─────────────────────────────────────────────────────────────────────────────

[roles]
fast      = "claude-haiku-4-5-20251001"
reasoning = "claude-opus-4-6"

# ─────────────────────────────────────────────────────────────────────────────
# [agent]
# ─────────────────────────────────────────────────────────────────────────────

[agent]
# Maximum number of agent turns per session.
# Type: integer  Default: 200
max-turns = 200

# Maximum spend per session in USD. Omit or null = no budget limit.
# Type: float or null  Default: null
# max-budget-usd = 5.0

# Maximum number of concurrent read-only tool calls.
# Type: integer  Default: 4
max-concurrent-tools = 4

# Use provider streaming completion and aggregate to a full response (provider must support it).
# Type: boolean  Default: false
# stream-model-completion = false

# Per-tool execute timeout in seconds (agent loop wrapper around Tool::execute).
# Type: integer  Default: 60
# tool-timeout-secs = 60

# Truncate tool output text beyond this many bytes (0 = unlimited).
# Type: integer  Default: 512000
# max-tool-output-bytes = 512000

# Structured harness: JSON tasks under .clido/harness/, TodoWrite disabled, reviewer-only pass.
# Type: boolean  Default: false
# harness = true

# ─────────────────────────────────────────────────────────────────────────────
# [context]
# ─────────────────────────────────────────────────────────────────────────────

[context]
# Compact conversation history when token usage exceeds this fraction of the
# context window. Range: 0.0–1.0  Default: ~0.58
compaction-threshold = 0.58

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

# ─────────────────────────────────────────────────────────────────────────────
# [index]  — SemanticSearch / clido index
# ─────────────────────────────────────────────────────────────────────────────

[index]
# exclude-patterns = ["*.lock", "vendor/**"]
# include-ignored = false

# ─────────────────────────────────────────────────────────────────────────────
# [skills]  — reusable agent instructions (.clido/skills/, ~/.clido/skills/)
# ─────────────────────────────────────────────────────────────────────────────

[skills]
# disabled = ["id-to-hide"]
# enabled = ["id1", "id2"]   # if non-empty, only these ids are injected
# extra-paths = ["~/shared-skills"]
# no-skills = false
# auto-suggest = true
# registry-urls = []         # reserved for future remote registries
```

See **[Skills guide](/docs/guide/skills)** for file format and commands.

## Key reference

### Top-level

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `default_profile` | string | `"default"` | Name of the default profile |

### `[profile.<name>]`

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `provider` | string | Yes | Provider: `anthropic`, `openai`, `openrouter`, `gemini`, `deepseek`, `mistral`, `xai`, `groq`, `togetherai`, `fireworks`, `cerebras`, `perplexity`, `minimax`, `alibabacloud`, `kimi`, `kimi-code`, `local` |
| `model` | string | Yes | Model name |
| `api_key` | string | No | Legacy fallback. The setup wizard stores keys in the credentials file instead. |
| `api_key_env` | string | No | Environment variable name for API key |
| `base_url` | string | No | Custom endpoint URL |

### `[profile.<name>.fast]`

Optional fast/cheap provider for utility tasks (title generation, summaries, commit messages, prompt enhancement). Falls back to the main profile provider when not set.

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `provider` | string | Yes | Provider identifier |
| `model` | string | Yes | Model name |
| `api_key` | string | No | Legacy fallback. The setup wizard stores keys in the credentials file instead. |
| `api_key_env` | string | No | Environment variable name for API key |
| `base_url` | string | No | Custom endpoint URL |

### `[agent]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `max-turns` | integer | `200` | Maximum turns per session |
| `max-budget-usd` | float \| omitted | none | Optional spend cap per session (USD) |
| `max-concurrent-tools` | integer | `4` | Max parallel read-only tool calls |
| `quiet` | boolean | `false` | Less verbose agent output |
| `no-rules` | boolean | `false` | Skip hierarchical rules / CLIDO injection |
| `rules-file` | string | none | Use a single rules file instead of discovery |
| `notify` | boolean | `false` | Desktop notify on turn complete (where supported) |
| `auto-checkpoint` | boolean | `true` | Checkpoint before file-mutating turns |
| `max-checkpoints-per-session` | integer | `50` | Retention cap for checkpoints |
| `max-output-tokens` | integer | none | Cap model output tokens per response |
| `harness` | boolean | `false` | Harness mode: `.clido/harness/` tasks, split `HarnessControl` tools, no `TodoWrite` |
| `max-wall-time-per-turn-sec` | integer | `900` | Wall seconds per user turn (`0` = unlimited) |
| `max-tool-calls-per-turn` | integer | `200` | Cap on individual tool invocations per user turn |
| `stall-threshold` | integer | `6` | Stall tracker score at which the turn fails |
| `doom-consecutive-same-error` | integer | `3` | Consecutive identical tool errors → doom loop |
| `doom-same-args-window` | integer | `8` | Window size for repeated identical tool+args |
| `doom-same-args-min` | integer | `4` | Minimum repeats in window to trigger doom |
| `max-tool-retries` | integer | `3` | Retries per tool call for transient failures (`tool-retries` alias) |
| `retry-backoff-max-ms` | integer | `10000` | Upper bound on backoff between retries (ms) |
| `retry-jitter-numerator` | integer | `25` | Jitter fraction: delay × numerator / 100 |
| `provider-min-request-interval-ms` | integer | `0` | Minimum gap between LLM `complete` calls (`0` = off) |

Commented examples and tuning notes: [Configuration guide](../guide/configuration.md).

### `[context]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `compaction-threshold` | float | `0.58` | Context compaction trigger fraction |
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

### `[index]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `exclude-patterns` | list | `[]` | Globs excluded from `clido index build` |
| `include-ignored` | boolean | `false` | Index ignored files (use with care) |

### `[skills]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `disabled` | list of strings | `[]` | Skill ids never injected |
| `enabled` | list of strings | `[]` | If non-empty, **only** these ids are injected |
| `extra-paths` | list of strings | `[]` | Extra directories to scan (`~/` expanded) |
| `no-skills` | boolean | `false` | Disable all skill injection |
| `auto-suggest` | boolean | `true` (if unset) | Stronger prompt text encouraging skill suggestions |
| `registry-urls` | list of strings | `[]` | Reserved for remote skill indexes (not used yet) |

### `[roles]` (legacy)

Parsed for backwards compatibility. **`[profile.<name>.fast]`** is the supported way to supply a utility model. The TUI `/fast` and `/smart` commands use built-in defaults when `[roles]` is absent.

User-level model favorites and recency live in `~/.config/clido/model_prefs.json` (`/fav`, `/models`).

## `permission_mode` values

Set via **`--permission-mode`** or **`CLIDO_PERMISSION_MODE`**. There is **no** `[agent] permission-mode` key today.

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

See [Environment Variables](/docs/reference/env-vars) for the full variable list.
