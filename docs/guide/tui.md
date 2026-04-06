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
╭─ header: brand · model · profile · session … ───────────────────────────────╮
│  [chat]  conversation, tools, markdown, code blocks                           │
│  …                                                                            │
├─ progress (optional) ─────────────────────────────────────────────────────────┤
│  Progress · auto   ○ todo   › active   ✓ done   (todos / planner / harness)   │
├─ status strip ────────────────────────────────────────────────────────────────┤
│  activity log / tool line · spinner                                            │
├─ queue (optional) ─────────────────────────────────────────────────────────────┤
│  current step / queued messages                                                │
├─ hint ─────────────────────────────────────────────────────────────────────────┤
│  key hints                                                                     │
╰─ multiline input ─────────────────────────────────────────────────────────────╯
```

| Area | Description |
|------|-------------|
| **Header** | Model, profile, workspace path, session id/title, token/cost hints. |
| **Chat pane** | Scrollable conversation: user, assistant, tools, errors. |
| **Plan / todo strip** | Optional panel between chat and status: **TodoWrite** steps (or plan snapshot), with ○ / › / ✓ markers. Controlled with `/plan on`, `/plan off`, `/plan auto` (default: **auto** hides on small terminals). |
| **Status strip** | Short activity log and current tool; complements the header. |
| **Input** | Multiline draft (grows up to a few lines); **Enter** sends, **Shift+Enter** newline. |

See [Slash commands](/docs/reference/slash-commands) for the full command list (Git, plan, skills, workflows, etc.).

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

Type `/` in the input field for autocomplete, or see the full tables in **[Slash commands](/docs/reference/slash-commands)** (session, model, Git, **plan**, **skills**, workflows, profiles, memory, etc.).

Common shortcuts:

| Command | Description |
|---------|-------------|
| `/help` | Key bindings + slash commands in chat |
| `/sessions` | Session picker |
| `/skills list` | Skills on disk and whether each is active |
| `/plan` / `/plan <task>` | Show plan or ask the agent to plan first |
| `/todo` | Current TodoWrite list |
| `/stop` | Cancel the current agent turn |
| `/quit` | Exit clido |

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

The strip **below the chat** (and below the optional progress panel) shows recent activity and the current tool. Detailed **session id, cost, tokens, and model** appear in the **header**, not only here.

When the agent is idle, the spinner clears; queued messages may still show above the input.

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

## Text selection

Shift+drag in the chat area to select text character-by-character. The selection is highlighted in real time. On mouse release, the selected text is automatically copied to your clipboard and a brief toast confirmation appears near the cursor.

This works even though clido enables mouse capture for scrolling — Shift bypasses terminal mouse capture so the app can handle selection itself.
