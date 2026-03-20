# Clido UX Requirements and Copy Standards

**Purpose:** Single source of truth for all user-facing text, layout, and interaction behavior. Every interactive moment has defined copy and clear input instructions. Implementation and tests must follow this document; the [CLI interface specification](cli-interface-specification.md) references it for §4 (First-Run) and for consistency with §5–§8.

**Scope:** Interactive prompts, first-run/init flow, permission prompts, empty states, error hints, and any script or wrapper that launches an interactive Clido command. Applies to both **functional** behavior (correct, unambiguous) and **visual** presentation (clear structure, consistent symbols, readable layout — "hübsch" as well as functional).

**Reference style:** Structure and rigor follow [REPORT.md](../REPORT.md) and [ARTIFACTS.md](../ARTIFACTS.md): evidence-backed where applicable; every user-visible string is specified so implementors and QA can verify against this doc.

---

## 1. Principles

1. **No silent waiting.** Whenever the process is waiting for user input, the user must see either (a) a prompt that explicitly states what to type and how to confirm (e.g. "Type 1 or 2 and press Enter"), or (b) a one-line notice before the prompt (e.g. "You will be asked two questions in this terminal. Type your answers and press Enter."). The program must never appear to hang without explanation.

2. **Every interactive prompt has defined copy.** Exact or template text is specified below for: first-run/init, permission, project-instruction trust, REPL, and any yes/no or choice prompt. Defaults are shown in brackets; the prompt must state the default (e.g. `[1]` or `[Y/n]`).

3. **Functional and visually clear.** Output uses consistent symbols, spacing, and (in TTY) light structure (e.g. a simple box for permission, a short header for setup). No walls of unbroken text. ASCII fallback preserves meaning; see terminal behavior matrix in CLI spec §5.

4. **Scripts that run interactive commands** (e.g. `scripts/run-in-test-env.sh` invoking `clido init`) must print a one- or two-line intro *before* starting the subprocess, stating that questions will follow and that the user should type answers in the same terminal. Example: see §3.2 below.

5. **Accessibility and portability.** Status and choices are conveyed by text and symbols, not by color alone. Copy uses only ASCII in non-TTY or when `NO_COLOR=1` / narrow terminal; Unicode symbols (e.g. ✓, ·) are optional in rich TTY mode. See CLI spec §16.

---

## 2. First-Run and `clido init` — Interactive Setup

### 2.1 When this flow runs

- **First-run:** No config file exists, user runs `clido` (with or without a prompt), stdin is a TTY → show setup header and run this flow, then write config and continue.
- **Explicit init:** User runs `clido init` (TTY or piped stdin for automation) → same flow, write config, then exit.

### 2.2 Header and one-time notice

Before any question, print a short header so the user knows what is happening.

**Rich TTY (preferred when stdin or stderr is a TTY and width ≥ 60):**

Before the box, print a welcome line (e.g. "Welcome to Clido." in bold/accent, "Let's set up your environment." in dim). Then a blank line, then the setup box. Production implementation uses a double-line box:

```
  Welcome to Clido. Let's set up your environment.

╔═══════════════════════════════════════════════════════════════╗
║  Clido setup                                                    ║
║  Choose a provider and where to store your API key.             ║
║  Answer the questions below; use arrow keys or type, then Enter.  ║
║  Defaults are in brackets.                                       ║
╚═══════════════════════════════════════════════════════════════╝
```

**Box alignment:** Each line between the vertical bars must have exactly the same character width (e.g. 59) so the right border aligns. Implementation uses a fixed inner width and pads each line.

**Rich TTY interaction:** When stdin/stderr is a TTY, provider choice and yes/no prompts use **arrow-key selection** (e.g. via inquire): user can move with ↑/↓ and confirm with Enter, or type and Enter. Non-TTY falls back to "Type 1 or 2, then press Enter" and read_line.

Alternative (single-line box): `┌─ Clido setup ───…───┐` / `│ … │` / `└──…──┘` is also valid.

**ASCII / non-TTY / narrow:**

