# Configuration

clido is configured through a TOML file, environment variables, and command-line flags. Values are merged in that order: config file is the baseline, environment variables override config, and flags override everything.

## Config file location

The config file is looked up in this order:

1. `$CLIDO_CONFIG` (environment variable), if set
2. Global config: `~/.config/clido/config.toml` (Linux/macOS)
3. Project config: `.clido/config.toml` in the current directory or any parent up to `$HOME`

Global and project configs are merged: project values override global values. This lets you set per-repository models or providers without changing your global config.

::: tip macOS config path
On macOS the platform config directory is `~/Library/Application Support/clido/config.toml`. Using `~/.config/clido/config.toml` also works if it exists.
:::

## A complete annotated config.toml

```toml
# Which profile to use when no --profile flag is given.
default_profile = "default"

# ── Profiles ────────────────────────────────────────────────────────────────
# Each profile defines a provider, model, and credentials.
# You can have as many profiles as you like and switch with --profile.

[profile.default]
provider    = "anthropic"
model       = "claude-sonnet-4-5"
# Store the key in an environment variable (recommended)
api_key_env = "ANTHROPIC_API_KEY"
# Keys are stored in the credentials file by the setup wizard.
# You can also set api_key directly (legacy, not recommended).

[profile.fast]
provider    = "anthropic"
model       = "claude-haiku-4-5"
api_key_env = "ANTHROPIC_API_KEY"

[profile.openrouter]
provider    = "openrouter"
model       = "anthropic/claude-3-5-sonnet"
api_key_env = "OPENROUTER_API_KEY"

[profile.local]
provider  = "local"
model     = "llama3.2"
base_url  = "http://localhost:11434"

# ── Agent settings ───────────────────────────────────────────────────────────

[agent]
max-turns       = 200      # Maximum agent turns per session (default in clido-core: 200).
max-budget-usd  = 5.0      # Maximum spend per session in USD (optional; omit for no cap).
max-concurrent-tools = 4   # Max parallel read-only tool calls. Default: 4.
# stream-model-completion = false   # Stream + aggregate when the provider supports it.
# If true, malformed streamed tool JSON fails the turn (no silent empty `{}` args).
# tool-timeout-secs = 60            # Wall-clock cap per tool invocation (agent loop).
# max-tool-output-bytes = 512000    # Truncate tool output beyond this size (0 = off).

# ── Production agent loop (optional overrides; defaults are safe) ─────────────
# Wall-clock cap for one user message / completion loop (0 = unlimited). Default: 900 s.
# max-wall-time-per-turn-sec = 900
# Hard cap on individual tool invocations per user turn. Default: 200.
# max-tool-calls-per-turn = 200
# Stall heuristic threshold (repeated all-error batches / identical tool batches). Default: 6.
# stall-threshold = 6
# Doom loop: consecutive identical normalized errors. Default: 3.
# doom-consecutive-same-error = 3
# Doom loop: sliding window size and min repeats of same (tool, args). Defaults: 8 / 4.
# doom-same-args-window = 8
# doom-same-args-min = 4
# Auto-retries for transient tool failures. Default: 3. Alias: tool-retries.
# max-tool-retries = 3
# retry-backoff-max-ms = 10000
# retry-jitter-numerator = 25   # fraction of delay used for spread (0–100)
# Minimum gap between provider completion calls (ms). 0 = off.
# provider-min-request-interval-ms = 0

# ── Context settings ─────────────────────────────────────────────────────────

[context]
compaction-threshold = 0.58   # Compact when usage exceeds this fraction of context (default ~0.58).
max-context-tokens   = 180000 # Optional: override max context window.

# ── Tools ────────────────────────────────────────────────────────────────────

[tools]
# Allow only a specific subset of tools.
allowed    = []          # Empty = all tools allowed.
# Always deny these tools regardless of allowed list.
disallowed = ["Bash"]    # e.g. prevent shell execution.

# ── Workflows ────────────────────────────────────────────────────────────────

[workflows]
directory = ".clido/workflows"   # Default workflow search path. Default: .clido/workflows.

# ── Hooks ────────────────────────────────────────────────────────────────────
# Shell commands run before and after each tool call.
# Available environment variables:
#   pre_tool_use:  CLIDO_TOOL_NAME, CLIDO_TOOL_INPUT
#   post_tool_use: CLIDO_TOOL_NAME, CLIDO_TOOL_INPUT, CLIDO_TOOL_OUTPUT,
#                  CLIDO_TOOL_IS_ERROR, CLIDO_TOOL_DURATION_MS

[hooks]
pre_tool_use  = ""   # e.g. "echo Tool $CLIDO_TOOL_NAME >> ~/clido-hooks.log"
post_tool_use = ""   # e.g. "notify-send clido $CLIDO_TOOL_NAME"

# ── Skills (optional) ─────────────────────────────────────────────────────────
# Reusable agent instructions from .md/.txt under .clido/skills/ and ~/.clido/skills/

[skills]
# disabled = ["some-id"]
# enabled = ["only-these"]   # if non-empty, whitelist mode
# extra-paths = ["~/shared-skills"]
# no-skills = false
# auto-suggest = true
# registry-urls = []         # reserved for future remote discovery
```

