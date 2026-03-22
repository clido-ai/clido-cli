# cli;do

<p align="center">
  <img src="https://merbeth.io/files/clido.svg" width="420" height="140" alt="cli;do logo">
</p>

**cli;do** is a local-first, multi-provider CLI coding agent. Run it in your terminal, give it a task in plain language, and it uses AI (with tools like read, edit, search, and run) to get the job done—with permission prompts for anything that changes your files.

## Vision

- **CLI-first** — Built for the terminal; scripting and automation are first-class.
- **Multi-provider** — Use different AI backends (e.g. Anthropic, OpenAI) via profiles.
- **Safe by default** — Destructive or state-changing actions require your approval.
- **Session-aware** — Resume after interrupt; cost and usage visible when you care.

Planned capabilities include: core agent loop with tools, sessions, context and permissions (V1); JSON output and operator tooling (V1.5); multi-provider, sandboxing, packaging (V2); memory, MCP, semantic search, declarative workflows (V3); optional task-graph planner (V4).

## Installation

**Prerequisites:** [Rust](https://rustup.rs) (install via `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`).

```sh
# Build and install from source
cargo install --path crates/clido-cli

# Or build a release binary manually
cargo build --release
# binary at: target/release/clido
```

### First-run setup

On first launch, clido will run an interactive wizard (`clido init`) that:

1. Prompts for your preferred provider (anthropic, openrouter, alibabacloud, local)
2. Asks for your API key or base URL
3. Writes `~/.config/clido/config.toml`

You can also run `clido init` at any time to reconfigure.

## Configuration

Config is stored in `~/.config/clido/config.toml` (global) or `.clido/config.toml` (project-local, takes precedence).

```toml
default_profile = "default"

[profiles.default]
provider = "anthropic"
model = "claude-3-5-sonnet-20241022"

[profiles.fast]
provider = "anthropic"
model = "claude-3-5-haiku-20241022"

[profiles.local]
provider = "local"
model = "llama3"
base_url = "http://localhost:11434"
```

Switch profiles with `--profile fast` or `CLIDO_PROFILE=fast`.

### Environment variables

| Variable | Description |
|---|---|
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI / OpenRouter API key |
| `OPENROUTER_API_KEY` | OpenRouter API key |
| `DASHSCOPE_API_KEY` | Alibaba Cloud (DashScope / Qwen) API key |
| `CLIDO_PROFILE` | Active profile name |
| `CLIDO_MODEL` | Model override |
| `CLIDO_PROVIDER` | Provider override |
| `CLIDO_MAX_TURNS` | Max agent turns (default: 10) |
| `CLIDO_MAX_BUDGET_USD` | Spend limit in USD |
| `CLIDO_PERMISSION_MODE` | `default`, `accept-all`, or `plan` |
| `CLIDO_OUTPUT_FORMAT` | `text`, `json`, or `stream-json` |
| `CLIDO_INPUT_FORMAT` | `text` or `stream-json` |
| `CLIDO_WORKDIR` | Working directory override |
| `CLIDO_MAX_PARALLEL_TOOLS` | Max parallel read-only tool calls |
| `CLIDO_SYSTEM_PROMPT` | System prompt override |

## CLI flags

```
clido [FLAGS] [OPTIONS] [PROMPT]...
clido <SUBCOMMAND>
```

### Flags

| Flag | Description |
|---|---|
| `-p, --print` | Non-interactive: no REPL, no permission prompts |
| `-q, --quiet` | Suppress spinner, tool output, and cost footer |
| `-v, --verbose` | Verbose logging |
| `--no-color` | Disable color (also respects `NO_COLOR`) |
| `--sandbox` | Enable Bash sandboxing (macOS `sandbox-exec` / Linux `bwrap`) |
| `--resume-ignore-stale` | Skip stale-file check when resuming a session |
| `--continue` | Continue the most recent session for this project |

### Options

| Option | Description |
|---|---|
| `--profile <NAME>` | Profile from config (env: `CLIDO_PROFILE`) |
| `--model <MODEL>` | Model override (env: `CLIDO_MODEL`) |
| `--provider <PROV>` | Provider override (env: `CLIDO_PROVIDER`) |
| `--max-turns <N>` | Max agent turns, default 10 (env: `CLIDO_MAX_TURNS`) |
| `--max-budget-usd <USD>` | Spend limit (env: `CLIDO_MAX_BUDGET_USD`) |
| `--permission-mode <MODE>` | `default`, `accept-all`, or `plan` |
| `--system-prompt <TEXT>` | Override system prompt |
| `--system-prompt-file <PATH>` | Load system prompt from file |
| `--append-system-prompt <TEXT>` | Append to system prompt |
| `--allowed-tools <LIST>` | Comma-separated allowed tools |
| `--disallowed-tools <LIST>` | Comma-separated disallowed tools |
| `--tools <LIST>` | Alias for `--allowed-tools` |
| `--output-format <FMT>` | `text`, `json`, or `stream-json` |
| `--input-format <FMT>` | `text` or `stream-json` (for SDK/subprocess) |
| `--resume <SESSION_ID>` | Resume a specific session |
| `-C, --workdir <PATH>` | Working directory (env: `CLIDO_WORKDIR`) |
| `--max-parallel-tools <N>` | Max parallel read-only tool calls |
| `--mcp-config <PATH>` | MCP config file path |

### Subcommands

| Subcommand | Description |
|---|---|
| `init` | Run first-run setup wizard |
| `run <PROMPT>` | Explicit run subcommand (scriptable) |
| `doctor` | Check environment, API key, config, and tool health |
| `sessions list` | List recent sessions |
| `sessions show <ID>` | Show a session |
| `sessions fork <ID>` | Fork a session to a new ID |
| `memory list` | List stored memories |
| `memory prune` | Prune old memories |
| `memory reset` | Delete all memories |
| `index build` | Build the repository index |
| `index stats` | Show index statistics |
| `index clear` | Clear the index |
| `workflow run <FILE>` | Run a declarative workflow |
| `workflow validate <FILE>` | Validate workflow YAML |
| `workflow inspect <FILE>` | List workflow steps and dependencies |
| `workflow list` | List workflows in configured directories |
| `workflow check <FILE>` | Run preflight checks on a workflow |
| `audit` | Show audit log |
| `audit --tail <N>` | Show last N audit entries |
| `audit --session <ID>` | Filter by session |
| `audit --tool <NAME>` | Filter by tool |
| `audit --since <TS>` | Filter by timestamp (ISO 8601) |
| `audit --json` | JSON output |
| `stats` | Show session statistics |
| `stats --session <ID>` | Show stats for a session |
| `stats --json` | JSON output |
| `completions <SHELL>` | Print shell completions (bash/zsh/fish/powershell/elvish) |
| `man` | Print man page |
| `config show` | Show resolved config |
| `config set <KEY> <VAL>` | Set a config value (model, provider, api-key) |
| `list-models` | List available models by provider |
| `list-models --provider <P>` | Filter by provider |
| `update-pricing` | Update model pricing data from remote |
| `fetch-models` | Fetch model list from a provider's API |
| `version` | Print version |

## Interactive TUI

Run `clido` (no arguments, at a TTY) to launch the full-screen interactive TUI. The TUI shows:
- A scrollable conversation panel with assistant responses, tool calls, and diffs.
- A header strip with provider, model, session ID, cost, and context window usage (% filled).
- A status strip with live tool activity.
- A hint bar with key bindings.

### Slash commands

Type `/` in the input bar to see completions. Available commands:

| Command | Description |
|---|---|
| `/clear` | Clear the conversation |
| `/help` | Show all key bindings and slash commands (grouped by category) |
| `/session` | Show current session ID |
| `/sessions` | Open session picker (list and resume recent sessions) |
| `/quit` | Exit |
| `/model [name]` | Show or switch the active model |
| `/models` | Open interactive model picker (filter, favorites, pricing) |
| `/fast` | Switch to the fast model (respects `[roles] fast` in config) |
| `/smart` | Switch to the smart model (respects `[roles] reasoning` in config) |
| `/role <name>` | Switch to the model assigned to a named role |
| `/fav` | Toggle the current model as a favorite |
| `/cost` | Show session cost so far |
| `/tokens` | Show input and output token usage |
| `/compact` | Compact the context window immediately |
| `/memory <query>` | Search long-term memory |
| `/plan` | Show current task plan (when `--planner` is active) |
| `/ship [msg]` | Stage all changes, commit, and push |
| `/save [msg]` | Stage all changes and commit locally |
| `/undo` | Undo the last committed change |
| `/index` | Show repo index status |
| `/rules` | Show active CLIDO.md rules files |
| `/image <path>` | Attach an image to the next message |

**Key bindings:** `Enter` send · `Ctrl+Enter` interrupt & send · `↑↓` history · `PgUp/PgDn` scroll · `Ctrl+U` clear input · `Ctrl+C` quit.

## MCP (Model Context Protocol)

Pass a JSON or YAML config file to connect external MCP tool servers:

```sh
clido --mcp-config ./mcp.json "use the file-system server to list project files"
```

```json
{
  "servers": [
    { "name": "fs", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] }
  ]
}
```

Each server entry requires `name` and `command`; `args` and `env` are optional. MCP tools appear alongside built-in tools.

## Memory

Clido stores long-term memories automatically during agent sessions. The agent reads relevant memories from the previous N sessions and can create new ones.

```sh
clido memory list              # show recent memories (default: 20)
clido memory list --limit 50   # show more
clido memory prune --keep 100  # keep only the 100 most recent
clido memory reset --force     # delete all memories
```

Memories are injected into the system prompt context automatically — no manual configuration needed.

## Repository Index

Build a file and symbol index so the agent can use `SemanticSearch` to find relevant code:

```sh
clido index build                            # index current directory (rs,py,js,ts,go)
clido index build --dir ./src --ext rs,toml  # custom directory and extensions
clido index stats                            # show index stats
clido index clear                            # delete the index
```

Once the index is built, the agent automatically uses `SemanticSearch` for relevant queries.

## Workflows

Run multi-step declarative YAML workflows:

```sh
clido workflow run ./my-workflow.yaml
clido workflow run ./my-workflow.yaml --input key=value
clido workflow run ./my-workflow.yaml --dry-run   # validate without API calls
clido workflow validate ./my-workflow.yaml        # check YAML structure
clido workflow list                               # list workflows in configured dirs
```

Example workflow YAML:

```yaml
name: summarize-and-test
steps:
  - id: summarize
    prompt: "Summarize the changes in the last git commit"
  - id: test
    prompt: "Run the test suite and report failures"
    depends_on: [summarize]
```

## Planner (experimental)

Pass `--planner` to enable task decomposition before execution:

```sh
clido --planner "refactor the auth module, add tests, and update docs"
```

The planner decomposes the prompt into a DAG of subtasks shown in the TUI. On plan failure or low-quality output, it falls back to the standard reactive agent loop transparently. Use `/plan` in the TUI to review the current plan.

## Audit Log

Every tool call is recorded in the audit log:

```sh
clido audit                        # show all entries
clido audit --tail 20              # show last 20 entries
clido audit --session <ID>         # filter by session
clido audit --tool Bash            # filter by tool name
clido audit --since 2026-01-01     # filter by date
clido audit --json                 # JSON output
```

## Build

```sh
cargo build --release             # build
cargo run --release               # interactive TUI
cargo run --release -- "task"     # one-shot run
cargo test --workspace            # run all tests
cargo bench -p clido-cli          # run startup benchmarks
```

See [Local development and testing](devdocs/guides/local-development-testing.md) for environment setup (API keys, config) and [Implementation bootstrap](devdocs/guides/implementation-bootstrap.md) for contributor workflow.

## Status

**V1+ implementation:** Core agent loop, six tools, config with profiles, sessions with resume and stale-file detection, context compaction, permission modes, `clido doctor` and `clido init`, interactive TUI (`clido` with no args at a TTY), first-run setup, memory, repo index, declarative workflows, audit log, stats, shell completions, man page, list-models, planner (experimental), and MCP support. Build and test: see **Build** above.

## Documentation

| Doc | Description |
| --- | --- |
| [Implementation bootstrap](devdocs/guides/implementation-bootstrap.md) | Where to start, canonical doc order, locked pre-build decisions |
| [Development plan](devdocs/plans/development-plan.md) | Architecture, Rust workspace, phased roadmap |
| [CLI interface spec](devdocs/plans/cli-interface-specification.md) | Canonical command surface and behavior |
| [UX requirements](devdocs/plans/ux-requirements.md) | Interactive copy, script intros, visual design (functional and clear) |
| [Releases](devdocs/plans/releases/README.md) | V1 → V4 scope and exit criteria |
| [Config reference](devdocs/schemas/config.md) | `config.toml`, `.clido/config.toml`, and `pricing.toml` schema |
| [Testing strategy](devdocs/guides/testing-strategy-and-master-test-plan.md) | Full validation strategy and test taxonomy |

## License

[MIT](LICENSE)
