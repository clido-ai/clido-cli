# Clido TUI Refactoring Plan

> Concrete, ordered implementation plan derived from opus-anal.md.
> Covers every identified issue — from critical architectural debt to polish.
>
> **Design identity:** Clido is not a Codex clone, not a React port, not a REPL.
> It's a profile-first, pricing-aware coding agent with a lean TUI.
> We take principles from competitors but build our own idioms.

---

## Design Principles (What Makes Clido Clido)

Before touching code, lock these down. Every decision below follows from these.

1. **Profiles are first-class.** No other agent has named profiles with per-profile
   providers, keys, models, worker/reviewer configs. This is our differentiator.
   Profiles should feel instant and effortless to create, switch, and manage.

2. **Pricing is visible.** The model picker shows cost, context window, and aliases.
   No other agent does this. Lean into it — make it the best model picker in any CLI.

3. **Four primitives, zero custom forms.** Every overlay is a Picker, TextInput,
   Choice, or ReadOnly. No special-case form widgets. If it needs a form, decompose
   into sequential overlays.

4. **Commands are data, not code.** The slash command system is a lookup table.
   Adding a command means adding one struct to a registry, not touching dispatch logic.

5. **One way to do things.** Text editing works the same everywhere. Pickers filter
   the same everywhere. ESC always dismisses. No "but in this overlay it's different."

6. **Lean means lean.** Target: under 8,000 total TUI lines across all files.
   Currently at 15,438 (tui.rs + setup.rs). That's almost 2x our target.

---

## Phase 0 — Foundation (CRITICAL)

These are blocking prerequisites. Everything else depends on them.
Do them in order — each builds on the previous.

### 0.1 · Extract `TextInput` struct

**Problem:** Text editing logic is copy-pasted 3–4 times with different feature sets.
Permission feedback can't even move the cursor. This is the #1 UX bug.

**What to build:**
```
struct TextInput {
    text: String,
    cursor: usize,          // byte offset
    mask: Option<char>,     // '●' for API keys
    placeholder: String,
    history: Vec<String>,   // optional, for main chat input
    history_idx: Option<usize>,
}
```

Methods: `insert_char`, `delete_back`, `delete_forward`, `word_left`, `word_right`,
`home`, `end`, `paste`, `clear`, `set_text`, `cursor_left`, `cursor_right`.

One function: `handle_text_key(input: &mut TextInput, key: KeyEvent) -> bool`
Returns true if the key was consumed.

**Where it replaces existing code:**
- `app.input` / `app.cursor` / `app.input_history` in main chat (handle_key ~200 lines)
- `st.input` / `st.input_cursor` in profile overlay (handle_profile_overlay_key ~100 lines)
- Permission feedback inline text handling (~50 lines)
- Settings field editing (~30 lines)

**What NOT to do:**
- Don't make this a ratatui Widget yet. It's a data struct with methods.
- Don't add selection/clipboard — keep it minimal. Paste comes from terminal events.
- Don't add multi-line. That's the PlanTextEditor's job.

**File:** `crates/clido-cli/src/text_input.rs` (~250 lines)

**Verification:** Every text field in the app supports: insert, backspace, delete,
←/→, Home/End, Ctrl+Backspace (word delete), and paste. Test with a property test
that does random operations on random strings.

---

### 0.2 · Extract `ListPicker<T>` struct

**Problem:** 6 picker implementations with different features. Model picker has
filtering + favorites. Session picker has no filtering. Role picker is bare.

**What to build:**
```
struct ListPicker<T> {
    items: Vec<T>,
    filtered: Vec<usize>,  // indices into items
    selected: usize,        // index into filtered
    scroll_offset: usize,
    visible_rows: usize,
    filter: TextInput,      // reuse TextInput for filter!
    title: String,
    has_filter: bool,       // some pickers may not need filter
}

trait PickerItem {
    fn matches_filter(&self, filter: &str) -> bool;
    fn render_row(&self, selected: bool, width: u16) -> Line<'_>;
    fn columns() -> Option<Vec<(&'static str, u16)>>; // optional header
}
```

