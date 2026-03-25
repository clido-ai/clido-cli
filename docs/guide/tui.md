# Interactive TUI

clido includes a full terminal user interface built with [Ratatui](https://ratatui.rs/). It provides a chat-style interaction model with real-time tool progress, session management, and permission prompts.

## Launching the TUI

Run `clido` with no arguments from a terminal (TTY):

```bash
clido
```

To start with an existing session:

```bash
clido --continue          # resume the most recent session for this directory
clido --resume abc123     # resume a specific session by ID prefix
```

## Layout

```
╭─ clido ──────────────────────────────── claude-3-5-sonnet ──────────────────╮
│                                                                               │
│  [assistant]                                                                  │
│  I've read src/main.rs. The issue is on line 42 — the match arm is           │
│  missing a wildcard case. I'll fix it now.                                    │
│                                                                               │
│  [tool: Edit]  src/main.rs                                                    │
│  + 42:    _ => Err(ParseError::Unknown),                                      │
│                                                                               │
│  [tool: Bash]  cargo check                                                    │
│  ✓ Finished in 1.2s                                                           │
│                                                                               │
│  Done. The wildcard arm has been added and the project compiles cleanly.      │
│                                                                               │
├───────────────────────────────────────────────────────────────────────────────┤
│ session:a1b2c3  $0.0034  1,240 tok  4 turns           [running] Edit         │
├───────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                           │
╰───────────────────────────────────────────────────────────────────────────────╯
```

| Area | Description |
|------|-------------|
| **Chat pane** (top) | Scrollable conversation history. Assistant messages, tool calls, and tool results appear here in order. |
| **Status strip** (middle) | Session ID prefix, accumulated cost, token count, turn count, and current tool activity. |
| **Input field** (bottom) | Single-line text input. Press Enter to send. Pasted text is inserted and waits for manual Enter. |

## Key bindings

### Normal mode (agent idle)

| Key | Action |
|-----|--------|
| `Enter` | Send the message in the input field |
| `Ctrl+C` | Quit |
| `Ctrl+/` | Interrupt current run without sending a new message |
| `Ctrl+Y` | Copy the last assistant message via OSC 52 |
| `Up` / `Down` | Scroll chat history (or history navigation when typing) |
| `Page Up` / `Page Down` | Scroll chat history by page |
| `Ctrl+U` | Clear the input field |

### While agent is running

| Key | Action |
|-----|--------|
| `Ctrl+Enter` | Cancel current run and send the current input immediately |
| `Ctrl+/` | Cancel current run without sending a follow-up prompt |
| Any text | Queue a message to send after the agent finishes |

### Permission prompt (modal)

| Key | Action |
|-----|--------|
| `y` / `Enter` | Allow this tool call |
| `n` / `Escape` | Deny this tool call |
| `a` | Allow all remaining calls in this session |

### Session picker

| Key | Action |
|-----|--------|
| `Up` / `Down` | Move selection |
| `Enter` | Open selected session |
| `Escape` | Close picker |
| `/` + text | Filter sessions by ID or preview text |

### Error modal

| Key | Action |
|-----|--------|
| `Enter` / `Escape` | Dismiss |

## Slash commands

Type a slash command in the input field and press Enter:

| Command | Description | Example |
|---------|-------------|---------|
| `/help` | Show all slash commands | `/help` |
| `/sessions` | Open the session picker | `/sessions` |
| `/clear` | Clear the chat display (session is preserved) | `/clear` |
| `/plan` | Show the current planner task graph (if `--planner` is active) | `/plan` |
| `/memory` | Search long-term memory | `/memory refactor patterns` |
| `/cost` | Print accumulated cost and token usage | `/cost` |
| `/model` | Show or switch active model | `/model gpt-4.1` |
| `/workdir` | Show or set workdir used by tools | `/workdir ~/src/project` |
| `/stop` | Interrupt current run without sending | `/stop` |
| `/copy` | Copy last assistant message (OSC 52) | `/copy` |
| `/quit` | Quit clido | `/quit` |

## Permission prompts

When the agent calls a state-changing tool (e.g. `Bash`, `Write`, `Edit`) and the permission mode is `default`, clido pauses and shows a prompt:

```
╭─ Permission Required ────────────────────────────────────────────────────────╮
│                                                                               │
│  Tool: Bash                                                                   │
│  Input:                                                                       │
│    command: "rm -rf ./build"                                                  │
│                                                                               │
│  Allow this tool call?                                                        │
│  [y] Yes   [n] No   [a] Allow all                                             │
│                                                                               │
╰───────────────────────────────────────────────────────────────────────────────╯
```

- **y / Enter** — allow this single call
- **n / Escape** — deny; the agent receives an error and may try an alternative
- **a** — allow all remaining tool calls in the session (equivalent to `--permission-mode accept-all`)

::: tip Skipping prompts
Pass `--permission-mode accept-all` to skip all permission prompts. Use `--permission-mode plan` to prevent the agent from calling any tools at all (plan-only mode).
:::

## Session picker

Open the session picker with `/sessions`:

```
╭─ Sessions ───────────────────────────────────────────────────────────────────╮
│  > a1b2c3  2026-03-21  ~/projects/app  "Refactor the parser module"  $0.023  │
│    d4e5f6  2026-03-20  ~/projects/app  "Add unit tests for lexer"    $0.041  │
│    789abc  2026-03-19  ~/projects/lib  "Fix memory leak in pool"     $0.019  │
╰───────────────────────────────────────────────────────────────────────────────╯
```

Press Enter to resume the selected session. The chat history is loaded and you can continue the conversation.

## Status strip

The status strip at the bottom of the chat pane shows:

| Field | Example | Description |
|-------|---------|-------------|
| Session ID | `session:a1b2c3` | First 6 characters of the session ID |
| Cost | `$0.0034` | Accumulated cost for this session |
| Tokens | `1,240 tok` | Total tokens used (input + output) |
| Turns | `4 turns` | Number of agent turns completed |
| Activity | `[running] Edit` | Current tool name while agent is active |

When the agent is idle the activity field is blank.

## Scroll behaviour

The chat pane scrolls automatically to the bottom as new content arrives. If you scroll up manually, auto-scroll is paused. Scroll back to the bottom to re-enable auto-scroll.

## Input history

The TUI remembers your last 50 inputs per session. Press `Up` when the input field is empty to cycle through previous messages.

## Queueing messages

You can type a message while the agent is running. The message is queued and sent automatically when the current agent turn completes. The queued message is shown with a subtle indicator in the status strip.

## Clipboard notes

`/copy` and `Ctrl+Y` use OSC 52 clipboard integration. Support depends on your terminal and SSH setup.

- Works in most modern local terminals.
- Over SSH, clipboard support may require explicit terminal settings.
- If OSC 52 is blocked, clido shows a copy error and your clipboard is unchanged.