```
  --- Clido setup ---
  Answer each question: type your choice, then press Enter. Defaults in [brackets].
```

**When launched from a script (e.g. run-in-test-env.sh):** The script must print *before* calling `clido init`:

```
  Next: Clido will ask 3 questions in this terminal (provider, model, then API key or base URL).
  Type your answer after each question and press Enter.
```

So the user never sees a motionless cursor without context.

### 2.3 Provider choice — exact copy

**Rich TTY (arrow-key selection):** Show an interactive list: "Anthropic (cloud) — requires API key", "OpenRouter (cloud) — requires API key", "Local (Ollama) — no key; use http://localhost:11434". User selects with arrow keys and Enter, or types and Enter. Default: first option (Anthropic).

**ASCII / non-TTY prompt (stderr):**

```
  Provider:
    1) Anthropic (cloud)  — requires API key
    2) OpenRouter (cloud) — requires API key
    3) Local (Ollama)     — no key; use http://localhost:11434
  Type 1, 2, or 3, then press Enter [default: 1]:
```

- Implementation must include the phrase "Type 1, 2, or 3, then press Enter" (or equivalent) in non-TTY so it is obvious that the program is waiting for input.
- Default: if the user presses Enter with no input, treat as `1`.

### 2.4 Model selection

After provider, the user selects a model.

**Rich TTY:** Show an interactive list of common models for the chosen provider, plus a "Custom..." option that opens a free-text prompt.

- Anthropic defaults: `claude-sonnet-4-5`, `claude-opus-4-5`, `claude-haiku-4-5-20251001`, `claude-3-5-sonnet-20241022`, `claude-3-5-haiku-20241022`
- OpenRouter defaults: `anthropic/claude-3-5-sonnet`, `anthropic/claude-haiku-3-5`, `openai/gpt-4o`, `openai/gpt-4o-mini`, `google/gemini-2.0-flash`
- Local: free-text `Text` prompt with default `llama3.2`

**ASCII / non-TTY prompt (stderr):**

```
  Model (press Enter for <default>):
```

- Empty input → use the default for that provider.

### 2.5 If provider = Anthropic — API key

**Prompt (stderr):**

```
  Use existing ANTHROPIC_API_KEY from your environment? [Y/n]:
```

- If user enters nothing, or `y`, `Y`, `yes`, `Yes` → use env; do not ask for a key value.
- If user enters `n` or `N` or `no` → print the following hint (stderr), then still write config with `api_key_env = "ANTHROPIC_API_KEY"`:

```
  Set your key in the environment, then run Clido again:
    export ANTHROPIC_API_KEY='your-key-here'
  Or add it to your shell profile. Then run: clido doctor
```

### 2.5a If provider = OpenRouter — API key

**Prompt (stderr):**

```
  Use existing OPENROUTER_API_KEY from your environment? [Y/n]:
```

Same logic as §2.5. Hint uses `OPENROUTER_API_KEY`. Generated config includes `api_key_env = "OPENROUTER_API_KEY"`.

### 2.6 If provider = Local — base URL

**Prompt (stderr):**

```
  Ollama base URL (press Enter for http://localhost:11434):
```

- Empty input → use `http://localhost:11434`.
- Otherwise use the trimmed line (no URL validation required in prompt; config load may validate later).

### 2.7 After config is written

**Message (stdout for `clido init`, stderr for first-run):**

```
  Created <path>. Run 'clido doctor' to verify.
```

- `<path>` is the actual config file path (e.g. `~/.config/clido/config.toml` or the value of `CLIDO_CONFIG`).

### 2.8 Titles for first-run vs init

- **First-run (no config, TTY):** Header title: `Clido setup` (or "First-time setup — Clido"); subline can say "Answer the questions below…" as in §2.2.
- **Explicit `clido init`:** Header title: `Clido setup`; subline can add "Re-run `clido init` anytime to change provider or reset config."

---

## 3. Scripts and Wrappers That Launch Interactive Commands

### 3.1 Rule

