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

**Rich TTY (preferred when stdout is a TTY and width ≥ 60):**

```
┌─ Clido setup ────────────────────────────────────────────────┐
│  Choose a provider and where to store your API key.          │
│  Answer the questions below; type a number or letter,         │
│  then press Enter. Defaults are in brackets.                  │
└─────────────────────────────────────────────────────────────┘
```

**ASCII / non-TTY / narrow:**

```
  --- Clido setup ---
  Answer each question: type your choice, then press Enter. Defaults in [brackets].
```

**When launched from a script (e.g. run-in-test-env.sh):** The script must print *before* calling `clido init`:

```
  Next: Clido will ask 2 questions in this terminal (provider, then API key).
  Type your answer after each question and press Enter.
```

So the user never sees a motionless cursor without context.

### 2.3 Provider choice — exact copy

**Prompt (stderr):**

```
  Provider:
    1) Anthropic (cloud) — requires API key
    2) Local (Ollama)    — no key; use http://localhost:11434
  Type 1 or 2, then press Enter [default: 1]:
```

- Implementation must include the phrase "Type 1 or 2, then press Enter" (or equivalent: "Enter 1 or 2 and press Enter") so it is obvious that the program is waiting for input and what to type.
- Default: if the user presses Enter with no input, treat as `1`.

### 2.4 If provider = Anthropic — API key

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

### 2.5 If provider = Local — base URL

**Prompt (stderr):**

```
  Ollama base URL (press Enter for http://localhost:11434):
```

- Empty input → use `http://localhost:11434`.
- Otherwise use the trimmed line (no URL validation required in prompt; config load may validate later).

### 2.6 After config is written

**Message (stdout for `clido init`, stderr for first-run):**

```
  Created <path>. Run 'clido doctor' to verify.
```

- `<path>` is the actual config file path (e.g. `~/.config/clido/config.toml` or the value of `CLIDO_CONFIG`).

### 2.7 Titles for first-run vs init

- **First-run (no config, TTY):** Header title: `Clido setup` (or "First-time setup — Clido"); subline can say "Answer the questions below…" as in §2.2.
- **Explicit `clido init`:** Header title: `Clido setup`; subline can add "Re-run `clido init` anytime to change provider or reset config."

---

## 3. Scripts and Wrappers That Launch Interactive Commands

### 3.1 Rule

Any script or wrapper that invokes a Clido command that may read from stdin (e.g. `clido init`, or `clido` with no args in TTY) must print a short, explicit notice *before* the subprocess starts, so the user knows that (a) the next output is from Clido, and (b) they should type in this terminal.

### 3.2 Canonical intro for "init" from a script

Print to stderr or stdout (before calling `clido init`):

```
  Clido will ask 2 questions: provider (1 or 2), then API key (Y/n).
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

- Use light Unicode box-drawing for setup and permission: `┌`, `─`, `┐`, `│`, `└`, `┘`. Top line: `┌─ <title> ───…───┐` (fill to a reasonable width, e.g. 60).
- ASCII fallback: use a simple line of dashes and a title, e.g. `--- Clido setup ---`, no box.

### 7.3 Colors

- Use color only to support, not to replace, text: e.g. green for success, red for error, dim for hints. When `NO_COLOR` is set or non-TTY, omit color; symbols and text carry full meaning (CLI spec §16).
- **Setup flow (first-run / `clido init`):** When stderr is a TTY and `NO_COLOR` is not set: cyan for the banner box, dim for the "Type 1 or 2…" prompt line and for hints (e.g. export key), green for the "Created … Run 'clido doctor'" success line. This keeps the flow readable and visually clear without relying on color for meaning.

### 7.4 Session footer and tool lifecycle

- As in CLI spec §5: tool lifecycle (· → ↻ → ✓/✗), session footer `✓ Done  ·  5 turns  ·  $0.0041  ·  2.3s`. Keep alignment and spacing consistent so the output looks orderly, not ragged.

---

## 8. Checklist for Implementors and QA

- [ ] Every interactive prompt in first-run/init uses the exact or template copy from §2 and includes a "Type … and press Enter" (or equivalent) instruction.
- [ ] Defaults are shown in brackets (e.g. `[1]`, `[Y/n]`).
- [ ] Scripts that run `clido init` (or other interactive commands) print the intro from §3 before the subprocess.
- [ ] Permission and project-instruction prompts use the copy from CLI spec §7 and security-model / §5 above.
- [ ] Empty states and errors use the copy from CLI spec §4 and §6.
- [ ] Rich TTY output uses consistent symbols and (where specified) a simple box or header; ASCII fallback is defined and tested.
- [ ] No interactive moment leaves the user without a visible explanation of what to type or that the program is waiting for input.

---

## 9. References

- **CLI spec:** [cli-interface-specification.md](cli-interface-specification.md) — §4 First-Run, §5 Text Output, §6 Errors, §7 Permission, §8 REPL, §16 Accessibility.
- **Security model:** [../guides/security-model.md](../guides/security-model.md) — project-instruction trust prompt.
- **Development plan:** [development-plan.md](development-plan.md) — Phase 3.4.2 (project instructions), Phase 4.3 (permission prompt).
- **Evidence style:** [../REPORT.md](../REPORT.md), [../ARTIFACTS.md](../ARTIFACTS.md) — structure and rigor for spec docs.
