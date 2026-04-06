# CLI Reference

Complete reference for all clido commands and subcommands.

## Synopsis

```
clido [OPTIONS] [PROMPT]
clido <SUBCOMMAND> [OPTIONS]
```

When invoked with a prompt (and no subcommand), clido runs the agent. When invoked with no arguments from a TTY, it opens the interactive TUI.

---

## `clido [PROMPT]`

Run the agent with a prompt.

```bash
clido "refactor the auth module to use JWT"
clido run "refactor the auth module to use JWT"   # equivalent
```

When stdin is not a TTY and no prompt is given, stdin is read as the prompt.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--model` | string | profile default | Model override |
| `--provider` | string | profile default | Provider override |
| `--profile` | string | `default_profile` from config | Profile name |
| `--workdir`, `-C` | path | current directory | Working directory |
| `--max-turns` | integer | from config (default **200**) | Maximum agent turns |
| `--max-budget-usd` | float | from config (optional) | Maximum cost in USD |
| `--permission-mode` | enum | `default` | `default`, `accept-all`, or `plan` |
| `--output-format` | enum | `text` | `text`, `json`, `stream-json` |
| `--input-format` | enum | `text` | `text`, `stream-json` |
| `--quiet`, `-q` | flag | false | Suppress spinner and cost footer |
| `--print`, `-p` | flag | false | Non-interactive (no REPL) |
| `--continue` | flag | false | Resume newest session for current dir |
| `--resume` | string | — | Resume session by ID prefix |
| `--resume-ignore-stale` | flag | false | Skip stale file check on resume |
| `--mcp-config` | path | — | Path to MCP server config file |
| `--sandbox` | flag | false | Enable Bash sandboxing |
| `--planner` | flag | false | Enable task decomposition planner |
| `--max-parallel-tools` | integer | 4 | Reserved (ignored for in-turn tool batching) |
| `--system-prompt` | string | — | System prompt override |
| `--system-prompt-file` | path | — | Read system prompt from file |
| `--append-system-prompt` | string | — | Append to default system prompt |
| `--allowed-tools` | string | — | Comma-separated allowed tool names |
| `--disallowed-tools` | string | — | Comma-separated disallowed tool names |
| `--tools` | string | — | Alias for `--allowed-tools` |
| `--no-color` | flag | false | Disable ANSI color output |
| `--verbose`, `-v` | flag | false | Enable debug logging |

**Exit codes:** `0` success, `1` agent error, `2` config/usage error, `3` soft limit (turns or budget), `130` interrupted.

**Examples:**

```bash
# Basic
clido "add error handling to all database functions"

# Non-interactive with JSON output
clido --print --output-format json "summarise this codebase"

# Resume last session, read-only
clido --continue --permission-mode plan

# Use a fast cheap model for a quick task
clido --profile fast "what does this function return?"