Any script or wrapper that invokes a Clido command that may read from stdin (e.g. `clido init`, or `clido` with no args in TTY) must print a short, explicit notice *before* the subprocess starts, so the user knows that (a) the next output is from Clido, and (b) they should type in this terminal.

### 3.2 Canonical intro for "init" from a script

Print to stderr or stdout (before calling `clido init`):

```
  Clido will ask 3 questions: provider (1, 2, or 3), model, then API key (Y/n) or base URL.
  Type your answer after each question and press Enter.
```

Optional second line:

```
  (Config will be written to $CLIDO_CONFIG or ~/.config/clido/config.toml)
```

### 3.3 No intro when user runs `clido init` directly

When the user runs `clido init` themselves in a terminal, no wrapper intro is needed; the CLI header (§2.2) and the prompts (§2.3–2.5) suffice.

---

## 4. Permission Prompt (State-Changing Tools)

Exact copy and layout are defined in **CLI spec §7**. This section only reiterates the rule that the prompt must show the possible answers clearly.

- **Rich TTY:** Box with tool name, file, change summary; then: `Allow? [y] yes  [n] no  [a] always  [d] disallow  [?] help`
- **ASCII:** `Allow? [y]es / [n]o / [a]lways / [d]isallow / [?]help:`

Implementations must not show a bare "Allow?" without the key options; the user must see what to type.

---

## 5. Project Instruction Trust (CLIDO.md / CLAUDE.md)

When loading an untrusted or changed project instruction file, prompt once (security-model and development-plan):

**Exact copy:**

```
  Load project instructions from <path>? [y/N]:
```

- Default: N (no). User must type `y` or `yes` to confirm.
- After confirm: store path and hash in allowlist; continue. If no: skip project instructions for this run.

---

## 6. Empty States and Hints

Defined in CLI spec §4 (empty-state output) and §6 (error message standards). Examples:

- **Sessions list (none):** `No sessions yet. Run 'clido <prompt>' to start one.`
- **Config missing, non-TTY:** `Error [Config]: No configuration found. Run 'clido init' to set up Clido.` (exit 2)

Every empty state and every error category must have defined copy and an actionable hint where applicable; see CLI spec §6 and the validation checklist §18.

---

## 7. Visual Design — "Hübsch" and Consistent

### 7.1 Goals

- **Readable at a glance:** Use spacing and simple structure (e.g. one blank line before a box, consistent indentation for tool lines).
- **Consistent symbols:** Same symbol for the same meaning everywhere (e.g. ✓ success, ✗ error, · pending, ↻ in progress). ASCII fallback: `[ok]`, `[err]`, `[run]`, etc. (CLI spec §5).
- **No walls of text:** Break long output into short blocks; use a box or a short rule for important prompts (permission, setup header).
- **Respect terminal size:** Minimum 60 columns; truncate paths and long lines in narrow or non-TTY mode (CLI spec §5).

### 7.2 Box drawing (rich TTY)

- Use light Unicode box-drawing for setup and permission: single-line `┌`, `─`, `┐`, `│`, `└`, `┘` (top line: `┌─ <title> ───…───┐`) or double-line `╔`, `═`, `╗`, `║`, `╚`, `╝` for a stronger header (e.g. setup banner). Fill to a reasonable width (e.g. 60–65 columns).
- ASCII fallback: use a simple line of dashes and a title, e.g. `--- Clido setup ---`, no box.

### 7.3 Colors

