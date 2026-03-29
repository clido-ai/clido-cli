# Clido TUI Architecture Analysis & Refactoring Plan

> Comparative analysis of 7 coding CLI agents, focused on simplifying and
> strengthening clido's TUI architecture, navigation, and interaction model.

**Tools analyzed:**
| Tool | Stack | TUI Lines | Architecture |
|------|-------|-----------|--------------|
| **Clido** | Rust / ratatui | 11,670 (tui.rs) + 3,768 (setup.rs) | Monolithic single-file |
| **Codex** | Rust / ratatui | 58,472 across 81 files | Modular, view-stack, event-driven |
| **Gemini CLI** | TypeScript / Ink (React) | ~6,700 across 300+ files | Component model, context providers |
| **OpenCode** | TypeScript / SolidJS | 8,306 across 89 files | Dialog stack, triple-layer state |
| **Goose** | Rust / cliclack + rustyline | ~4,000 | No TUI — REPL with prompts |
| **Aider** | Python / prompt_toolkit | ~4,200 | REPL, reflection-based commands |
| **Open Interpreter** | Python / Rich | ~2,500 | Minimal streaming loop |

---

## 1. Overall TUI Architecture

### Comparison

| Tool | Pattern | Key Trait |
|------|---------|-----------|
| Codex | **Layered**: App → ChatWidget → BottomPane → Views | View trait unifies all modals |
| Gemini | **Component tree**: Providers → Layouts → Components | React composition |
| OpenCode | **Route + Dialog stack**: Routes for pages, stack for modals | SolidJS reactivity |
| Goose/Aider | **REPL loop**: input → process → output | No retained UI state |
| **Clido** | **Monolith**: Single 11,670-line file, 86 App fields | Everything in one struct |

### Simplest Robust Approach

Codex's **three-layer model** scales well in ratatui:
1. **App** — owns all state, routes events, coordinates rendering
2. **Main viewport** — scrollable message history + streaming
3. **Footer / overlay layer** — input field + modal stack (only top modal is interactive)

### Where Clido Is Overcomplicated

- **11,670 lines in one file** — no separation of concerns
- **86 fields in `App`** — every overlay, picker, and mode flag is a flat field
- **13 concurrent `Option<State>` overlay fields** — checked in priority cascade during both render and key handling
- **No trait abstraction for overlays** — each modal is ad-hoc, inline code

### Simplification Directions

1. **Extract overlay/modal system** into a `Vec<Box<dyn Overlay>>` stack with a shared trait (render, handle_key, is_done)
2. **Split tui.rs** into modules: `tui/app.rs`, `tui/render.rs`, `tui/input.rs`, `tui/overlays/*.rs`, `tui/commands.rs`
3. **Group App fields** into sub-structs: `UiState`, `AgentState`, `SessionState`, `OverlayStack`

### TODOs
- [ ] Define `Overlay` trait (render, handle_key, handle_paste, is_complete)
- [ ] Replace 13 `Option<XxxState>` fields with `overlay_stack: Vec<Box<dyn Overlay>>`
- [ ] Split tui.rs into 5–8 module files (target: <2,000 lines each)
- [ ] Group App's 86 fields into 4–5 sub-structs

---

## 2. Navigation Model

### Comparison

| Tool | Navigation | Mode Indicators |
|------|-----------|----------------|
| Codex | Implicit: top view on stack gets all input | View title in footer |
| Gemini | Mode flags + layout branches | Visual mode badge |
| OpenCode | Route-based pages + dialog stack | Route header |
| Goose | N/A (REPL) | Prompt prefix |
| **Clido** | Priority cascade of 13 if-checks in handle_key | None — user guesses |

### Where Clido Is Overcomplicated

The key handler has a **12-layer priority cascade**:
```
plan_text_editor → plan_editor → profile_overlay → pending_error
→ rules_overlay → pending_perm → settings → session_picker
→ model_picker → profile_picker → role_picker → normal input
```
Each is a separate if-block with duplicated patterns. There's no visual indicator of which "mode" the user is in.

### Simplification Directions

1. **Replace cascade with overlay stack**: top of stack receives all input. No priority logic needed.
2. **Always show mode indicator** in the status bar: "Creating profile…", "Selecting model…", etc.
3. **Consistent escape semantics**: ESC always pops the top overlay. No exceptions.

### TODOs
- [ ] Implement overlay stack with automatic input routing to top
- [ ] Add mode indicator to status bar (overlay title)
- [ ] Enforce: ESC = dismiss top overlay (universal rule)

---

## 3. Menu & Overlay System

### Comparison

| Tool | Approach | Modals |
|------|----------|--------|
| Codex | `BottomPaneView` trait + stack (push/pop) | 10+ view types |
| Gemini | React `<DialogManager>` with stack | 14+ dialog types |
| OpenCode | `dialog.show(element)` / `dialog.clear()` stack | 14 dialog types |
| Goose | cliclack::select() — blocking prompt | 0 overlays |
| **Clido** | 13 independent `Option<State>` fields, checked in order | Ad-hoc per modal |

### Simplest Robust Pattern (from Codex)

```rust
trait Overlay {
    fn title(&self) -> &str;
    fn render(&self, frame: &mut Frame, area: Rect);
    fn handle_key(&mut self, key: KeyEvent) -> OverlayResult;
    fn handle_paste(&mut self, text: &str) -> bool { false }
    fn is_complete(&self) -> bool { false }
}

enum OverlayResult {
    Consumed,       // Key handled, stay on this overlay
    Dismiss,        // Close this overlay
    PassThrough,    // Let parent handle
    Action(AppAction), // Trigger an app-level action
}
```

Stack in App:
```rust
struct App {
    overlay_stack: Vec<Box<dyn Overlay>>,
    // ...
}
```

Rendering: render base UI, then render overlays bottom-to-top (topmost on top).
Input: route to `overlay_stack.last_mut()`. If PassThrough or empty stack → main input.

### Where Clido Is Overcomplicated

