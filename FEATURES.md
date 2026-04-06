# Clido TUI Features & Keybindings

## Input Field (Bottom)

### Basic Typing
- **Any character** - Type into input field
- **Cursor** - Visual indicator shows position

### Cursor Movement
| Key | Action |
|-----|--------|
| `←` / `→` | Move cursor left/right by character |
| `Alt+←` / `Alt+→` | Move cursor by word |
| `Home` | Jump to start of line |
| `End` | Jump to end of line |
| `↑` (in multiline) | Move to previous line |
| `↓` (in multiline) | Move to next line |

### Text Editing
| Key | Action |
|-----|--------|
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character after cursor |
| `Ctrl+W` | Delete word backward |
| `Ctrl+U` | Clear entire input |

### Sending Messages
| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | Insert newline (multiline) |
| `Ctrl+Enter` | Force send (interrupt current run) |

### History Navigation
| Key | Action |
|-----|--------|
| `↑` (when not in multiline) | Previous message in history |
| `↓` (when not in multiline) | Next message in history |
| `Esc` | Clear input + exit history browsing |

### Queue Navigation (when agent busy)
| Key | Action |
|-----|--------|
| `↑` | Cycle through queued items (newest first) |
| `↓` | Cycle forward through queued items |

Note: Queue nav happens BEFORE history nav when items are queued.

---

## Chat Area (Main)

### Scrolling
| Key / Action | Result |
|--------------|--------|
| `PageUp` | Scroll up 10 lines |
| `PageDown` | Scroll down 10 lines |
| `Ctrl+Home` | Jump to top |
| `Ctrl+End` | Jump to bottom (enable follow mode) |
| **Mouse wheel** | Scroll up/down |

### Text Selection & Copy
- **Shift + drag** — Select text character-by-character (works with mouse capture)
- **Auto-copy** — Selection is copied to clipboard automatically on mouse release
- **Toast** — A brief "Copied N chars" message appears near the cursor for 2 seconds

---

## Global Shortcuts

| Shortcut | Action |
|----------|--------|
| `Esc` | Clear input OR cancel auto-resume timer |
| `Ctrl+L` | Refresh screen |
| `Ctrl+K` | Show keybindings overlay |
| `Ctrl+/` | Stop current agent run |

---

## Slash commands

Type `/` for autocomplete. The **authoritative list** with arguments and notes is in the repo:

`docs/reference/slash-commands.md`

Examples: `/sessions`, `/skills list`, `/plan`, `/plan <task>`, `/progress on|off|auto`, `/workflow`, `/memory`, `/git`-related shortcuts (`/ship`, `/pr`, …), `/profile`, `/models`.

---

## Mouse Support

| Action | Result |
|--------|--------|
| **Scroll wheel** | Scroll chat up/down |
| **Shift + drag** | Select text (character-level, auto-copies to clipboard) |
| **Click** | (Currently no click handling) |

---

## Layout (summary)

- **Header** — brand, model, profile, workspace path, session id/title, cost/tokens when shown.
- **Chat** — scrollable transcript.
- **Progress strip** (optional) — todos / planner / harness; `/progress on|off|auto`.
- **Status strip** — short activity log + spinner.
- **Queue / thinking** — current step line and queued messages when relevant.
- **Hint line** — context shortcuts.
- **Input** — multiline draft; Enter sends, Shift+Enter newline.

---

## Changes Since Beta 1

### Fixed
1. **Terminal escape sequences** - No more `^[[201~` garbage on startup
2. **Mouse scrolling** - Works again (was broken, now fixed)
3. **Queue display** - Each item on separate line
4. **Input field height** - Multiline input grows up to a few lines (see TUI render limits)
5. **Session loss** - Canonicalized paths fix session detection
6. **Tool timeout** - 60s timeout prevents indefinite hangs
7. **/stop command** - Actually stops agent immediately
8. **Tool history** - Tool results added before interrupt

### Added
1. **Queue editing** - Arrow up to edit queued items
2. **Stall warning** - Warning at 30s if agent stuck
3. **Kimi Code user-agent** - Uses RooCode/3.0.0

### Behavior Changes
- **Text selection**: Now requires Shift+drag (due to mouse capture for scrolling)
- **Queue priority**: Thinking shown when active, queue when idle
- **Input clearing**: Won't clear if you're typing when queued item sends

---

## Known Limitations

1. **Text selection**: Requires Shift+drag (terminal limitation with mouse capture). Auto-copies on release.
2. **Click handling**: No click-to-focus or click-to-position yet
3. **Clipboard**: OSC 52 clipboard integration may not work in all terminals or over SSH
