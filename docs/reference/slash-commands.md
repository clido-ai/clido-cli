# Slash Commands (TUI)

Slash commands are typed in the TUI input field and executed immediately when you press Enter. They are only available in the interactive TUI — not in CLI / non-TTY mode.

## Command list

### Session

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/clear` | Clear the chat display | `/clear` | The session JSONL file is not modified; history is preserved |
| `/sessions` | Open the session picker | `/sessions` | Use arrow keys to select, Enter to resume |
| `/session` | Show the current session ID | `/session` | |
| `/help` | Display all key bindings and slash commands | `/help` | Output appears in the chat pane |
| `/quit` | Exit clido | `/quit` | Equivalent to pressing `Ctrl+C` when idle |
| `/init` | Re-run setup wizard | `/init` | Reconfigure provider, model, API key, and roles |
| `/search <query>` | Search conversation history | `/search auth bug` | Highlights matching messages |
| `/export` | Save conversation to a markdown file | `/export` | Saves to current directory |

### Settings

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/config` | Show all settings | `/config` | Displays provider, model, roles, agent, context |
| `/configure <intent>` | Change settings with natural language | `/configure use gpt-4.1` | |
| `/settings` | Open settings editor | `/settings` | Edit roles and default model |
| `/enhance <prompt>` | Enhance a prompt before sending | `/enhance fix the login bug` | Sends to utility model; result appears in input field for review. Press Enter to send or edit first |

### Model

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/model [name]` | Show or switch the active model | `/model claude-opus-4-6` | Switches immediately; reverts after session ends |
| `/models` | Open the interactive model picker overlay | `/models` | Live type-to-filter; shows pricing, context window, and role assignments |
| `/fast` | Switch to the fast (cheap) model | `/fast` | Uses `[roles] fast` from config, falls back to `claude-haiku-4-5-20251001` |
| `/smart` | Switch to the smart (powerful) model | `/smart` | Uses `[roles] reasoning` from config, falls back to `claude-opus-4-6` |
| `/fav` | Toggle current model in/out of favorites | `/fav` | Favorites shown with ★ in the model picker and `/model` output |
| `/reviewer [on\|off]` | Toggle reviewer sub-agent | `/reviewer on` | When on, a second model reviews each assistant response before it is shown |

### Context

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/cost` | Print accumulated cost for this session | `/cost` | Mirrors the status strip numbers |
| `/tokens` | Print input and output token usage | `/tokens` | |
| `/compact` | Compact the context window immediately | `/compact` | Summarises history via LLM; shows before/after message count |
| `/memory <query>` | Search long-term memory | `/memory error handling` | The agent also uses memory automatically |
| `/todo` | Show the agent's current task list | `/todo` | Displays pending tasks from the current session |

### Git

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/branch <name>` | Create a new branch and switch to it | `/branch feature/auth` | Stashes uncommitted changes, creates branch, pushes with upstream |
| `/sync` | Pull and rebase from upstream | `/sync` | Stashes if needed, fetches, rebases, resolves simple conflicts |
| `/pr [title]` | Create a pull request | `/pr add login rate limiting` | Auto-generates title and body from diff; requires `gh` or prints for manual creation |
| `/ship [msg]` | Stage all changes, commit, and push | `/ship fix login bug` | Auto-generates message if none given; repair cycle on hook/push failures |
| `/save [msg]` | Stage all changes and commit locally (no push) | `/save wip checkpoint` | Auto-generates message if none given; repair cycle on hook failures |
| `/undo` | Undo the last committed change | `/undo` | Runs `git reset HEAD~1`; shows what was undone |
| `/rollback [id]` | Restore to a checkpoint or commit | `/rollback ck_abc123` | Accepts checkpoint ID (`ck_…`) or git commit hash |

### Plan

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/plan` | Show the current task plan | `/plan` | Active when `--plan` or `--planner` flag is set |
| `/plan edit` | Re-open the plan editor overlay | `/plan edit` | Edit tasks, complexity, notes before executing |
| `/plan save` | Save the current plan to `.clido/plans/` | `/plan save` | Saved plans can be resumed with `clido plan run` |
| `/plan list` | List all saved plans | `/plan list` | Shows id, task count, done count, and goal |