# CI usage with budget cap
clido --permission-mode accept-all --max-budget-usd 0.10 "fix linting errors"
```

---

## `clido run <PROMPT>`

Explicit run subcommand. Identical to passing a positional prompt but avoids ambiguity in scripts.

```bash
clido run "generate tests for src/parser.rs"
```

Accepts all the same flags as the root command.

---

## `clido init`

Interactive first-run setup wizard. Prompts for provider, API key, and model, then writes `~/.config/clido/config.toml`.

```bash
clido init
```

Safe to re-run — asks for confirmation before overwriting an existing config.

**Exit codes:** `0` success, `1` user cancelled.

---

## `clido doctor`

Check environment health: binary version, API key, config file, session directory, and shell availability.

```bash
clido doctor
```

Prints a line per check with `✓` (pass) or `✗` (fail) and a hint for each failure.

**Exit codes:** `0` all checks pass, `1` one or more checks failed.

---

## `clido config show`

Print the current merged configuration (global + project) in TOML format.

```bash
clido config show
```

---

## `clido config set <KEY> <VALUE>`

Update a key in the global config file.

```bash
clido config set model claude-haiku-4-5
clido config set provider openrouter
clido config set api-key sk-ant-...
```

Supported keys: `model`, `provider`, `api-key`.

**Exit codes:** `0` success, `2` unknown key.

---

## `clido sessions list`

List all sessions for all projects, most recent first.

```bash
clido sessions list
```

```
ID        DATE        PROJECT                  PREVIEW                        COST
a1b2c3    2026-03-21  ~/projects/app           "Refactor the parser"          $0.023
```

---

## `clido sessions show <ID>`

Print the contents of a session in human-readable form.

```bash
clido sessions show a1b2c3
```

---

## `clido sessions fork <ID>`

Copy a session to a new session ID.

```bash
clido sessions fork a1b2c3
# Forked a1b2c3 → new session f9e8d7
```

---

## `clido stats`

Show session statistics: total sessions, total cost, average cost, total turns.

```bash
clido stats
clido stats --session a1b2c3   # single session
clido stats --json             # JSON output
```

| Flag | Description |
|------|-------------|
| `--session <id>` | Filter to a specific session ID prefix |
| `--json` | Output as JSON |

---

## `clido audit`

View the tool call audit log.

```bash
clido audit
clido audit --tail 20
clido audit --session a1b2c3
clido audit --tool Bash
clido audit --since 2026-03-01
clido audit --json
```

| Flag | Description |
|------|-------------|
| `--tail <N>` | Show last N entries |
| `--session <id>` | Filter by session ID prefix |
| `--tool <name>` | Filter by tool name |
| `--since <date>` | Filter by start date (ISO 8601) |
| `--json` | Output as newline-delimited JSON |

See [Audit Log](/docs/guide/audit) for full documentation.

---

## `clido memory list`

List long-term memories, most recent first.

```bash
clido memory list
clido memory list --limit 50
```

| Flag | Description |
|------|-------------|
| `--limit <N>` | Maximum entries to show (default: 20) |

---

## `clido memory prune`

Delete old memories, keeping the N most recent.

```bash
clido memory prune --keep 100
```

| Flag | Description |
|------|-------------|
| `--keep <N>` | Number of memories to keep (default: 100) |

---

## `clido memory reset`

Delete all memories.

```bash
clido memory reset
clido memory reset --force   # skip confirmation
```

| Flag | Description |
|------|-------------|
| `--force` | Skip confirmation prompt |

---

## `clido index build`

Build (or update) the repository index.

```bash
clido index build
clido index build --dir /path/to/project
clido index build --ext rs,py,ts
```

| Flag | Default | Description |
|------|---------|-------------|
| `--dir <path>` | current directory | Directory to index |
| `--ext <exts>` | see `clido index build --help` | Comma-separated file extensions (includes sol/move and common langs by default) |

---

## `clido index stats`

Show repository index statistics.

```bash
clido index stats
```

---

## `clido index clear`

Delete the repository index.

```bash
clido index clear
```

---

## `clido workflow run <FILE> [OPTIONS]`

Run a workflow from a YAML file.

```bash
clido workflow run full-review.yaml -i branch=main
clido workflow run full-review.yaml -i branch=main --dry-run
clido workflow run full-review.yaml -i branch=main --yes
```

| Flag | Description |
|------|-------------|
| `-i <key=value>` | Input override (repeatable) |
| `--dry-run` | Validate and render prompts without API calls |
| `--yes` | Skip cost confirmation |

---

## `clido workflow validate <FILE>`

Validate a workflow YAML file.

```bash
clido workflow validate full-review.yaml
```

---

## `clido workflow inspect <FILE>`

Print the workflow step graph and execution order.

```bash
clido workflow inspect full-review.yaml
```

---

## `clido workflow check <FILE>`

Run preflight checks (profiles, tools, inputs) without executing the workflow.

```bash
clido workflow check full-review.yaml
clido workflow check full-review.yaml --json
```

| Flag | Description |
|------|-------------|
| `--json` | Output as JSON |

---

## `clido workflow list`

List all workflows in the configured workflow directory.

```bash
clido workflow list
```

---

## `clido list-models`

List all models known to clido from the built-in pricing table.

```bash
clido list-models
clido list-models --provider anthropic
clido list-models --json
```

| Flag | Description |
|------|-------------|
| `--provider <name>` | Filter by provider |
| `--json` | Output as JSON |

---

## `clido fetch-models`

Fetch the current model list from a provider's live API.

```bash
clido fetch-models
clido fetch-models --provider openrouter --json
```

| Flag | Description |
|------|-------------|
| `--provider <name>` | Provider to query |
| `--json` | Output as JSON |

---

## `clido update-pricing`

Download the latest model pricing data from the remote pricing source.

```bash
clido update-pricing
```

Prints the current pricing file path and its age. Downloads and replaces it if newer data is available.

---

## `clido skills`

List or toggle **skills** (reusable instructions loaded from `.clido/skills/` and related paths). See [Skills](/docs/guide/skills).

```bash
clido skills list
clido skills paths
clido skills disable my-skill
clido skills enable my-skill
```

| Subcommand | Description |
|------------|-------------|
| `list` | Print each skill id, short summary, and whether it is active for this workspace |
| `paths` | Print resolved search directories (and configured `registry-urls`, reserved) |
| `disable <id>` | Append id to `[skills].disabled` in **project** `.clido/config.toml` |
| `enable <id>` | Remove id from the disabled list |

Restart the agent session after changes so the system prompt reloads.

---

## `clido completions <SHELL>`

Generate shell completion script and print to stdout.

```bash
clido completions bash >> ~/.bash_completion
clido completions zsh > "${fpath[1]}/_clido"
clido completions fish > ~/.config/fish/completions/clido.fish
```

Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`.

---

## `clido man`

Generate a man page and print to stdout.

```bash
clido man > /usr/local/share/man/man1/clido.1
man clido
```

---

## `clido version`

Print the clido version and exit.

```bash
clido version
# clido 0.1.0
```

Equivalent to `clido --version`.
