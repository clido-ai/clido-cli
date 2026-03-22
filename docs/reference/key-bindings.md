# Key Bindings (TUI)

All keyboard shortcuts available in the interactive TUI.

## Normal mode — agent idle

These keys are active when the agent is not running and no modal is open.

| Key | Action |
|-----|--------|
| `Enter` | Send the message in the input field |
| `Ctrl+J` | Insert a newline into the input field (multi-line input) |
| `Ctrl+C` | Quit clido |
| `q` | Quit clido (only when the input field is empty) |
| `Esc` | Cancel partial input / dismiss inline hints |
| `Up` | Scroll chat history up one line |
| `Down` | Scroll chat history down one line |
| `Page Up` | Scroll chat history up by one page |
| `Page Down` | Scroll chat history down by one page |
| `Home` | Scroll chat history to the top |
| `End` | Scroll chat history to the bottom (re-enable auto-scroll) |
| `Up` (empty input) | Recall previous input from history |
| `Down` (in history) | Move forward through input history |
| `Ctrl+L` | Clear the input field |
| `Ctrl+A` | Move cursor to start of input |
| `Ctrl+E` | Move cursor to end of input |
| `Ctrl+W` | Delete word before cursor |
| `Ctrl+U` | Delete everything before cursor |
| `Ctrl+K` | Delete everything after cursor |
| `Left` | Move cursor left |
| `Right` | Move cursor right |
| `Ctrl+Left` | Move cursor one word left |
| `Ctrl+Right` | Move cursor one word right |

## While agent is running

| Key | Action |
|-----|--------|
| `Ctrl+C` | Cancel the running agent turn and stop execution |
| Any text | Type a message; it is queued and sent after the agent finishes |
| `Esc` | Dismiss any inline notifications |

## Permission prompt (modal)

Appears when the agent calls a state-changing tool and `--permission-mode default` is active.

| Key | Action |
|-----|--------|
| `y` | Allow this tool call |
| `Enter` | Allow this tool call |
| `n` | Deny this tool call |
| `Esc` | Deny this tool call |
| `a` | Allow all remaining tool calls in this session (equivalent to `accept-all`) |

## Session picker

Opened with `/sessions`.

| Key | Context | Action |
|-----|---------|--------|
| `Up` | Picker | Move selection up |
| `Down` | Picker | Move selection down |
| `Enter` | Picker | Open the selected session |
| `Esc` | Picker | Close the picker without changing sessions |
| Any printable character | Picker | Append to the filter string |
| `Backspace` | Picker | Delete last character from filter |
| `Ctrl+U` | Picker | Clear the filter string |

## Plan editor (full-screen overlay)

Opened automatically when `--plan` generates a plan, or via `/plan edit`.

### Task list

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move task selection |
| `Enter` | Open inline edit form for the selected task |
| `d` | Delete the selected task (blocked if other tasks depend on it) |
| `n` | Add a new task at the end and open its edit form |
| `Space` | Toggle skip on the selected task |
| `r` | Move selected task up one position (reorder) |
| `s` | Save plan to `.clido/plans/<id>.json` |
| `x` | Execute the plan (sends tasks as a structured prompt to the agent) |
| `Esc` | Abort — close editor without executing |

### Inline task edit form

| Key | Action |
|-----|--------|
| `Tab` | Move focus to next field (Description → Notes → Complexity) |
| `←` / `→` | Cycle complexity when Complexity field is focused |
| `Enter` | Save edits and return to task list |
| `Esc` | Discard edits and return to task list |

## Error modal

Appears when a non-recoverable error occurs.

| Key | Action |
|-----|--------|
| `Enter` | Dismiss the error modal |
| `Esc` | Dismiss the error modal |
| `q` | Dismiss and quit |

## Notes

- Key bindings are not currently user-configurable. Custom bindings are planned for a future release.
- On some terminals, `Ctrl+J` may be indistinguishable from `Enter`. Use a terminal emulator that sends distinct escape sequences (most modern ones do).
- macOS Terminal.app has limited key support. iTerm2 or Warp are recommended for the best TUI experience.
