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

```sh
curl -fsSL https://raw.githubusercontent.com/0xkurt/clido/master/scripts/install.sh | sh
```

Or build from source (requires [Rust 1.94+](https://rustup.rs)):

```sh
git clone https://github.com/0xkurt/clido.git && cd clido
cargo install --path crates/clido-cli --locked
```

### First-run setup

On first launch, clido will run an interactive wizard (`clido init`) that:

1. Prompts for your preferred provider (anthropic, openrouter, openai, mistral, minimax, alibabacloud, local)
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

### Profile structure

Each profile defines a provider, model, and optional credentials:

```toml
[profiles.myprofile]
provider = "anthropic"        # required — see Supported Providers below
model = "claude-sonnet-4-5"   # required — model identifier
api_key = "sk-..."            # optional — inline key (env var preferred)
api_key_env = "MY_KEY"        # optional — name of env var holding the key
base_url = "https://..."      # optional — override API endpoint

# Optional fast/cheap provider for utility tasks (titles, summaries, etc.)
[profiles.myprofile.fast]
provider = "openai"
model = "gpt-4o-mini"
```

**Managing profiles:**

```sh
clido profile list               # list all profiles
clido profile create myprofile   # create via guided wizard
clido profile switch myprofile   # set as default
clido profile edit myprofile     # edit via guided wizard
clido profile delete myprofile   # delete a profile
```

In the TUI, press **Ctrl+P** to open the profile picker or use `/profile` slash commands.

### Environment variables

| Variable | Description |
|---|---|
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI / OpenRouter API key |
| `OPENROUTER_API_KEY` | OpenRouter API key |
| `DASHSCOPE_API_KEY` | Alibaba Cloud (DashScope / Qwen) API key |
| `MINIMAX_API_KEY` | MiniMax API key |
| `CLIDO_PROFILE` | Active profile name |
| `CLIDO_MODEL` | Model override |
| `CLIDO_PROVIDER` | Provider override |
| `CLIDO_MAX_TURNS` | Max agent turns |
| `CLIDO_MAX_BUDGET_USD` | Spend limit in USD |
| `CLIDO_PERMISSION_MODE` | `default`, `accept-all`, or `plan` |
| `CLIDO_OUTPUT_FORMAT` | `text`, `json`, or `stream-json` |
| `CLIDO_INPUT_FORMAT` | `text` (stream-json reserved for V2) |
| `CLIDO_WORKDIR` | Working directory override |
| `CLIDO_MAX_PARALLEL_TOOLS` | Max parallel read-only tool calls |
| `CLIDO_SYSTEM_PROMPT` | System prompt override |
| `CLIDO_DATA_DIR` | Override data directory (sessions, index, audit) |
| `CLIDO_SESSION_DIR` | Override session storage directory |

## Supported Providers

| Provider | ID | Default Model | API Key Env | Notes |
|---|---|---|---|---|
| Anthropic | `anthropic` | claude-sonnet-4-5 | `ANTHROPIC_API_KEY` | Native SDK |
| OpenAI | `openai` | gpt-4o | `OPENAI_API_KEY` | |
| OpenRouter | `openrouter` | anthropic/claude-sonnet-4-5 | `OPENROUTER_API_KEY` | Multi-provider gateway |
| Google Gemini | `gemini` | gemini-2.5-flash | `GEMINI_API_KEY` | |
| DeepSeek | `deepseek` | deepseek-chat | `DEEPSEEK_API_KEY` | |
| Mistral | `mistral` | mistral-large-latest | `MISTRAL_API_KEY` | |
| xAI (Grok) | `xai` | grok-3-beta | `XAI_API_KEY` | |
| Groq | `groq` | llama-3.3-70b-versatile | `GROQ_API_KEY` | |
| Together AI | `togetherai` | meta-llama/Llama-3.3-70B-Instruct-Turbo | `TOGETHER_API_KEY` | |
| Fireworks AI | `fireworks` | llama-v3p3-70b-instruct | `FIREWORKS_API_KEY` | |
| Cerebras | `cerebras` | llama3.1-70b | `CEREBRAS_API_KEY` | |
| Perplexity | `perplexity` | sonar-pro | `PERPLEXITY_API_KEY` | |
| MiniMax | `minimax` | MiniMax-M1 | `MINIMAX_API_KEY` | |
| Alibaba Cloud | `alibabacloud` | qwen-max | `DASHSCOPE_API_KEY` | DashScope / Qwen |
| Kimi (Moonshot) | `kimi` | moonshot-v1-8k | `MOONSHOT_API_KEY` | |
| Kimi Code | `kimi-code` | kimi-for-coding | `KIMI_CODE_API_KEY` | |
| Local / Ollama | `local` | llama3.2 | *(none needed)* | `http://localhost:11434` |

All providers except Anthropic use the OpenAI-compatible API format. Use `clido list-models --provider <id>` to see available models for a provider.

**Model aliases:** `sonnet` → claude-sonnet-4-5, `opus` → claude-opus-4-6, `haiku` → claude-haiku-4-5, `4o` → gpt-4o, `flash` → gemini-2.5-flash, `deepseek` → deepseek-chat, `r1` → deepseek-reasoner, `grok` → grok-3-beta, `sonar` → sonar-pro.

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
| `profile list` | List all profiles with active model per slot |
| `profile create [NAME]` | Create a new profile via guided wizard |
| `profile switch <NAME>` | Switch the active (default) profile |
| `profile edit <NAME>` | Edit a profile via guided wizard |
| `profile delete <NAME>` | Delete a profile |
| `memory list` | List stored memories |
| `memory prune` | Prune old memories |
| `memory reset` | Delete all memories |
| `index build` | Build the repository index |
| `index stats` | Show index statistics |
| `index clear` | Clear the index |
| `checkpoint` | Manage session checkpoints |
| `rollback [ID]` | Restore to a checkpoint |
| `plan` | Manage task plans |
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

Type `/` in the input bar to see completions. Commands are grouped by category:

**Session**

| Command | Description |
|---|---|
| `/clear` | Clear the conversation |
| `/help` | Show key bindings and all slash commands |
| `/keys` | Show keyboard shortcuts overlay |
| `/quit` | Exit clido |
| `/session` | Show current session ID |
| `/sessions` | Open session picker (list and resume recent sessions) |
| `/search <query>` | Search conversation history |
| `/export` | Save this conversation to a markdown file |
| `/init` | Reconfigure the current profile (opens in-TUI editor) |

**Model & Roles**

| Command | Description |
|---|---|
| `/model [name]` | Show or switch the active model |
| `/models` | Open interactive model picker (search, filter, favorites, pricing) |
| `/fast` | Switch to fast (cheap) model (respects `[roles] fast` in config) |
| `/smart` | Switch to smart (powerful) model (respects `[roles] reasoning` in config) |
| `/fav` | Toggle the current model as a favorite |
| `/reviewer [on\|off]` | Show or toggle reviewer sub-agent |

**Settings**

| Command | Description |
|---|---|
| `/config` | Show all settings — provider, model, roles, agent, context |
| `/configure <intent>` | Change settings with natural language |
| `/settings` | Open settings editor (roles, default model) |
| `/prompt-mode [auto\|off\|status]` | Show or set prompt enhancement mode |
| `/prompt-preview` | Preview the enhanced version of your next message |
| `/prompt-rules [list\|add\|remove\|reset]` | Manage prompt enhancement rules |

**Git**

| Command | Description |
|---|---|
| `/ship [msg]` | Stage all changes, commit, and push |
| `/save [msg]` | Stage all changes and commit locally |
| `/pr [title]` | Create a pull request |
| `/branch <name>` | Create and switch to a new branch |
| `/sync` | Pull --rebase from upstream, resolve conflicts if needed |
| `/undo` | Undo the last commit safely (asks for confirmation) |
| `/rollback [id]` | Restore to a checkpoint or commit |

**Context & Cost**

| Command | Description |
|---|---|
| `/cost` | Show session cost so far |
| `/tokens` | Show input and output token usage |
| `/compact` | Compact the context window immediately (summarizes history) |
| `/memory [query]` | Search long-term memory |
| `/todo` | Show the agent's current task list |

**Plan**

| Command | Description |
|---|---|
| `/plan [task]` | Show current plan, or plan a task first |
| `/plan edit` | Open plan editor for the current plan |
| `/plan save` | Save current plan to `.clido/plans/` |
| `/plan list` | List all saved plans |

**Project**

| Command | Description |
|---|---|
| `/agents` | Show current agent configuration (main + fast provider) |
| `/profiles` | List all profiles with active model per slot |
| `/profile [name]` | Open profile picker — switch, create, or edit |
| `/profile new` | Create a new profile via the guided wizard |
| `/profile edit [name]` | Edit a profile in the TUI |
| `/check` | Run diagnostics on current project |
| `/rules` | Show active CLIDO.md rules files |
| `/image <path>` | Attach an image to the next message |
| `/workdir [path]` | Show or set working directory |
| `/stop` | Interrupt current run without sending a message |
| `/copy [all\|n]` | Copy last assistant message (or all / nth) to clipboard |
| `/notify [on\|off]` | Toggle desktop notifications |
| `/index` | Show codebase index stats |

### Keybindings

**Global**

| Key | Action |
|---|---|
| `Ctrl+C` / `Ctrl+D` | Quit |
| `Ctrl+/` | Interrupt current agent run |
| `Ctrl+Y` | Copy last assistant response to clipboard |
| `Ctrl+M` | Open model picker |
| `Ctrl+P` | Open profile picker |
| `Ctrl+K` | Open keyboard shortcuts overlay |
| `Ctrl+L` | Refresh screen |

**Chat input**

| Key | Action |
|---|---|
| `Enter` | Send message |
| `Shift+Enter` | Insert newline (multiline input) |
| `Ctrl+Enter` | Interrupt current run and send immediately |
| `Esc` | Clear input field |
| `↑` / `↓` | Browse input history (single-line) or navigate lines (multiline) |
| `Ctrl+U` | Clear entire input |
| `Ctrl+W` / `Ctrl+Backspace` | Delete word backward |
| `Alt+Left` / `Alt+Right` | Jump by word |
| `Home` / `End` | Start / end of line |

**Scrolling**

| Key | Action |
|---|---|
| `Ctrl+Home` | Jump to top of chat |
| `Ctrl+End` | Jump to bottom of chat (follow mode) |
| `PageUp` / `PageDown` | Scroll chat by page |
| `↑` / `↓` | Scroll chat (when input is empty) |

**Pickers & overlays**

| Key | Action |
|---|---|
| `↑` / `↓` | Navigate list |
| `Enter` | Select / confirm |
| `Esc` | Close overlay |
| Type to filter | Narrows results in model/session/profile pickers |
| `f` | Toggle favorite (model picker) |
| `Ctrl+S` | Save as default (model picker) |
| `n` / `e` | New / edit (profile picker) |
| `d` | Delete (session picker) |

**Permission prompts**

| Key | Action |
|---|---|
| `1`–`5` | Quick-select: Once / Session / Workdir / Deny / Deny+feedback |
| `Enter` | Confirm selected option |
| `Esc` | Deny and close |

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

See [Building](docs/developer/building.md) for build/test commands and [Contributing](docs/developer/contributing.md) for contributor workflow.

## Status

**V1+ implementation:** Core agent loop, six tools, config with profiles, sessions with resume and stale-file detection, context compaction, permission modes, `clido doctor` and `clido init`, interactive TUI (`clido` with no args at a TTY), first-run setup, memory, repo index, declarative workflows, audit log, stats, shell completions, man page, list-models, planner (experimental), MCP support, agent profiles (create/switch/edit/delete with optional fast provider), checkpoints and rollback, and multi-provider support including Anthropic, OpenAI, OpenRouter, Mistral, MiniMax, Alibaba Cloud, and local (Ollama). Build and test: see **Build** above.

## Documentation

| Doc | Description |
| --- | --- |
| [Architecture](docs/developer/architecture.md) | Runtime architecture and component boundaries |
| [CLI reference](docs/reference/cli.md) | Canonical command surface and behavior |
| [Flags reference](docs/reference/flags.md) | Global flags and semantics |
| [Slash commands](docs/reference/slash-commands.md) | TUI slash command catalog |
| [Configuration reference](docs/reference/config.md) | `config.toml` and profile schema |
| [Environment variables](docs/reference/env-vars.md) | Runtime env var overrides |
| [Output formats](docs/reference/output-formats.md) | Text, JSON, and stream output contracts |
| [Key bindings](docs/reference/key-bindings.md) | TUI interaction model |
| [Workflows guide](docs/guide/workflows.md) | Declarative workflow authoring and execution |
| [Planner guide](docs/guide/planner.md) | Planner behavior, review, and execution |
| [MCP guide](docs/guide/mcp.md) | MCP server configuration and usage |
| [Contributing](docs/developer/contributing.md) | Project conventions and contributor workflow |

## License

[MIT](LICENSE)