See **[Skills](/docs/guide/skills)** for file format, CLI, and TUI commands.

## Viewing the current config

```bash
clido config show
```

This prints the merged config (global + project) with values that came from environment variables shown with an asterisk.

## Setting config values

Use `clido config set` to update the global config file:

```bash
# Change the default model
clido config set model claude-3-opus-20240229

# Change the provider for the default profile
clido config set provider openrouter

# Set the API key for the default profile
clido config set api-key sk-ant-api03-...
```

::: warning Direct file edits
`clido config set` modifies the global config file at `~/.config/clido/config.toml`. It only supports the keys listed above. To set other values (e.g. `max-turns`, hooks), edit the file directly.
:::

## Environment variable overrides

All config values can be overridden at runtime via environment variables. They take precedence over the config file:

| Variable | Overrides |
|----------|-----------|
| `CLIDO_MODEL` | `--model` / profile model |
| `CLIDO_PROVIDER` | `--provider` / profile provider |
| `CLIDO_PROFILE` | `--profile` |
| `CLIDO_MAX_TURNS` | `--max-turns` |
| `CLIDO_MAX_BUDGET_USD` | `--max-budget-usd` |
| `CLIDO_MAX_PARALLEL_TOOLS` | `--max-parallel-tools` |
| `CLIDO_PERMISSION_MODE` | `--permission-mode` |
| `CLIDO_OUTPUT_FORMAT` | `--output-format` |
| `CLIDO_INPUT_FORMAT` | `--input-format` |
| `CLIDO_WORKDIR` | `--workdir` |
| `CLIDO_SYSTEM_PROMPT` | `--system-prompt` |
| `CLIDO_CONFIG` | Config file path |
| `ANTHROPIC_API_KEY` | Anthropic credentials |
| `OPENAI_API_KEY` | OpenAI-compatible credentials |
| `OPENROUTER_API_KEY` | OpenRouter credentials |

See [Environment Variables](/docs/reference/env-vars) for the full list.

## Profiles

A profile is a named set of provider credentials and model settings. Use profiles to switch between providers or models without rewriting config.

```toml
default_profile = "default"

[profile.default]
provider    = "anthropic"
model       = "claude-sonnet-4-5"
api_key_env = "ANTHROPIC_API_KEY"

[profile.cheap]
provider    = "anthropic"
model       = "claude-haiku-4-5"
api_key_env = "ANTHROPIC_API_KEY"
```

Switch profiles at runtime:

```bash
clido --profile cheap "quick question"
CLIDO_PROFILE=cheap clido "quick question"
```

## Per-project config

Create `.clido/config.toml` in your project root to override the global config for that project. Only the keys you specify are overridden; everything else falls back to the global config.

```toml
# .clido/config.toml in a Rust project
default_profile = "default"

[profile.default]
provider = "anthropic"
model    = "claude-sonnet-4-5"
api_key_env = "ANTHROPIC_API_KEY"

[agent]
max-turns = 20

[tools]
disallowed = []
```

::: tip Project config in source control
Project `.clido/config.toml` files are safe to commit — API keys are stored in the separate `credentials` file, not in config.toml. If you do use the legacy `api_key` field, use `api_key_env` to reference an environment variable instead.
:::