- **No shared abstraction**: profile overlay has its own 5-mode FSM (477 lines), permission modal has its own inline logic, error modal is 3 lines, each with different dismiss semantics
- **Render priority**: 13 if-checks in render() decide which modal to show — fragile if two are active
- **Paste routing**: had to special-case paste for profile overlay because there's no unified input routing

### Simplification Directions

1. **One trait, one stack** — eliminates all priority cascading
2. **Auto-dismiss**: overlays set `is_complete()` → stack auto-pops (Codex pattern)
3. **Shared rendering**: `modal_block()` already exists — make it part of the trait's default render

### TODOs
- [ ] Define `Overlay` trait with render, handle_key, handle_paste, is_complete, title
- [ ] Implement `OverlayStack` (push, pop, render_all, route_input)
- [ ] Migrate each modal: ProfileOverlay, PermissionModal, ErrorModal, SessionPicker, ModelPicker, etc.
- [ ] Remove all 13 `Option<XxxState>` fields from App
- [ ] Remove priority cascade from handle_key and render

---

## 4. Input Handling (Keyboard + Text Editing)

### Comparison

| Tool | Text Editing | Duplicated? |
|------|-------------|-------------|
| Codex | `TextArea` widget (8,081 lines) — single implementation | No — one widget, used everywhere |
| Gemini | Ink `<TextInput>` component | No — React composable |
| Aider | prompt_toolkit PromptSession | No — library handles it |
| **Clido** | Inline cursor/insert logic in **3 separate places** | Yes — main input, profile overlay, perm feedback |

### Where Clido Is Overcomplicated

Text editing logic (insert char, delete, cursor movement, word-jump, paste) is duplicated in:
1. `handle_key()` — main chat input (~200 lines)
2. `handle_profile_overlay_key()` — profile fields (~100 lines)
3. Permission feedback input (~50 lines)