Methods: `move_up`, `move_down`, `home`, `end`, `apply_filter`, `selected_item`.

**Where it replaces existing code:**
- `ModelPickerState` + its rendering + its key handling
- `SessionPickerState` + rendering + keys
- `ProfilePickerState` + rendering + keys
- `RolePickerState` + rendering + keys
- Provider picker inside profile overlay
- Model picker inside profile overlay

**What makes ours unique (vs Codex/OpenCode):**
- `PickerItem` trait means any Rust struct can be picked — no type erasure needed
- Column headers are optional (roles don't need them, models do)
- Filter is a real `TextInput` — full editing, not just append/backspace
- Pricing data renders inline (our model picker's killer feature)

**File:** `crates/clido-cli/src/list_picker.rs` (~350 lines)

**Verification:** Model picker still shows pricing table, favorites, filter.
Session picker now has filtering. All pickers have Home/End, wrapping, scroll indicators.

---

### 0.3 · Define `Overlay` trait + `OverlayStack`

**Problem:** 13 `Option<XxxState>` fields checked in a 12-layer if-cascade for both
input routing and rendering. Paste routing was broken because there's no unified dispatch.

**What to build:**
```
trait Overlay {
    fn title(&self) -> &str;
    fn render(&self, frame: &mut Frame, area: Rect, hints: &mut Vec<Span>);
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction;
    fn handle_paste(&mut self, text: &str) -> bool { false }
}

enum OverlayAction {
    Consumed,                    // handled, stay open
    Dismiss,                     // close this overlay
    Push(Box<dyn Overlay>),      // open a sub-overlay
    Replace(Box<dyn Overlay>),   // swap this overlay for another
    AppAction(AppAction),        // trigger app-level side effect
}

enum AppAction {
    SwitchModel(String),
    SwitchProfile(String),
    ResumeSession(String),
    GrantPermission(PermGrant),
    RunCommand(String),
    ShowError(ErrorInfo),
    // ... one variant per app-level side effect
}
```

Stack in App:
```
struct App {
    overlays: Vec<Box<dyn Overlay>>,
    // remove: pending_perm, pending_error, settings, profile_overlay,
    //         session_picker, model_picker, profile_picker, role_picker,
    //         rules_overlay, plan_editor, plan_text_editor
}
```

**Rendering:** Render base UI. Then render each overlay bottom-to-top (topmost last).
Each overlay renders into a centered Rect (modal) or full Rect (editor).

**Input routing:** If `overlays.last_mut()` exists → route there.
Otherwise → main input. No if-cascade.

**Paste routing:** Same — `overlays.last_mut().handle_paste()` or main input.
Never breaks again.

**What makes ours different from Codex:**
- Codex has a view stack that's tightly coupled to their ChatWidget.
  Ours is generic — any `impl Overlay` can be pushed. No widget hierarchy.
- `OverlayAction::Replace` enables the sequential-wizard pattern (push provider picker,
  on select → replace with API key input, on enter → replace with model picker).
  Codex doesn't have this — they use separate view pushes.
- `AppAction` enum means overlays never touch App directly. Clean boundary.

**File:** `crates/clido-cli/src/overlay.rs` (~150 lines for trait + stack logic)

**Verification:** Push an error overlay, verify ESC dismisses. Push two overlays,
verify only topmost gets input. Verify paste routes correctly at all stack depths.

---

### 0.4 · Build concrete overlay implementations

Using the three primitives from 0.1–0.3, build the actual overlays:

**From primitives (thin wrappers):**

| Overlay | Built from | Lines (est.) |
|---------|-----------|-------------|
| `ModelPickerOverlay` | ListPicker<ModelRow> | ~80 (render columns + favorites) |
| `SessionPickerOverlay` | ListPicker<SessionRow> | ~50 |
| `ProfilePickerOverlay` | ListPicker<ProfileRow> | ~50 |
| `RolePickerOverlay` | ListPicker<RoleRow> | ~40 |
| `ProviderPickerOverlay` | ListPicker<ProviderRow> | ~40 |
| `TextInputOverlay` | TextInput + label + validation | ~60 |
| `ChoiceOverlay` | Vec<(label, action)> + selected | ~80 |
| `ErrorOverlay` | ErrorInfo + dismiss | ~40 |
| `ReadOnlyOverlay` | String + scrollable + dismiss | ~50 |

**Domain-specific (keep as-is, just implement Overlay trait):**

| Overlay | Current state | Migration effort |
|---------|--------------|-----------------|
| `PermissionOverlay` | `PermGrant` struct → impl Overlay | Medium (5-choice + feedback sub-mode) |
| `PlanEditorOverlay` | `PlanEditor` struct → impl Overlay | Medium (task list + inline edit) |
| `PlanTextEditorOverlay` | `PlanTextEditor` struct → impl Overlay | Low (already self-contained) |

**File:** `crates/clido-cli/src/overlays/` directory with one file per overlay type.

---

### 0.5 · Migrate handle_key and render to use OverlayStack

**What changes:**

`handle_key` goes from:
```rust
if app.plan_text_editor.is_some() { ... }
else if app.plan_editor.is_some() { ... }
else if app.profile_overlay.is_some() { ... }
// ... 10 more branches
else { handle_main_input(app, key) }
```

To:
```rust
if let Some(overlay) = app.overlays.last_mut() {
    match overlay.handle_key(key) {
        OverlayAction::Consumed => {}
        OverlayAction::Dismiss => { app.overlays.pop(); }
        OverlayAction::Push(o) => { app.overlays.push(o); }
        OverlayAction::Replace(o) => { *app.overlays.last_mut().unwrap() = o; }
        OverlayAction::AppAction(a) => { app.handle_action(a); }
    }
} else {
    handle_main_input(app, key);
}
```

`render` goes from 13 if-blocks to:
```rust
render_base_ui(frame, app, area);
for overlay in &app.overlays {
    overlay.render(frame, centered_area, &mut hints);
}
render_hint_bar(frame, hints, bottom_area);
```

**Remove from App:** all 13 `Option<XxxState>` fields.

**Add to App:** `overlays: Vec<Box<dyn Overlay>>`, `fn handle_action(&mut self, action: AppAction)`.

**This is the highest-risk step.** Do it after 0.1–0.4 are solid and tested.
The approach: migrate one overlay at a time, keeping both old and new systems running.
Start with ErrorOverlay (simplest), end with ProfileOverlay (most complex).

---

## Phase 1 — Command System (HIGH)

### 1.1 · Define command registry

**Problem:** `execute_slash()` is 1,700 lines — a 29-way match with inline everything.

**What to build:**
```
struct SlashCommand {
    name: &'static str,
    aliases: &'static [&'static str],
    category: CommandCategory,
    description: &'static str,
    usage: &'static str,           // e.g., "/model [name]"
    min_args: usize,
    execute: fn(ctx: &mut CommandContext, args: &str) -> CommandResult,
}

enum CommandCategory { Session, Model, Profile, Plan, Context, Git, System }

struct CommandContext<'a> {
    app: &'a mut App,
    // anything commands need access to
}

enum CommandResult {
    Ok(Option<String>),           // optional success message
    OpenOverlay(Box<dyn Overlay>),// command opens a picker/input
    Error(String),                // error message
    Quit,                         // exit the TUI
}
```

**Registry:**
```
static COMMANDS: &[SlashCommand] = &[
    SlashCommand { name: "help", aliases: &["?", "h"], category: System, ... execute: cmd_help },
    SlashCommand { name: "model", aliases: &["m"], category: Model, ... execute: cmd_model },
    // ... 29 entries
];
```

**What makes ours different:**
- Commands return `CommandResult` — they don't mutate App directly for overlays.
  They say "open this overlay" and the caller pushes it.
- `CommandContext` limits what commands can access (future: permission scoping).
- Categories are used for slash completion grouping (already grouped, just formalize it).

**Completions derived from registry:**
```rust
fn slash_completions(input: &str) -> Vec<(&str, &str)> {
    COMMANDS.iter()
        .filter(|c| c.name.starts_with(input) || c.aliases.iter().any(|a| a.starts_with(input)))
        .map(|c| (c.name, c.description))
        .collect()
}
```

No more hand-maintained completion list.

**File:** `crates/clido-cli/src/commands/mod.rs` (registry, ~100 lines)
**Files:** `crates/clido-cli/src/commands/session.rs`, `commands/model.rs`, etc. (one per category)

---

### 1.2 · Extract command implementations

Take each branch of the current 29-way match and move it to a function:

| Command | Target file | Complexity |
|---------|-------------|-----------|
| /help, /version, /keys | commands/system.rs | Low |
| /model, /models, /role, /roles | commands/model.rs | Medium |
| /profile, /profile new, /profile edit | commands/profile.rs | Medium |
| /session, /sessions, /resume, /clear | commands/session.rs | Medium |
| /plan, /plan edit, /plan text | commands/plan.rs | Medium |
| /add, /remove, /rules, /search | commands/context.rs | Low |
| /git, /diff, /status, /commit | commands/git.rs | Low |
| /export, /cost, /undo, /compact | commands/misc.rs | Low |

**Target:** `execute_slash()` becomes <50 lines:
```rust
fn execute_slash(app: &mut App, input: &str) {
    let (name, args) = split_command(input);
    let cmd = COMMANDS.iter().find(|c| c.name == name || c.aliases.contains(&name));
    match cmd {
        Some(c) => {
            let mut ctx = CommandContext { app };
            match (c.execute)(&mut ctx, args) {
                CommandResult::Ok(msg) => { if let Some(m) = msg { ctx.app.push_system_message(m); } }
                CommandResult::OpenOverlay(o) => { ctx.app.overlays.push(o); }
                CommandResult::Error(e) => { ctx.app.push_error(e); }
                CommandResult::Quit => { ctx.app.should_quit = true; }
            }
        }
        None => { app.push_error(format!("Unknown command: {name}")); }
    }
}
```

---

## Phase 2 — File Structure (HIGH)

### 2.1 · Split tui.rs into modules

**Target structure:**
```
crates/clido-cli/src/
├── tui/
│   ├── mod.rs          — re-exports, App struct definition
│   ├── app.rs          — App impl, event_loop, agent_task
│   ├── render.rs       — render() + render helpers (status bar, message bubbles)
│   ├── input.rs        — handle_key for main chat input (non-overlay)
│   ├── event_loop.rs   — event_loop() + tick handling
│   └── agent.rs        — agent_task(), streaming, tool calls
├── text_input.rs       — TextInput struct (from 0.1)
├── list_picker.rs      — ListPicker<T> (from 0.2)
├── overlay.rs          — Overlay trait + OverlayStack (from 0.3)
├── overlays/
│   ├── mod.rs
│   ├── model_picker.rs
│   ├── session_picker.rs
│   ├── profile_picker.rs
│   ├── role_picker.rs
│   ├── provider_picker.rs
│   ├── text_input_overlay.rs
│   ├── choice.rs
│   ├── error.rs
│   ├── read_only.rs
│   ├── permission.rs
│   ├── plan_editor.rs
│   └── plan_text_editor.rs
├── commands/
│   ├── mod.rs          — SlashCommand registry
│   ├── system.rs
│   ├── model.rs
│   ├── profile.rs
│   ├── session.rs
│   ├── plan.rs
│   ├── context.rs
│   ├── git.rs
│   └── misc.rs
```

**Line budget:**
| File | Max lines |
|------|----------|
| tui/mod.rs (App struct + re-exports) | 300 |
| tui/app.rs (App impl) | 500 |
| tui/render.rs | 1,200 |
| tui/input.rs | 300 |
| tui/event_loop.rs | 500 |
| tui/agent.rs | 600 |
| text_input.rs | 250 |
| list_picker.rs | 350 |
| overlay.rs | 150 |
| overlays/* (13 files) | ~50–100 each |
| commands/* (9 files) | ~50–200 each |

**Total:** ~6,500 lines (down from 11,670). Not by deleting features —
by eliminating duplication.

---

### 2.2 · Group App fields into sub-structs

**Current:** 86 flat fields.

**Target:**
```rust
pub struct App {
    // Core
    pub config: AppConfig,
    pub session: SessionState,
    pub agent: AgentState,
    pub ui: UiState,
    pub overlays: Vec<Box<dyn Overlay>>,

    // Channels
    pub agent_tx: Sender<AgentEvent>,
    pub agent_rx: Receiver<AgentEvent>,
    pub perm_tx: Sender<PermGrant>,
}

pub struct SessionState {
    pub id: String,
    pub messages: Vec<Message>,
    pub turn_count: usize,
    pub total_cost: f64,
    pub is_streaming: bool,
    // ...
}

pub struct AgentState {
    pub running: bool,
    pub current_tool: Option<String>,
    pub pending_tool_calls: Vec<ToolCall>,
    // ...
}

pub struct UiState {
    pub input: TextInput,
    pub scroll_offset: usize,
    pub following: bool,
    pub render_cache: RenderCache,
    pub status_message: Option<(String, Instant)>,
    pub known_models: Vec<ModelInfo>,
    // ...
}
```

**Rule:** Each sub-struct should have <20 fields. If it has more, split again.

---

## Phase 3 — Menu Flow Fixes (HIGH)

### 3.1 · Simplify profile creation flow

**Current:** 4-step FSM inside ProfileOverlayState (Name → Provider → Key → Model).

**New:** 3 sequential overlay pushes (no FSM):
```
1. Push ProviderPickerOverlay
   → on select: if provider needs key → push TextInputOverlay("API Key", masked)
                 else → push ModelPickerOverlay(provider)

2. TextInputOverlay returns key
   → push ModelPickerOverlay(provider, key)

3. ModelPickerOverlay returns model_id
   → auto-generate profile name ("{provider}" or "{provider}-2")
   → save profile to config + credentials
   → show success message in chat
```

**No name step.** Auto-name from provider. User can `/profile rename` later.
This matches how people actually think: "I want to add my Anthropic key" not "I want
to create a profile called work-claude."

**No FSM.** Three simple overlays. Each does one thing.

---

### 3.2 · Simplify profile editing flow

**Current:** 5-mode FSM (Overview → EditField → PickingProvider → PickingModel).

**New:** Two overlay pushes:
```
1. Push ListPicker<ProfileField> showing all editable fields with current values
   → on select field:

2a. If field is provider → push ProviderPickerOverlay
2b. If field is model → push ModelPickerOverlay
2c. If field is text (name, key, url) → push TextInputOverlay(current_value)
   → on confirm: save field, reload config if active profile
```

**No overview mode.** The field picker IS the overview.

---

### 3.3 · Replace Settings overlay with /role commands

**Current:** Unique form overlay for managing roles + default model. Custom interaction
model (n/d/s shortcuts, enter-to-edit, no cursor movement in text fields).

**Replace with:**
```
/roles           → ListPicker(roles + "➕ Add new")
/role add        → TextInput("Role name") → on confirm → ListPicker(models) → save
/role delete     → ListPicker(roles) → ChoiceOverlay("Delete {name}?") → delete
/role <name>     → switch to role's model immediately (no menu)
/model default   → ListPicker(models) → save as default in config
```

**Remove:** SettingsState, settings rendering, settings key handling, settings overlay entirely.

---

### 3.4 · Fix ESC consistency

**Rule:** ESC always dismisses. Never saves. Never has side effects.

**Changes needed:**
| Overlay | Current ESC | New ESC |
|---------|-------------|---------|
| Profile wizard step N | Go back to step N-1 | Dismiss (cancel flow) |
| Settings overview | Close without saving | N/A (removed) |
| Plan text editor | Save and close | Dismiss without saving |
| Plan editor | Close and save | Dismiss without saving |

**Plan editors:** Add `Ctrl+S = save` hint prominently. ESC = discard changes.
This matches nano, vim, and every other editor.

**Profile wizard:** No back-stepping. ESC cancels the whole flow.
If user wants to change provider, they start over (it's 3 steps, not 30).

---

### 3.5 · Fix config reload after profile save/switch

**Problem:** Profile overlay saves to disk but doesn't reload active config in memory.
Profile switch quits the TUI entirely.

**Fix:**
- After any profile save: if it's the active profile, rebuild the provider in-place.
  `app.provider = make_provider(&updated_profile)?;`
- After profile switch: don't quit. Update `app.config`, rebuild provider, refresh model list.
  Clear session if provider changed (new provider = new context).

---

## Phase 4 — UX Consistency (MEDIUM)

### 4.1 · Add filter to all pickers

**Current:** Only model picker and slash completion support filtering.

**Fix:** `ListPicker<T>` has filtering by default. Session picker, profile picker,
and role picker all gain type-to-filter. Implementation is free — it's built into
the ListPicker primitive.

---

### 4.2 · Mode indicator in status bar

**Current:** No visual indication of which overlay is active.

**Fix:** Status bar shows overlay title when overlays are active:
```
[Creating profile…]  or  [Select model]  or  [Permission required]
```

Implementation: `app.overlays.last().map(|o| o.title())` → render in status bar.

---

### 4.3 · Standardize hint placement

**Current:** Hints appear in title bar, bottom of popup, or inline — depending on overlay.

**Fix:** Every overlay renders hints in the **bottom border** of its frame:
```
┌─ Select Model ───────────────────────────────────────────┐
│  ...                                                      │
│  ...                                                      │
└─ ↑↓ navigate · type to filter · Enter select · Esc close ┘
```

The `Overlay` trait's `render` method receives `&mut Vec<Span>` for hints.
The OverlayStack renders them consistently.

---

### 4.4 · Structured error types

**Current:** `pending_error: Option<String>` — just a string, no context, no recovery.

**New:**
```rust
struct ErrorInfo {
    title: String,
    detail: String,
    recovery: Option<String>,     // "Press R to retry" or "Run /init to reconfigure"
    is_transient: bool,           // rate limit vs bad key
}
```

**Map API codes:**
| Code | Title | Recovery |
|------|-------|---------|
| 401 | Invalid API key | "Check your key with /profile edit" |
| 403 | Access denied | "This model may require a different plan" |
| 429 | Rate limited | "Wait a moment and try again (R to retry)" |
| 500+ | Server error | "Try again in a few seconds (R to retry)" |
| timeout | Request timed out | "Check your connection (R to retry)" |

---

### 4.5 · Add PageUp/PageDown to main viewport

**Current:** Only scroll wheel and arrow keys for message history.

**Fix:** PageUp/PageDown scrolls by (viewport_height - 2) lines.
Home scrolls to top. End scrolls to bottom (and re-enables following mode).

---

### 4.6 · Add /keys command

**Current:** Key bindings are discoverable only by reading source code.

**Fix:** `/keys` (or `/help keys`) shows a read-only overlay:
```
┌─ Keyboard Shortcuts ──────────────────────────────────┐
│                                                        │
│  CHAT                                                  │
│  Enter .............. Send message                     │
│  ↑/↓ ................ Scroll history                   │
│  Ctrl+C ............. Interrupt agent                  │
│  Ctrl+L ............. Clear screen                     │
│  /  ................. Open command menu                 │
│                                                        │
│  IN ANY OVERLAY                                        │
│  Esc ................ Close / Cancel                    │
│  Enter .............. Confirm / Select                  │
│  ↑/↓ ................ Navigate items                   │
│                                                        │
│  MODEL PICKER                                          │
│  F .................. Toggle favorite                   │
│  Ctrl+S ............. Set as default                    │
│                                                        │
└─ Esc close ────────────────────────────────────────────┘
```

Uses `ReadOnlyOverlay` with scroll. Free to implement once primitives exist.

---

## Phase 5 — Deduplication (MEDIUM)

### 5.1 · Unify setup.rs pickers with TUI overlays

**Problem:** setup.rs (3,768 lines) has its own ratatui rendering with provider pickers,
model pickers, and API key inputs — completely separate from tui.rs.

**Options:**
1. **Rewrite setup.rs to use the same overlay primitives.** Setup becomes a sequence
   of overlay pushes, rendered by the same engine.
2. **Keep setup.rs separate but import shared primitives.** Setup still has its own
   event loop but uses `ListPicker<T>` and `TextInput` for consistency.

**Recommendation:** Option 2 for now. Full unification (option 1) is a larger project
and setup.rs works. But import `TextInput` and `ListPicker` so the UX is consistent.

---

### 5.2 · Eliminate render duplication

**Current:** `modal_block()`, `modal_row_two_col()`, and similar render helpers are
used by some overlays but not others. Each overlay handles its own sizing.

**Fix:** The `Overlay` trait gets a default `render_frame` method that:
1. Calculates centered Rect based on `self.size_hint() -> (u16, u16)` (width%, height%)
2. Renders the border with `self.title()`
3. Renders hints in bottom border
4. Calls `self.render_content(frame, inner_area)` for the overlay-specific content

Overlays only implement `render_content`. Frame, centering, hints are automatic.

---

## Phase 6 — Polish (LOW)

### 6.1 · Model picker categories

**Current:** Flat list sorted by favorites → recent → rest (implicitly).

**New:** Section headers:
```
── ★ Favorites ──
  claude-sonnet-4-20250514    anthropic    $3.00    ...
  gpt-4.1                    openai       $2.00    ...
── Recent ──
  deepseek-chat               deepseek     $0.14    ...
── All Models ──
  ...
```

Implementation: `ListPicker` gains optional `category: Option<String>` on items.
Render inserts section dividers between categories.

---

### 6.2 · Toast system for transient notifications

**Current:** All feedback goes through modal (blocking) or chat line (easily missed).

**New:** Non-blocking toasts for transient info:
- "Model switched to claude-sonnet-4" (auto-dismiss after 3s)
- "Profile saved" (auto-dismiss after 2s)
- "Rate limited — retrying in 5s" (auto-dismiss when retry starts)

Implementation: `Vec<Toast>` in App, rendered as stack in top-right corner.
Each has a creation timestamp + duration. Tick handler removes expired toasts.

---

### 6.3 · Rules overlay scrolling

**Current:** Rules overlay displays all rules but doesn't support scrolling.
Long rule lists are cut off.

**Fix:** `ReadOnlyOverlay` supports ↑/↓/PageUp/PageDown scrolling.
Free once the ReadOnly primitive exists.

---

### 6.4 · Session picker enhancements

**Current:** Shows id, turns, cost, date, preview. No filter, no delete.

**Enhancements:**
- Type-to-filter by preview text (free from ListPicker)
- `d` to delete session (with ChoiceOverlay confirmation)
- Show relative time ("2 hours ago" instead of "2025-03-29 12:00")

---

### 6.5 · Global keybinds for common actions

**Current:** Everything via slash commands.

**Add:**
| Keybind | Action |
|---------|--------|
| Ctrl+M | Open model picker (same as /models) |
| Ctrl+P | Open profile picker (same as /profile) |
| Ctrl+K | Open command palette (fuzzy search all commands) |

These are standard across Codex, OpenCode, and VS Code. Users expect them.
Implementation: check for these in `handle_main_input` before checking text input.

---

## Phase 7 — Testing (ONGOING)

### 7.1 · TextInput property tests

Use `proptest` or `quickcheck`:
- Random string + random operations (insert, delete, cursor move) → never panics
- Cursor always within `0..=text.len()` (byte boundary)
- After paste, text contains pasted content at cursor position

---

### 7.2 · Overlay state machine tests

For each overlay type:
- Create → handle_key(ESC) → returns Dismiss
- Create → handle_key(Enter) → returns appropriate action
- Create → handle_key(↑/↓) → updates internal state correctly

---

### 7.3 · Snapshot tests for rendered output

Use ratatui's `TestBackend`:
- Render each overlay type → assert output matches snapshot
- Render with different terminal widths → no panics, no overflow

---

### 7.4 · Integration tests for commands

For each slash command:
- Create minimal App state → execute command → assert state changed correctly
- Execute with bad args → assert error message (not panic)

---

### 7.5 · End-to-end flow tests

- Create profile → switch → send message (mock provider) → receive response
- Open model picker → filter → select → verify model changed
- Permission modal → approve → verify tool call proceeds

---

## Phase 8 — Documentation (ONGOING)

### 8.1 · User-facing docs

- Update README: `CLIDO_USER_AGENT` env var, credentials file location
- Update `/help` output to include key bindings
- Add profile management guide (create, edit, switch, delete)

### 8.2 · Developer docs

- Architecture doc: overlay stack pattern, how to add new overlays
- Style guide: ESC = dismiss, Enter = confirm, hints at bottom
- Module map: which file owns what
- Contributing guide: where to add a new command, a new overlay

---

## Execution Order Summary

```
PHASE 0 (CRITICAL — do first, in order)
  0.1  TextInput struct                    ░░░░░░░░░░  ~250 lines
  0.2  ListPicker<T> struct                ░░░░░░░░░░  ~350 lines
  0.3  Overlay trait + OverlayStack        ░░░░░░░░░░  ~150 lines
  0.4  Concrete overlay implementations    ░░░░░░░░░░  ~600 lines
  0.5  Migrate handle_key + render         ░░░░░░░░░░  ~-3,000 lines

PHASE 1 (HIGH — unblocks all future commands)
  1.1  Command registry                    ░░░░░░░░    ~100 lines
  1.2  Extract command functions            ░░░░░░░░    ~-1,400 lines

PHASE 2 (HIGH — makes code maintainable)
  2.1  Split tui.rs into modules            ░░░░░░░░    restructure
  2.2  Group App fields into sub-structs    ░░░░░░░░    refactor

PHASE 3 (HIGH — fixes broken/confusing UX)
  3.1  Simplify profile creation            ░░░░░░      ~-300 lines
  3.2  Simplify profile editing             ░░░░░░      ~-200 lines
  3.3  Replace Settings with /role cmds     ░░░░░░      ~-200 lines
  3.4  Fix ESC consistency                  ░░░░░░      ~20 lines
  3.5  Fix config reload after save         ░░░░░░      ~50 lines

PHASE 4 (MEDIUM — UX consistency)
  4.1  Filter in all pickers                ░░░░        free (from ListPicker)
  4.2  Mode indicator in status bar         ░░░░        ~20 lines
  4.3  Standardize hint placement           ░░░░        ~30 lines
  4.4  Structured error types               ░░░░        ~100 lines
  4.5  PageUp/PageDown in viewport          ░░░░        ~30 lines
  4.6  /keys command                        ░░░░        ~50 lines

PHASE 5 (MEDIUM — deduplication)
  5.1  Unify setup.rs with TUI primitives   ░░░░        ~200 lines changed
  5.2  Eliminate render duplication          ░░░░        ~-100 lines

PHASE 6 (LOW — polish)
  6.1  Model picker categories              ░░          ~50 lines
  6.2  Toast system                         ░░          ~100 lines
  6.3  Rules overlay scrolling              ░░          free (from ReadOnly)
  6.4  Session picker enhancements          ░░          ~50 lines
  6.5  Global keybinds (Ctrl+M/P/K)         ░░          ~30 lines

PHASE 7 (ONGOING)
  7.1–7.5  Testing                          ░░░░░░░░    ~500 lines

PHASE 8 (ONGOING)
  8.1–8.2  Documentation                    ░░░░        prose
```

**Net effect:** ~15,400 lines → ~6,500 lines. Same features. Better UX.
Every text field works the same. Every picker filters. ESC always closes.
Adding a new command = one function + one registry entry.
Adding a new overlay = one struct + `impl Overlay`.