- Use color only to support, not to replace, text: e.g. green for success, red for error, yellow/dim for warnings, cyan for accent. When `NO_COLOR` is set or when no TTY is available for the relevant stream, omit color; symbols and text carry full meaning (CLI spec §16).
- **Setup flow (first-run / `clido init`):** When stdin or stderr is a TTY and `NO_COLOR` is not set: bold + bright cyan for "Welcome to Clido.", dim for "Let's set up your environment." and for hints; cyan for the banner box; dim for the "Type 1 or 2…" prompt line; green for the "Created … Run 'clido doctor'" success line.
- **Agent banner:** When stdout is a TTY and `NO_COLOR` is not set: cyan for the ASCII art banner so the product name stands out.
- **REPL prompt:** When stderr is a TTY and `NO_COLOR` is not set: dim or cyan for the "clido> " prompt.
- **Doctor:** Green for ✓ (pass), red for ✗ (mandatory failure), yellow or dim for ⚠ (warnings).
- **Errors:** Red for "Error [Config]:" and "Error:" on stderr when TTY and no `NO_COLOR`.
- **First-run notice and "Interrupted.":** Dim (or red for Interrupted) when TTY and no `NO_COLOR`.
- **Deprecation warnings:** Yellow or dim for "Warning: …" when TTY and no `NO_COLOR`.

### 7.4 Session footer and tool lifecycle

- As in CLI spec §5: tool lifecycle (· → ↻ → ✓/✗), session footer `✓ Done  ·  5 turns  ·  $0.0041  ·  2.3s`. Keep alignment and spacing consistent so the output looks orderly, not ragged.

### 7.5 Banners: setup vs agent

- **Setup banner (init / first-run):** The welcome line + double-line box shown at the start of `clido init` or first-run setup. This is the "setup screen"; it is the first thing the user sees when running init (no "clido starting" log line before it). See §2.2 and [ui-implementation-plan.md](ui-implementation-plan.md) §2.1.
- **Agent banner:** The ASCII-art "Clido" logo shown when starting a **run** (e.g. `clido "fix the test"`) or the REPL in text mode. Only visible when stdout is a TTY. See [ui-implementation-plan.md](ui-implementation-plan.md) §2.4.

### 7.6 Other UI surfaces (agent, doctor, sessions, workflow, errors)

- **Agent banner:** Shown when starting a run or REPL in text mode and stdout is a TTY; optional cyan when color is on. See [ui-implementation-plan.md](ui-implementation-plan.md) §2.4.
- **Doctor:** ✓ / ✗ / ⚠ with optional green/red/yellow per §7.3. Copy and exit codes in CLI spec §4 and §6.
- **Sessions list:** Empty state and list format in CLI spec §4; optional header when TTY. See ui-implementation-plan §2.7–2.8.
- **Workflow:** Completion line and dry-run format; optional success color. See ui-implementation-plan §2.9–2.11.
- **Errors:** "Error [Config]: …" and "Error: …" on stderr; optional red when TTY. See CLI spec §6 and ui-implementation-plan §2.12.

---

## 8. Checklist for Implementors and QA

- [ ] Every interactive prompt in first-run/init uses the exact or template copy from §2; in rich TTY, arrow-key selection is available; in non-TTY, a "Type … and press Enter" (or equivalent) instruction is shown.
- [ ] Defaults are shown in brackets (e.g. `[1]`, `[Y/n]`).
- [ ] Scripts that run `clido init` (or other interactive commands) print the intro from §3 before the subprocess.
- [ ] Permission and project-instruction prompts use the copy from CLI spec §7 and security-model / §5 above.
- [ ] Empty states and errors use the copy from CLI spec §4 and §6.
- [ ] Rich TTY output uses consistent symbols and (where specified) a simple box or header; ASCII fallback is defined and tested.
- [ ] No interactive moment leaves the user without a visible explanation of what to type or that the program is waiting for input.

---

## 9. References

- **UI implementation plan:** [ui-implementation-plan.md](ui-implementation-plan.md) — every touchpoint, user story, and implementation step; single source for what to implement where.
- **CLI spec:** [cli-interface-specification.md](cli-interface-specification.md) — §4 First-Run, §5 Text Output, §6 Errors, §7 Permission, §8 REPL, §16 Accessibility.
- **Security model:** [../guides/security-model.md](../guides/security-model.md) — project-instruction trust prompt.
- **Development plan:** [development-plan.md](development-plan.md) — Phase 3.4.2 (project instructions), Phase 4.3 (permission prompt).
- **Evidence style:** [../REPORT.md](../REPORT.md), [../ARTIFACTS.md](../ARTIFACTS.md) — structure and rigor for spec docs.