Each uses slightly different field names (`app.input`/`app.cursor` vs `st.input`/`st.input_cursor`) and slightly different features (main has history, profile doesn't; paste was missing from profile until just now).

### Simplification Directions

1. **Extract a `TextInput` struct** with cursor, insert, delete, word-jump, paste, history
2. **Use it everywhere**: main input, profile fields, any future text field
3. **One handle_text_key function** that operates on `&mut TextInput`

### TODOs
- [ ] Create `TextInput` struct: text, cursor, history, selection
- [ ] Implement: insert_char, delete_back, delete_forward, word_left, word_right, paste, clear
- [ ] Replace main input fields (app.input, app.cursor) with `app.text_input: TextInput`
- [ ] Replace profile overlay input with same struct
- [ ] Single `handle_text_key(input: &mut TextInput, key: KeyEvent) -> bool`

---

## 5. Keyboard & Mouse Interaction Model

### Comparison

| Tool | Key Routing | Mouse |
|------|------------|-------|
| Codex | Hierarchical: view → pane → app | Scroll only |
| Gemini | Priority-based subscription system | Click + scroll |
| OpenCode | Dialog → keybind context → component | Click + scroll |
| **Clido** | 12-layer if-cascade | Scroll only |

### Where Clido Is Overcomplicated

- **No key map**: bindings are scattered across 880+ lines of match arms
- **No documentation of available keys** in any given mode
- **Mouse**: only scroll — no click-to-focus on overlays or messages

### Simplification Directions

1. **Overlay stack eliminates routing complexity** (top gets input, done)
2. **Document key bindings per context** — `/keys` command to show current bindings
3. **Consider leader-key mode** (OpenCode pattern) for power users

### TODOs
- [ ] Overlay stack handles routing (see §3)
- [ ] Add `/keys` command showing current context bindings
- [ ] Consolidate global shortcuts (Ctrl+C, Ctrl+L, Ctrl+D) in one place

---

## 6. Scrolling & Viewport Management

### Comparison

All tools with scrollable content use either:
- **Offset-based**: scroll position = line offset from top (Codex, clido)
- **Ratio-based**: preserve scroll ratio on resize (clido already does this)

### Where Clido Works Well

Clido's scrolling is actually reasonable: offset + ratio-preservation on resize + "following" mode that auto-scrolls to bottom during streaming. The `render_cache` avoids re-wrapping unchanged messages.

### Minor Issues

- Scroll state isn't preserved when entering/leaving overlays
- No page-up/page-down in message history (only scroll wheel + arrow keys)

### TODOs
- [ ] Add PageUp/PageDown support in main viewport
- [ ] Preserve scroll position when overlay opens/closes

---

## 7. Command System (Slash Commands)

### Comparison

| Tool | Commands | Implementation |
|------|----------|---------------|
| Codex | Typed `AppCommand` enum wrapping protocol `Op` | Pattern match in app |
| Gemini | 60+ commands via hook-based registry | `useSlashCommandProcessor` |
| OpenCode | Command registry with keybind mappings | `CommandOption[]` array |
| Aider | **Reflection**: any `cmd_*` method auto-discovered | Metaclass magic |
| **Clido** | **1,700-line match** in `execute_slash()` | Monolithic |

### Where Clido Is Overcomplicated

`execute_slash()` is **1,700 lines** — a single function with a 29-way match. Each branch:
- Parses arguments inline
- Executes side effects inline
- Handles errors inline
- Formats output inline

This is the single biggest complexity hotspot.

### Simplest Robust Pattern

Command registry (from OpenCode/Gemini):
```rust
struct SlashCommand {
    name: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    execute: fn(&mut App, args: &str) -> CommandResult,
}

static COMMANDS: &[SlashCommand] = &[
    SlashCommand { name: "help", aliases: &["?", "h"], description: "Show help", execute: cmd_help },
    SlashCommand { name: "model", aliases: &["m"], description: "Switch model", execute: cmd_model },
    // ...
];
```

### Simplification Directions

1. **Command registry** — each command is a separate function (or file)
2. **Auto-discovery for completions** — derive from registry, not hand-maintained list
3. **Uniform result type** — `CommandResult { message, action, error }`

### TODOs
- [ ] Define `SlashCommand` struct and `COMMANDS` registry
- [ ] Extract each command branch into `fn cmd_xxx(app, args) -> CommandResult`
- [ ] Move to `tui/commands/` module (one file per command or grouped)
- [ ] Derive completions from registry (remove hand-maintained list)
- [ ] Target: execute_slash() becomes <50 lines (lookup + dispatch)

---

## 8. Settings & Editing Flows

### Comparison

| Tool | Settings UX |
|------|-------------|
| Codex | In-app pickers (ListSelectionView) for model, theme, etc. |
| Gemini | Full settings dialog with scoped changes |
| OpenCode | Dialog-based: DialogModel, DialogProvider, DialogAgent |
| Goose | External config file — no in-app editing |
| Aider | `/model`, `/settings` commands + external files |
| **Clido** | Profile overlay: 5-mode FSM, 9 fields, 4-step wizard — 477 lines |

### Where Clido Is Overcomplicated

The profile overlay is a **5-mode state machine** (Overview, Creating{4 steps}, EditField, PickingProvider, PickingModel) with:
- 9 editable field types
- Nested provider/model pickers within the overlay
- Its own text input handling (duplicated from main)
- Its own save logic
- Its own error display

This is essentially an app-within-an-app.

### Simplification Directions

1. **Break into smaller overlays**: ProfileList → ProviderPicker → ModelPicker → TextInput
   Each is a simple overlay pushed onto the stack
2. **Reuse generic components**: ProviderPicker and ModelPicker should be the same ListPicker overlay with different data
3. **Eliminate the FSM**: sequential overlay pushes replace the state machine

### TODOs
- [ ] Create generic `ListPicker` overlay (filter, scroll, select — works for providers, models, sessions)
- [ ] Create generic `TextInputOverlay` (label, placeholder, validation, mask for API keys)
- [ ] Profile create = push sequence: TextInput(name) → ListPicker(provider) → TextInput(key) → ListPicker(model)
- [ ] Profile edit = push ListPicker(field) → then appropriate editor overlay
- [ ] Delete ProfileOverlayMode FSM entirely

---

## 9. Models, Roles, and Configuration UX

### Comparison

| Tool | Model Selection | Role System |
|------|----------------|-------------|
| Codex | ListSelectionView with categories, search, preview | ModelPreset with capabilities |
| Gemini | DialogModel with favorites, recent, suggested | Agent-based |
| OpenCode | DialogModel with categories + fuzzy search | Agent configs |
| **Clido** | Model picker overlay + pricing table | Role = (name, model_id) pairs |

### Where Clido Works Reasonably

The model picker backed by the pricing table works. Roles as (name, model_id) pairs are simple and useful.

### Improvements

- Model picker should show **categories** (favorites, recent, by provider) like OpenCode
- Roles should be editable via the same generic overlays (not a separate flow)
- Per-turn model override (`@model prompt`) already works — good

### TODOs
- [ ] Add categories to model picker: [Favorites] [Recent] [All]
- [ ] Unify role editing into the same ListPicker/TextInput overlay pattern

---

## 10. Error Messages & Recovery

### Comparison

| Tool | Error Pattern |
|------|--------------|
| Codex | Structured errors with user message + technical details + recovery action |
| Gemini | Dialog with retry button |
| OpenCode | Toast notifications for transient errors |
| Goose | Print to stderr, continue loop |
| **Clido** | `pending_error: Option<String>` modal — OK/dismiss |

### Where Clido Is Underdeveloped

- Errors are opaque strings with no recovery suggestion
- API errors (401, 403, 429) all show the same generic modal
- No retry action
- No distinction between transient (rate limit) and permanent (bad key) errors

### Simplification Directions

1. **Structured errors**: `ErrorInfo { title, detail, recovery: Option<Action>, is_transient }`
2. **Recovery actions**: "Press R to retry" for transient errors, "Run /init to reconfigure" for auth errors
3. **Toast for transient**: rate limits → brief toast, not blocking modal

### TODOs
- [ ] Define `ErrorInfo` struct with title, detail, recovery action
- [ ] Map API status codes to specific recovery suggestions
- [ ] Add retry capability for transient errors
- [ ] Consider toast system for non-blocking notifications

---

## 11. State & Flow Completeness

### Where Clido Has Incomplete Flows

1. **Profile create via overlay**: paste was broken until this session (fixed)
2. **Credentials file**: setup writes credentials, but agent_setup wasn't reading them (fixed this session)
3. **User-Agent override**: was silently dropped in `make_provider()` (fixed this session)
4. **Profile overlay save**: writes to config but doesn't reload the active config in memory
5. **Model picker after profile switch**: doesn't refresh model list for new provider
6. **Worker/reviewer setup**: no way to configure from TUI overlay (only via init wizard or manual config edit)

### What "100% Complete" Means for a Flow

Every flow must:
1. Have a clear entry point (command, keybind, or automatic trigger)
2. Show visual feedback during each step
3. Handle all error cases with recovery suggestions
4. Have a clear exit (success message or cancel)
5. Persist changes that survive restart
6. Reload affected runtime state after save
7. Be reachable without restarting the app

### TODOs
- [ ] After profile overlay save → reload active config + rebuild provider
- [ ] After model switch → refresh known_models if provider changed
- [ ] Audit every overlay: does dismiss always clean up properly?
- [ ] Test: create profile → switch to it → send message (end-to-end)

---

## 12. Code Complexity & Maintainability

### Current State

| Metric | Value | Target |
|--------|-------|--------|
| tui.rs lines | 11,670 | <2,000 per file |
| App struct fields | 86 | <30 (via sub-structs) |
| Largest function | 1,700 (execute_slash) | <100 |
| Overlay types | 13 (flat) | N (stack) |
| Text editing copies | 3 | 1 |
| Picker copies | 5 | 1 (generic ListPicker) |

### Complexity Sources

1. **No abstraction boundaries** — everything references `App` directly
2. **Copy-paste patterns** — text editing, picker navigation, modal rendering
3. **Inline side effects** — commands do I/O, state mutation, and rendering in one place
4. **Stringly-typed** — slash commands matched by string, not enum

### TODOs
- [ ] Split into modules (see §1)
- [ ] Extract TextInput (see §4)
- [ ] Extract ListPicker (see §8)
- [ ] Extract command registry (see §7)
- [ ] Extract overlay stack (see §3)

---

## 13. Cross-Cutting Simplification Strategy

### Concept Reduction

**Current concepts a user must understand:**
- Chat input (main)
- 29+ slash commands
- Profile overlay (5 modes)
- Session picker
- Model picker
- Role picker
- Permission modal
- Error modal
- Plan editor
- Settings overlay
- Rules overlay

**Target: 4 interaction patterns:**
1. **Chat** — type and send messages
2. **Commands** — `/xxx` for actions
3. **Overlays** — always a list-picker or text-input; ESC to close
4. **Notifications** — errors and confirmations; auto-dismiss or Enter

### Unified Interaction Rules

| Input | Everywhere |
|-------|-----------|
| ESC | Close/cancel/dismiss current overlay |
| Enter | Confirm/select/submit |
| ↑/↓ | Navigate list items |
| Type | Filter list or edit text (context-dependent) |
| Ctrl+C | Interrupt agent or exit |
| Tab | Cycle focus (if applicable) |

**No exceptions.** If ESC doesn't close something, it's a bug.

### Eliminate Special Cases

- Profile overlay's 5-mode FSM → sequence of generic overlays
- Each picker (session, model, profile, role) → one `ListPicker<T>`
- Each text field (name, API key, URL, feedback) → one `TextInput`
- Each confirmation → one `ConfirmOverlay`

---

## 14. Consistency Audit

### Current Inconsistencies

| Area | Inconsistency |
|------|---------------|
| Dismiss | ESC closes error modal but not plan editor; Ctrl+C sometimes needed |
| Paste | Routed to main input even when overlay is active (fixed partially) |
| Text editing | Main input has word-jump, profile overlay doesn't (or has different bindings) |
| Error display | API errors → modal; command errors → chat line; parse errors → status line |
| Confirmation | Some actions confirm (/clear), others don't (/undo) |
| Navigation | ↑/↓ in model picker ≠ ↑/↓ in profile overlay ≠ ↑/↓ in session picker |

### Universal Interaction Rules

1. **ESC always dismisses** the topmost overlay. Zero exceptions.
2. **Enter always confirms** the current selection or submits current input.
3. **↑/↓ always navigates** list items in any picker/list context.
4. **Typing always filters** in list context, edits text in input context.
5. **Paste always goes** to the active text input (overlay or main).
6. **Errors always show** structured info with recovery suggestion.

---

## 15. Flow Completeness Audit

### Incomplete Flows

| Flow | Issue | Severity |
|------|-------|----------|
| Profile create → use | Doesn't reload config after save | High |
| Model switch mid-session | Works but no confirmation feedback | Medium |
| Worker/reviewer setup | No TUI flow — manual config only | Medium |
| /undo | No confirmation, no preview of what's undone | Low |
| /export | Silent failure if path is unwritable | Low |
| /search | Results not navigable (just printed) | Low |

### Definition of "100% Complete"

A flow is complete when:
- ✅ Entry: discoverable (command, keybind, or help text)
- ✅ Feedback: every step shows progress or result
- ✅ Errors: caught, displayed, and recoverable
- ✅ Exit: success message or clean cancel
- ✅ Persistence: changes survive restart
- ✅ Runtime: affected state reloaded (no restart needed)
- ✅ Tested: at least one unit test for the happy path

---

## 16. Codebase Impact

### Fragmented Systems

| System | Current | Unified |
|--------|---------|---------|
| Text editing | 3 copies | 1 `TextInput` |
| List picking | 5 copies | 1 `ListPicker<T>` |
| Modal rendering | 13 ad-hoc | 1 `OverlayStack` |
| Command dispatch | 1,700-line match | Registry + per-command functions |
| Key routing | 12-layer cascade | Stack-based (top gets input) |

### Parallel Systems

- `setup.rs` (3,768 lines) has its own TUI with ratatui rendering — completely separate from `tui.rs`
- Profile overlay in tui.rs duplicates much of what setup.rs already does
- Both have provider pickers, model pickers, API key inputs
- **Consider**: can profile create in TUI reuse setup.rs components? Or vice versa?

### Where a Unified Abstraction Reduces Most Complexity

**Biggest ROI: Overlay stack + ListPicker + TextInput**
- Eliminates: 13 Option fields, 12-layer cascade, 3 text editing copies, 5 picker copies
- Estimated reduction: ~3,000 lines from tui.rs
- Makes adding new overlays trivial (implement trait, push to stack)

---

## 17. Documentation Updates

### User-Facing

- [ ] Update `/help` output to show key bindings per context
- [ ] Add `/keys` command (or section in /help) for current mode's shortcuts
- [ ] Document `CLIDO_USER_AGENT` env var in README
- [ ] Document credentials file format and location
- [ ] Update profile docs: create, edit, switch, delete flows

### Internal / Developer

- [ ] Architecture doc: overlay stack pattern, how to add new overlays
- [ ] Style guide: interaction rules (ESC = dismiss, Enter = confirm, etc.)
- [ ] Module map: which file owns what responsibility
- [ ] State diagram: overlay lifecycle (create → active → complete → pop)

---

## 18. Testing Strategy

### Critical Paths That Must Be Covered

1. **Profile lifecycle**: create → save → switch → use → edit → delete
2. **Command execution**: each slash command's happy path
3. **Overlay lifecycle**: open → interact → dismiss (for each overlay type)
4. **API key resolution**: env var → credentials file → config → error
5. **Provider construction**: correct user_agent, base_url, headers per provider
6. **Text input**: insert, delete, paste, word-jump, cursor bounds

### Regression Strategy

- **Snapshot tests** for rendered output (ratatui has `TestBackend`)
- **State-machine tests** for overlay transitions (unit tests on state structs)
- **Integration tests** for command execution (mock provider, verify state changes)
- **Property tests** for text editing (arbitrary strings, cursor positions)

### Current Gaps

- No tests for overlay interaction (key → state change → render)
- No tests for paste routing
- No tests for command side effects
- setup.rs has tests for TOML generation but not for the TUI flow itself

### TODOs
- [ ] Add `TestBackend` snapshot tests for main render + each overlay
- [ ] Add state-machine tests for ProfileOverlay transitions
- [ ] Add integration tests for top-10 most-used slash commands
- [ ] Add text input property tests (insert at any position, delete at bounds)

---

## System-Wide Simplification Principles

1. **One interaction per concept.** If the user needs to pick from a list, there's one `ListPicker`. Not five.
2. **Stack, not flags.** Overlays are a stack. Input goes to top. ESC pops. No mode flags.
3. **Extract, don't duplicate.** Text editing, list picking, modal rendering — write once, use everywhere.
4. **Registry, not match.** Commands are data (name, handler, keybind). Dispatch is a table lookup.
5. **Structured, not strings.** Errors have types, recovery actions, and severity — not just a string.
6. **Reload after save.** Any config change must take effect immediately without restart.
7. **Small files.** No file over 2,000 lines. If it's bigger, it has hidden abstractions waiting to be extracted.

---

## Consistency Rules (Global Interaction Model)

| Rule | Description |
|------|-------------|
| **ESC = Dismiss** | Always closes/cancels the topmost overlay |
| **Enter = Confirm** | Always submits/selects/confirms |
| **↑↓ = Navigate** | Always moves selection in lists |
| **Typing = Context** | Filters in lists, edits in text fields |
| **Paste = Active field** | Always goes to the focused text input |
| **Ctrl+C = Interrupt** | Stops agent, or if idle, asks to quit |
| **Mode indicator** | Status bar always shows current context |

---

## Full TODO Roadmap

### High Priority (Architectural Foundation)

| # | Task | Impact | Est. Lines Changed |
|---|------|--------|-------------------|
| H1 | Define `Overlay` trait + `OverlayStack` | Eliminates 12-layer cascade | +200, -500 |
| H2 | Extract `TextInput` struct + `handle_text_key` | Eliminates 3× duplication | +250, -350 |
| H3 | Extract `ListPicker<T>` generic overlay | Eliminates 5× duplication | +300, -600 |
| H4 | Break `execute_slash` into command registry | 1,700 → 50 lines dispatch | +400, -1,600 |
| H5 | Split tui.rs into modules | 11,670 → 5–8 files | ±0 (restructure) |
| H6 | Group App fields into sub-structs | 86 fields → ~25 | +50, -30 |
| H7 | Fix config reload after profile save | Broken flow | +30 |

### Medium Priority (UX Polish)

| # | Task | Impact |
|---|------|--------|
| M1 | Mode indicator in status bar | Users always know their context |
| M2 | Structured error types with recovery | Better error UX |
| M3 | Model picker categories (favorites, recent) | Faster model selection |
| M4 | PageUp/PageDown in message history | Power user navigation |
| M5 | `/keys` command showing current bindings | Discoverability |
| M6 | Unify setup.rs pickers with TUI overlays | Reduce parallel systems |

### Low Priority (Nice to Have)

| # | Task | Impact |
|---|------|--------|
| L1 | Toast system for transient notifications | Non-blocking feedback |
| L2 | Click-to-focus on overlays | Mouse UX |
| L3 | Leader-key mode (Vim-style) | Power user speed |
| L4 | Snapshot tests for all overlays | Regression safety |
| L5 | Property tests for TextInput | Edge case coverage |

---

## Risks and Tradeoffs

| Risk | Mitigation |
|------|-----------|
| **Big refactor breaks things** | Do incrementally: extract TextInput first (lowest risk), then ListPicker, then OverlayStack |
| **Overlay stack may not fit all cases** | Plan editor is full-screen, not a modal. Keep it as a separate "view mode" alongside the stack. |
| **Command registry adds indirection** | Worth it — 1,700 lines of inline code is far worse than a lookup table |
| **Splitting files loses grep-ability** | Use clear module names and re-export from `tui/mod.rs` |
| **Generic ListPicker may not fit all pickers** | Use trait bounds for item display; allow custom rendering via generic parameter |
| **Tests will break during refactor** | Existing tests are mostly unit tests on data structures — they'll survive. Add overlay tests after refactor. |

---

*Generated from analysis of: Codex (Rust/ratatui), Gemini CLI (TS/Ink), OpenCode (TS/SolidJS),
Goose (Rust/cliclack), Aider (Python/prompt_toolkit), Open Interpreter (Python/Rich), and Clido itself.*

---
---

# Part 2: Concrete Menu Design & Menu Flows

> Focused analysis of menu types, step-by-step flows, navigation, transitions,
> and consistency — with concrete directions for simplification.

---

## 1. Menu Types

### What Exists Across Tools

| Type | Codex | Gemini | OpenCode | Clido |
|------|-------|--------|----------|-------|
| **List picker** (select one from list) | ListSelectionView | BaseSelectionList | DialogSelect | 6 separate implementations |
| **Text input** (type a value) | TextArea widget | Ink TextInput | DialogPrompt | 4 separate inline implementations |
| **Confirmation** (yes/no/choice) | Approval buttons | — | DialogConfirm | Permission modal (5-choice) |
| **Form** (multi-field edit) | — | AskUserDialog (tabs) | — | Settings overlay, Plan task form |
| **Read-only display** | — | — | — | Rules overlay, Error modal |
| **Full-screen editor** | — | — | — | Plan text editor |

### How Many Types Are Actually Needed?

**Four.** Every menu in every tool reduces to one of these:

1. **Picker** — Choose one item from a filterable list.
   Used for: models, providers, profiles, sessions, roles, commands, plan tasks.

2. **Text field** — Type or paste a single value.
   Used for: profile name, API key, base URL, search filter, feedback text.

3. **Choice** — Pick from 2–5 labeled actions.
   Used for: permissions (allow/deny/…), confirmations (save/discard), errors (OK).

4. **Editor** — Free-form multi-line text editing.
   Used for: plan text, (future: system prompt editing, message editing).

Clido currently has **11 overlays** that are really just combinations of these 4 primitives.
The problem isn't the number of menus — it's that each one re-implements the primitives from scratch.

---

## 2. Menu Flows (Step-by-Step)

### Flow 1: Create Profile

**Competitors:**

| Tool | Steps | Flow |
|------|-------|------|
| Codex | 2 | Pick provider → Pick model (API key from env, no manual entry) |
| OpenCode | 3 | Pick provider → Pick auth method → Enter API key (model auto-detected) |
| Gemini | 1 | Config file only (no in-app creation) |

**Clido (current): 4 steps**
```
[Text field] Name → [Picker] Provider → [Text field] API Key → [Picker] Model
```

Each step is a different mode inside one monolithic `ProfileOverlayState` FSM.
ESC goes back one step. Enter advances. The model list is fetched async during step 3.

**Problems:**
- 4 steps feels heavy for something done once per provider
- The wizard is a 5-mode FSM inside a single overlay — complex code for a linear flow
- If model fetch fails, the user is stuck on step 4 with no models and must type manually
- "Skip API key for local providers" is a special case that breaks the linear expectation

**Simpler approach (3 steps, reusing primitives):**
```
[Picker] Provider → [Text field] API Key (skip if local) → [Picker] Model
```
- Name = auto-generated from provider (e.g. "anthropic", "openai-2") — editable later
- Each step is a separate overlay pushed onto the stack (not modes in one FSM)
- If model fetch fails → show picker with empty list + text fallback: "Type model ID manually"

### Flow 2: Switch Model

**Competitors:**

| Tool | Steps | Flow |
|------|-------|------|
| Codex | 1 | Open picker → select → done |
| OpenCode | 1 | Open picker → type to filter → select → done |
| Gemini | 1 | Open picker → select → done |

**Clido: 1 step** ✓ (already good)
```
/models → [Picker with filter + favorites + pricing] → Enter to select
```

This works well. The model picker is clido's best menu — filterable, shows metadata, supports favorites. Only issue: no categories (favorites/recent/all sections) like OpenCode.

### Flow 3: Switch Profile

**Clido: 1 step** ✓
```
/profile → [Picker] → Enter to switch
```

Works, but profile switch currently **quits the TUI** to reload config. Competitors handle this in-place.

### Flow 4: Approve Tool Use

**Codex: 1 step** — buttons inline (Approve / Deny / Always)
**Clido: 1–2 steps**
```
[Choice] Allow once / Session / All / Deny / Deny+feedback
  └─ if "Deny with feedback" → [Text field] feedback → Enter
```

The 5-option choice is reasonable. The nested feedback text field is a nice touch.
**Problem:** the feedback text input is a reduced editor (no cursor movement, no Delete key, no paste) — feels broken compared to the profile text input which is fully featured.

### Flow 5: Edit Profile Field

**Clido: 2 steps**
```
[Picker] Select field from overview → [Text field or Picker] Edit value
```

This is fine structurally. But:
- For provider/model fields, it opens nested sub-pickers *within* the overlay
- For text fields, the edit mode is inline with different keybindings than the create wizard
- Ctrl+S saves in overview mode but Enter saves in edit mode — inconsistent

### Flow 6: Manage Roles (Settings)

**Clido: 2–3 steps**
```
/settings → [Form] Navigate fields → Enter to edit → [Text field] type value → Enter to save
```

**Problem:** This is the only form-style menu in the app. It uses a completely unique interaction model:
- Up/Down to navigate fields (like a picker, but it's a form)
- Enter to start editing a field
- `n`/`d`/`s` as single-key shortcuts (unique to this overlay)
- Text editing is minimal (no cursor movement)

**Simpler:** Roles don't need a custom form. A role is just (name, model). Use:
```
/roles → [Picker] select role or "Add new" → [Text field] name → [Picker] model
```
Same primitives as everything else.

---

## 3. Enter / Exit Behavior

### How Users Open Menus

| Trigger | Examples |
|---------|---------|
| Slash command | `/models`, `/profile`, `/sessions`, `/roles`, `/settings`, `/rules`, `/plan edit` |
| Automatic | Permission modal (agent requests tool), Error modal (operation fails) |
| Keybind within picker | `n` in profile picker → create, `e` → edit |

**Problem:** There's no global keybind to open any menu. Everything goes through slash commands or is triggered automatically. Competitors use Ctrl+M for models, Ctrl+K for command palette, etc.

### How Users Exit (ESC behavior)

| Overlay | ESC does |
|---------|----------|
| Profile wizard | Goes back one step (step 1 → closes) |
| Profile overview | Closes |
| Profile edit field | Cancels edit → overview |
| All pickers | Closes |
| Permission normal | Defaults to Deny |
| Permission feedback | Returns to choice mode |
| Error modal | Closes |
| Settings overview | Closes (without saving!) |
| Settings edit | Cancels edit → overview |
| Plan editor | Closes (saves) |
| Plan text editor | Saves and closes |
| Rules | Closes |
| Slash completion | Closes menu |

**Inconsistencies:**
- Settings ESC closes **without saving** — data loss risk
- Plan text editor ESC **saves** and closes — opposite of Settings
- Profile wizard ESC goes **back** — different from all pickers which just close

**Principle from competitors:** ESC should always mean "dismiss without side effects." Save should be explicit (Enter or Ctrl+S).

### Predictability Assessment

Users can predict ESC = close for simple overlays (pickers, error, rules).
Users **cannot** predict ESC behavior for complex overlays (wizard step-back, settings no-save, plan auto-save). These need to be made consistent.

---

## 4. Navigation Inside Menus

### Selection (Pickers)

All 6 clido pickers use the same pattern:
- **↑/↓**: Move selection (wraps at boundaries)
- **Enter**: Confirm selection
- **Visual**: `▶` marker + highlighted row

This is **consistent and correct**. Matches all competitors.

Where they differ:
- Model picker: has filtering (type to narrow), Home/End, favorites toggle (F)
- Session picker: no filtering
- Profile picker: has `n`/`e` shortcuts (unique)
- Role picker: bare minimum (navigate + select only)

**Problem:** Filtering should be available in all pickers, not just models. Session picker with 50+ sessions and no filter is painful.

### Text Editing

Capabilities vary wildly across overlays:

| Capability | Profile overlay | Perm feedback | Settings edit | Plan text editor | Main input |
|-----------|:-:|:-:|:-:|:-:|:-:|
| Insert char | ✓ | ✓ | ✓ | ✓ | ✓ |
| Backspace | ✓ | ✓ | ✓ | ✓ | ✓ |
| Delete | ✓ | ✗ | ✗ | ✓ | ✓ |
| ←/→ cursor | ✓ | ✗ | ✗ | ✓ | ✓ |
| Home/End | ✓ | ✗ | ✗ | ✓ | ✓ |
| Ctrl+Backspace | ✓ | ✗ | ✗ | ✗ | ✓ |
| Paste | ✓ | ✗ | ✗ | ✓ | ✓ |

This is the **biggest UX inconsistency** in clido. Users will try to move their cursor in any text field. When it doesn't work in some fields, it feels broken.

**Fix:** One `TextInput` component used everywhere. All text fields get full editing.

### Focus

Focus is implicit — the topmost overlay gets all input. This works because there's never ambiguity about what's focused (only one overlay can be active).

**Problem:** Within a form (Settings, Plan task), focus between fields uses different keys:
- Settings: Enter to start editing, ESC to stop
- Plan task: Tab to cycle fields

Should be one pattern everywhere.

---

## 5. User Guidance

### How Menus Show Available Actions

**Competitors:**
- Codex: `"space to toggle · enter to confirm · esc to cancel"` — bottom of popup, dimmed
- OpenCode: `"esc"` in top-right corner + custom keybinds at bottom
- Gemini: Footer line changes based on context

**Clido:**
- Pickers: Title bar contains hints like `(↑↓ navigate  Enter=resume  Esc=close)`
- Permission: Bottom hint line: `↑↓/1-5 select   Enter confirm   Esc deny`
- Plan editor: Full hint bar at bottom
- Error modal: Inline text: `[ OK ]  (Enter / Esc / Space)`

**Assessment:** Clido shows hints — good. But the placement and format varies:
- Sometimes in the title bar (pickers)
- Sometimes at the bottom (permission, plan)
- Sometimes inline (error)

**Simplification:** All overlays should show hints in the **same position** (bottom of overlay frame). Format: `action=key · action=key · action=key`

---

## 6. Consistency Across Menus

### Consistent Patterns ✓

| Pattern | All menus? |
|---------|-----------|
| ↑/↓ navigates | ✓ All pickers |
| Enter confirms | ✓ All menus |
| ESC dismisses | ✓ All menus (with caveats) |
| `▶` marks selection | ✓ All pickers |
| Scroll indicators | ✓ All long lists |

### Inconsistent / Special Cases ✗

| Behavior | Where it breaks |
|----------|----------------|
| **Text editing features** | Varies per overlay (see §4 table) |
| **ESC side-effects** | Settings discards, Plan saves, Wizard steps back |
| **Filter/search** | Only in Model picker and slash completion |
| **Number shortcuts** | Only in Permission modal (1-5) |
| **Letter shortcuts** | Only in Profile picker (n/e) and Settings (n/d/s) |
| **Tab** | Only in Plan task form |
| **Space** | Error modal dismiss + Plan task status toggle (different meanings) |
| **Ctrl+S** | Profile overview save + Plan text editor save (at least these agree) |
| **Hint placement** | Title bar vs bottom vs inline |
| **Favorite toggle (F)** | Only in Model picker |

---

## 7. Menu Transitions

### How Tools Chain Menus

**Codex:** Queue-based. One approval at a time. When resolved → auto-advance to next. No nesting.

**OpenCode:** `dialog.replace()` — swaps current dialog content without closing the modal frame.
```
Provider picker ──replace──▶ Auth picker ──replace──▶ Key input ──clear──▶ Done
```
Feels like one continuous flow even though it's 3 different dialogs.

**Clido:** Mode-switching within a single overlay struct.
```
ProfileOverlayMode::Creating(step1) → Creating(step2) → Creating(step3) → Creating(step4)
```
All steps live inside one `ProfileOverlayState` — ESC goes back by decrementing step.

### Problem with Clido's Approach

Mode-switching means the overlay must handle ALL possible states and ALL transitions.
The Profile overlay handles: Overview + 4 wizard steps + EditField(9 variants) + PickingProvider + PickingModel = **16 distinct states** in one struct.

### Better Pattern: Sequential Stack Push

```
Push: ProviderPicker → user selects → pop, push: TextInput(API key) → user enters → pop, push: ModelPicker → user selects → pop → done
```

Each overlay is simple (one job). The stack manages transitions.
ESC always pops (dismiss current step). No step-back FSM needed — if user wants to change provider, they re-run the flow.

**Tradeoff:** User can't go "back" to a previous step (they start over). Competitors don't support back either — Codex and OpenCode both use forward-only flows. This is fine for short (2–3 step) wizards.

---

## 8. Complexity of Common Tasks

### Step Count Comparison

| Task | Codex | OpenCode | Clido |
|------|-------|----------|-------|
| Switch model | 1 (picker) | 1 (picker) | 1 (picker) ✓ |
| Create profile | 2 | 3 | 4 (name+provider+key+model) |
| Edit profile field | — | — | 2 (select field + edit) |
| Approve tool | 1 | — | 1 (choice) ✓ |
| Deny with reason | 2 | — | 2 (choice + text) ✓ |
| Switch session | 1 | 1 | 1 ✓ |
| Add role | — | — | 3 (settings → navigate → type name → type model) |
| Change default model | 1 | 1 | 2 (settings → edit field) |

### Where Clido Is Unnecessarily Deep

1. **Create profile (4 steps):** Name should be auto-generated. Reduces to 3 steps (provider → key → model) or even 2 if model auto-selects the provider's default.

2. **Add role (3 steps through Settings form):** Should be 2 steps: `/role add` → text input for name → model picker. No need to navigate through the Settings overlay.

3. **Change default model (2 steps through Settings):** `/model set` or Ctrl+S in model picker already does this — the Settings route is redundant.

### Where Clido Is Already Lean
- Model switching: 1 step, great UX
- Session switching: 1 step, fine
- Permission approval: 1 step, 5 clear choices

---

## 9. Mapping to Our System

### Current Menu Inventory (11 overlays)

| Overlay | Primitive Type | Can Simplify? |
|---------|---------------|---------------|
| **Model picker** | Picker (with filter) | ✓ Keep — best menu in the app |
| **Session picker** | Picker (no filter) | ✓ Add filter, otherwise keep |
| **Profile picker** | Picker + shortcuts | ✓ Keep, move n/e to slash commands |
| **Role picker** | Picker (bare) | ✓ Keep, add filter |
| **Profile create wizard** | FSM (4 modes) | **Simplify → 3 sequential overlays** |
| **Profile overview/edit** | Form + nested pickers | **Simplify → picker + editor overlays** |
| **Settings** | Custom form | **Remove → use /role commands instead** |
| **Permission modal** | Choice (5 options) | ✓ Keep |
| **Error modal** | Read-only + dismiss | ✓ Keep |
| **Rules overlay** | Read-only + dismiss | ✓ Add scrolling |
| **Plan editor** | Custom form + task list | Keep (domain-specific) |
| **Plan text editor** | Full editor | Keep (domain-specific) |
| **Slash completion** | Filtered picker (auto) | ✓ Keep |

### What Can Be Merged or Removed

**MERGE into generic Picker:**
- Session picker + Model picker + Profile picker + Role picker + Provider picker + any future list selection
- One `ListPicker<T>` with: filter, scroll, favorites (optional), categories (optional)

**MERGE into generic TextInput:**
- Profile name/key/url fields + Permission feedback + Settings field editing + future text fields
- One `TextInput` with: cursor, paste, word-delete, mask (for keys)

**MERGE into generic Choice:**
- Permission modal + Error modal + any future confirmation
- One `ChoiceOverlay` with: N labeled options, optional text input for one option

**REMOVE:**
- **Settings overlay** — replace with `/role add`, `/role delete`, `/model default` commands that use generic pickers/inputs
- **Profile overview form** — replace with a picker showing fields, then appropriate editor overlay per field

### Target: 4 Reusable Primitives + 2 Domain-Specific Editors

```
┌─────────────────────────────────────────────────────┐
│  PRIMITIVES (reusable across all flows)             │
│                                                     │
│  1. ListPicker    — filter, scroll, select one      │
│  2. TextInput     — type, paste, cursor, mask       │
│  3. ChoiceOverlay — pick from 2–5 labeled actions   │
│  4. ReadOnly      — display text, dismiss           │
│                                                     │
│  DOMAIN-SPECIFIC (unique interaction models)        │
│                                                     │
│  5. PlanEditor    — task list + inline editing       │
│  6. PlanTextEditor — nano-style text editor          │
│                                                     │
│  AUTOMATIC (not user-triggered)                     │
│                                                     │
│  7. SlashCompletion — appears when typing /          │
└─────────────────────────────────────────────────────┘
```

Every flow in the app is composed from these 7 components. No more.

### Flow Redesign Using Primitives

**Create Profile:**
```
ListPicker(providers) → TextInput("API Key", masked) → ListPicker(models)
                         ↑ skipped for local providers
Name auto-generated as "{provider}" or "{provider}-2" if duplicate.
Editable later via /profile rename.
```

**Edit Profile:**
```
ListPicker(fields: provider, api_key, model, base_url, name, ...) → TextInput or ListPicker depending on field type
```

**Manage Roles:**
```
/role             → ListPicker(roles + "Add new")
/role add         → TextInput("Role name") → ListPicker(models)  
/role delete      → ListPicker(roles) → ChoiceOverlay("Delete {name}?")
/role set <name>  → activates role (no menu)
```

**Approve Tool:**
```
ChoiceOverlay(5 options) → if "Deny with feedback" → TextInput("Reason")
```

---

## Summary: The Simplest Menu System

### Universal Rules (No Exceptions)

| Rule | Meaning |
|------|---------|
| **ESC = dismiss** | Always closes/cancels. Never saves. Never has side effects. |
| **Enter = confirm** | Selects item, submits text, or acknowledges. |
| **↑/↓ = navigate** | In pickers and choices. |
| **Typing = filter or edit** | Depends on context: picker → filter, text field → edit. |
| **Paste = active text field** | Always routes to whatever text input is focused. |
| **Hints at bottom** | Every overlay shows available keys in the same position and format. |

### Consistency Checklist

Every text input supports: insert, backspace, delete, ←/→, Home/End, Ctrl+Backspace, paste.
Every picker supports: ↑/↓, filter by typing, Enter to select, ESC to close.
Every choice supports: ↑/↓, number keys, Enter to confirm, ESC to cancel/default.
Every overlay shows: title at top, key hints at bottom.

### Where Our Current Menus Are Confusing

1. **Text editing inconsistency** — the #1 issue. Users paste in profile overlay but can't paste in permission feedback. Users move cursor in main input but can't in Settings edit.

2. **Settings overlay** — a unique form interaction model used nowhere else. Replace with commands that reuse pickers and text inputs.

3. **Profile wizard FSM** — 16 states in one overlay. Replace with 3 sequential stack pushes.

4. **ESC ambiguity** — saves in Plan text editor, discards in Settings, steps back in wizard. Should always just dismiss.

5. **Filter availability** — Model picker filters beautifully. Session picker with 50 sessions doesn't filter at all.

### Priority Actions

| Priority | Action | Impact |
|----------|--------|--------|
| **P0** | Unify text input into `TextInput` struct | Fixes paste, cursor, delete everywhere |
| **P0** | Make ESC always dismiss (no side effects) | Predictable navigation |
| **P1** | Create generic `ListPicker<T>` with filter | Replaces 6 picker implementations |
| **P1** | Replace profile wizard FSM with stacked overlays | Simpler code, same UX |
| **P2** | Remove Settings overlay → use /role commands | Fewer concepts to learn |
| **P2** | Add filter to Session picker | Better UX at scale |
| **P3** | Standardize hint placement (always bottom) | Visual consistency |
| **P3** | Add categories to Model picker (favorites/recent/all) | Better organization |