### Workflow

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/workflow` | List all saved workflows | `/workflow` | Same as `/workflow list` |
| `/workflow new` | Create a workflow with AI guidance | `/workflow new review PRs and test` | Agent walks you through the design step by step |
| `/workflow list` | List all saved workflows | `/workflow list` | Scans `.clido/workflows/` and global dir |
| `/workflow show` | Display a workflow's YAML | `/workflow show full-review` | Shows the full YAML in the chat |
| `/workflow edit` | Open in text editor | `/workflow edit full-review` | Ctrl+S validates & saves, Esc discards |
| `/workflow save` | Save last YAML from chat | `/workflow save my-review` | Extracts YAML block from last assistant message |
| `/workflow run` | Run a saved workflow | `/workflow run full-review` | Sends steps to the agent for execution |

### Project

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/agents` | Show current agent configuration | `/agents` | Lists main provider and fast provider (if configured) |
| `/profiles` | List all profiles | `/profiles` | Shows active model per slot for each profile |
| `/profile` | Open profile picker | `/profile` | Switch, create, or edit profiles interactively |
| `/profile new` | Create a new profile | `/profile new` | Launches the guided setup wizard |
| `/profile edit [name]` | Edit a profile | `/profile edit cheap` | Edit provider, model, and API key for the named profile |
| `/settings` | Open settings editor | `/settings` | Edit roles and default model; changes saved to `config.toml` |
| `/workdir [path]` | Show or set working directory | `/workdir ~/projects/myapp` | Without argument shows current cwd |
| `/check` | Run diagnostics on the current project | `/check` | Invokes the DiagnosticsTool |
| `/index` | Show repo index stats | `/index` | Build with `clido index build` |
| `/rules` | Show active CLIDO.md rules files | `/rules` | Overlay listing all discovered rules |
| `/image <path>` | Attach an image to the next message | `/image screenshot.png` | Supports PNG, JPEG, GIF, WebP |
| `/stop` | Interrupt current run | `/stop` | Cancels the in-progress agent turn without exiting |
| `/copy` | Copy last assistant message to clipboard | `/copy` | Uses OSC 52 escape sequence; requires terminal support |
| `/notify [on\|off]` | Toggle desktop notifications | `/notify on` | Shows system notifications when agent completes a turn |
| `/index` | Show codebase index stats | `/index` | Build the index with `clido index build` |

## Using slash commands

Type a `/` followed by the command name in the input field:

```
> /sessions
```

Press Enter to execute. Commands that produce output render it as a system message in the chat pane (visually distinct from user and assistant messages).

### Commands with arguments

`/memory` accepts a search query as the rest of the line:

```
> /memory refactor authentication module
```

```
[memory search: "refactor authentication module"]
  • User prefers JWT over session cookies (2026-03-15)
  • Auth module was refactored to use tower-service (2026-03-10)
  • AuthError variants: Expired, Invalid, MissingToken (2026-03-08)
```

## Model picker

`/models` opens a searchable overlay listing all models known to clido:

```
╭─ Models ─────────────────────────────────────────────────────────────────────╮
│  Filter: _                                                                     │
│                                                                                │
│  ★  claude-haiku-4-5-20251001      anthropic   0.80   4.00  200k  fast        │
│  ★  claude-opus-4-6                anthropic  15.00  75.00  200k  reasoning   │
│  >  claude-sonnet-4-6              anthropic   3.00  15.00  200k              │
│     gpt-4o                         openai      2.50  10.00  128k              │
│     mistralai/mistral-large        openrouter  2.00   6.00   32k              │
╰──────────────────────────────────────────────────────────────────────────────╯
  ↑/↓ navigate  Enter select  f favorite  Escape cancel  Type to filter
```

Columns: ★ (favorited), model ID, provider, in$/mtok, out$/mtok, context window, role.

| Key | Action |
|-----|--------|
| `Up` / `Down` | Move selection |
| `Enter` | Switch to the selected model |
| `f` | Toggle favorite on the highlighted model |
| `Escape` | Close the picker without switching |
| Any text | Live-filter models by ID or provider |
| `Backspace` | Delete last filter character |

Models are ordered: favorites (alphabetical) → recently used → rest (alphabetical).

## Session picker

`/sessions` opens a full-screen picker overlay:

```
╭─ Sessions ──────────────────────────────────────────────────────────────────╮
│  Filter: _                                                                    │
│                                                                               │
│  > a1b2c3  2026-03-21  "Refactor the parser module"   ~/projects/app  $0.02  │
│    d4e5f6  2026-03-20  "Add unit tests for lexer"      ~/projects/app  $0.04  │
│    789abc  2026-03-19  "Fix memory leak in pool"       ~/projects/lib  $0.02  │
╰─────────────────────────────────────────────────────────────────────────────╯
  ↑/↓ navigate  Enter open  Escape cancel  Type to filter
```

| Key | Action |
|-----|--------|
| `Up` / `Down` | Move selection |
| `Enter` | Open the selected session |
| `Escape` | Close the picker without changing sessions |
| Any text | Filter sessions by ID prefix or preview text |

## Difference from CLI commands

TUI slash commands are distinct from CLI subcommands. For example, `/sessions` in the TUI opens the picker, while `clido sessions list` on the command line prints a table. See [CLI Reference](/docs/reference/cli) for the full list of CLI commands.
