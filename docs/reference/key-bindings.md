# Key Bindings (TUI)

All keyboard shortcuts available in the interactive TUI.

## Normal mode — agent idle

These keys are active when the agent is not running and no modal is open.

| Key | Action |
|-----|--------|
| `Enter` | Send the message in the input field |
| `Ctrl+C` | Quit clido |
| `Ctrl+/` | Interrupt current run without sending a follow-up message |
| `Ctrl+Y` | Copy last assistant message (OSC 52) |
| `Esc` | Dismiss inline hints/modals when applicable |
| `Up` | Scroll chat up, or recall history when editing input |
| `Down` | Scroll chat down, or move forward in history |
| `Page Up` | Scroll chat history up by one page |
| `Page Down` | Scroll chat history down by one page |
| `Ctrl+U` | Delete everything before cursor |
| `Home` | Move cursor to start of input |
| `End` | Move cursor to end of input |
| `Left` | Move cursor left |
| `Right` | Move cursor right |

## While agent is running

| Key | Action |
|-----|--------|
| `Ctrl+Enter` | Cancel running turn and send current input immediately |
| `Ctrl+/` | Cancel running turn without sending current input |
| Any text | Type a message; it is queued and sent after the agent finishes |

## Permission prompt (modal)

Appears when the agent calls a state-changing tool and `--permission-mode default` is active.

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move option selection |
| `Enter` | Confirm selected option |
| `Esc` | Deny this tool call |

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
- `Ctrl+Y` and `/copy` rely on OSC 52 clipboard integration and may be blocked by some terminals or SSH hops.
- macOS Terminal.app has limited key support. iTerm2 or Warp are recommended for the best TUI experience.
