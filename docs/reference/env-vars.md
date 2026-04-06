# Environment Variables

All environment variables recognised by clido.

## API keys

| Variable | Description |
|----------|-------------|
| `ANTHROPIC_API_KEY` | Anthropic API key. Used when the active profile has `api_key_env = "ANTHROPIC_API_KEY"` or no `api_key_env` and provider is `anthropic`. |
| `OPENAI_API_KEY` | OpenAI-compatible API key. Used when provider is `openai` and `api_key_env` is not set. |
| `OPENROUTER_API_KEY` | OpenRouter API key. Used when provider is `openrouter` and `api_key_env` is not set. |

Any custom environment variable name can be set in a profile's `api_key_env` field:

```toml
[profile.my-provider]
provider    = "openai"
api_key_env = "MY_CUSTOM_KEY"
```

## Config and data paths

| Variable | Default | Description |
|----------|---------|-------------|
| `CLIDO_CONFIG` | Platform config dir + `/config.toml` | Override the config file path. Can be absolute or relative. |
| `CLIDO_DATA_DIR` | Platform data dir | Override the data directory (sessions, memory DB, audit logs). |
| `CLIDO_SESSION_DIR` | under data dir | Override **only** the session storage directory. |

## Skills

| Variable | Description |
|----------|-------------|
| `CLIDO_SKILL_PATHS` | Additional directories to scan for skill files. Use `:` separators on macOS/Linux and `;` on Windows. See [Skills](/docs/guide/skills). |

## Runtime overrides

These variables mirror CLI flags. CLI flags take precedence over environment variables.

| Variable | CLI equivalent | Description |
|----------|---------------|-------------|
| `CLIDO_MODEL` | `--model` | Model name override |
| `CLIDO_PROVIDER` | `--provider` | Provider override |
| `CLIDO_PROFILE` | `--profile` | Profile name |
| `CLIDO_WORKDIR` | `--workdir` | Working directory |
| `CLIDO_MAX_TURNS` | `--max-turns` | Maximum agent turns |
| `CLIDO_MAX_BUDGET_USD` | `--max-budget-usd` | Maximum cost in USD |
| `CLIDO_PERMISSION_MODE` | `--permission-mode` | Permission mode (`default`, `accept-all`, `plan`) |
| `CLIDO_OUTPUT_FORMAT` | `--output-format` | Output format (`text`, `json`, `stream-json`) |
| `CLIDO_INPUT_FORMAT` | `--input-format` | Input format (`text`, `stream-json`) |
| `CLIDO_MAX_PARALLEL_TOOLS` | `--max-parallel-tools` | Max concurrent tool calls |
| `CLIDO_SYSTEM_PROMPT` | `--system-prompt` | System prompt override |

## Display and UI

| Variable | Default | Description |
|----------|---------|-------------|
| `NO_COLOR` | unset | When set to any non-empty value, disables ANSI color output. Also respected by `--no-color` flag. Follows the [no-color.org](https://no-color.org) convention. |
| `TERM` | — | Used to detect terminal capabilities. clido respects `TERM=dumb` to disable color. |

## Logging

| Variable | Default | Description |
|----------|---------|-------------|
| `CLIDO_LOG` | `info` | Log filter for [tracing](https://docs.rs/tracing). Examples: `debug`, `clido_agent=debug`, `warn`. |
| `RUST_LOG` | — | Standard Rust log filter (fallback when `CLIDO_LOG` is not set). |
| `CLIDO_TRACE_METRICS` | unset | Set to `1` to emit agent-loop metric events at `tracing` **DEBUG** (target `clido::metrics`) for run, TUI, REPL, workflow steps, and `clido commit`. |

## Examples

### Set provider and model for a CI job

```bash
export ANTHROPIC_API_KEY="$SECRET_KEY"
export CLIDO_MAX_TURNS=20
export CLIDO_MAX_BUDGET_USD=0.50
export CLIDO_PERMISSION_MODE=accept-all

clido --output-format json "fix all clippy warnings"
```

### Use a custom config file

```bash
CLIDO_CONFIG=./ci/clido.toml clido "run smoke tests"
```

### Enable debug logging

```bash
CLIDO_LOG=debug clido "test prompt" 2>debug.log
```

### Disable color in scripts

```bash
NO_COLOR=1 clido --output-format json "list files"
```
