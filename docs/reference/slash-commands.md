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

### Model

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/model [name]` | Show or switch the active model | `/model claude-opus-4-6` | Switches immediately; reverts after session ends |
| `/fast` | Switch to the fast (cheap) model | `/fast` | `claude-haiku-4-5-20251001` |
| `/smart` | Switch to the smart (powerful) model | `/smart` | `claude-opus-4-6` |

### Context

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/cost` | Print accumulated cost for this session | `/cost` | Mirrors the status strip numbers |
| `/tokens` | Print input and output token usage | `/tokens` | |
| `/compact` | Compact the context window immediately | `/compact` | Summarises history via LLM; shows before/after message count |
| `/memory <query>` | Search long-term memory | `/memory error handling` | The agent also uses memory automatically |

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

### Project

| Command | Description | Example | Notes |
|---------|-------------|---------|-------|
| `/workdir` | Show the current working directory | `/workdir` | |
| `/check` | Run diagnostics on the current project | `/check` | Invokes the DiagnosticsTool |
| `/index` | Show repo index stats | `/index` | Build with `clido index build` |
| `/rules` | Show active CLIDO.md rules files | `/rules` | Overlay listing all discovered rules |
| `/image <path>` | Attach an image to the next message | `/image screenshot.png` | Supports PNG, JPEG, GIF, WebP |

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

TUI slash commands are distinct from CLI subcommands. For example, `/sessions` in the TUI opens the picker, while `clido sessions list` on the command line prints a table. See [CLI Reference](/reference/cli) for the full list of CLI commands.
