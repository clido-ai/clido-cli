# Clido UI Implementation Plan

**Purpose:** Single reference for every user-facing UI touchpoint. Each step maps to a user story, exact copy, TTY vs non-TTY behavior, and the file/location that implements it. Implementation and UX requirements must stay in sync with this plan.

**Scope:** All CLI output (setup, agent banner, REPL, doctor, sessions, workflow, errors, permission prompt, scripts). Every surface is production-grade: clear structure, consistent symbols and colors, no silent waiting.

**References:** [ux-requirements.md](ux-requirements.md), [cli-interface-specification.md](cli-interface-specification.md).

---

## 1. Principles (aligned with UX requirements)

1. **No silent waiting** — Every prompt states what to type and how to confirm (e.g. "Type 1 or 2, then press Enter").
2. **Every prompt has defined copy** — Exact or template text in ux-requirements and this plan; defaults in brackets.
3. **Functional and visually clear** — Consistent symbols (✓ ✗ ⚠), spacing, boxes; ASCII fallback when non-TTY or NO_COLOR.
4. **Scripts that launch interactive commands** — Print a short intro before the subprocess (see §3.2 in ux-requirements).
5. **Color supports, never replaces** — Green success, red error, yellow/dim warning, cyan accent; meaning is in text/symbols.

---

## 2. Touchpoint map and user stories

| # | Surface | User story | When | Output stream | TTY behavior | Non-TTY / NO_COLOR |
|---|--------|------------|------|----------------|--------------|--------------------|
| 2.1 | Setup welcome + box | As a new user I see a clear setup screen | First-run or `clido init` | stderr | Welcome line (bold+bright cyan), **aligned** double-line box (59-char width, cyan), hints dim; **arrow-key selection** for provider and Y/n | ASCII "--- Clido setup ---" + one line; "Type 1 or 2…" when non-TTY |
| 2.2 | Provider / API prompts | I know what to type | Same flow | stderr | Options listed; "Type 1 or 2…" dim; hints dim | Same text, no color |
| 2.3 | Config created | I see success and next step | After setup | stdout (init) / stderr (first-run) | Green "Created … Run 'clido doctor'" | Plain text |
| 2.4 | Agent banner | I see the product name when the agent starts | `clido <prompt>` or REPL, text mode, stdout TTY | stdout | ASCII art; optional cyan when color on | No banner when stdout not TTY; or plain art |
| 2.5 | REPL prompt | I know where to type | REPL loop | stderr | "clido> " with optional dim/cyan | "clido> " plain |
| 2.6 | Doctor checks | I see pass/fail at a glance | `clido doctor` | stdout (✓), stderr (✗ ⚠) | Green ✓, red ✗, yellow/dim ⚠ | Same symbols, no color |
| 2.7 | Sessions list empty | I know what to do next | `clido sessions list`, no sessions | stdout | One line hint | Same |
| 2.8 | Sessions list | I see session id, time, turns, cost | `clido sessions list` | stdout | Optional header/box when TTY | Plain list |
| 2.9 | Workflow dry-run | I see rendered prompts | `clido workflow run --dry-run` | stdout | "Step N: …" and "---\n…\n---" | Same |
| 2.10 | Workflow completed | I see summary | `clido workflow run` | stdout | Optional green/success style | Plain "Workflow completed: …" |
| 2.11 | Workflow validate/inspect/list | I see result or list | workflow subcommands | stdout | Optional dim for paths | Plain |
| 2.12 | Errors | I see what went wrong | Any command | stderr | "Error [Config]:" / "Error:" in red when TTY | Same text, no color |
| 2.13 | First-run notice | I know setup is starting | No config, TTY, before setup | stderr | Dim "No configuration found. Running first-time setup." | Plain |
| 2.14 | Interrupted | I see that I cancelled | Ctrl-C during agent | stderr | "Interrupted." optional dim/red | Plain |
| 2.15 | Permission prompt | I can allow/deny a tool | Default permission mode, state-changing tool | stderr | (Current: simple "Allow …? [y/N]"; future: box + [y][n][a][d][?]) | Same text |
| 2.16 | Deprecation warnings | I know to switch command | list-sessions, show-session | stderr | Dim/yellow "Warning: …" | Plain "Warning: …" |
| 2.17 | Script intro | I know questions will follow | run-in-test-env.sh before `clido init` | stdout | Two lines per ux-requirements §3.2 | Same |

---

## 3. Implementation steps (by file)

### 3.1 `crates/clido-cli/src/main.rs`

| Step | What | Where / constant | Detail |
|------|------|-------------------|--------|
| 1 | Shared color / TTY helpers | Top of file | `cli_use_color()`: (stdin or stderr or stdout).is_terminal() && no NO_COLOR. Reuse in setup (or keep setup_use_color as alias). `setup_use_rich_ui()`: stdin or stderr TTY (already present). |
| 2 | ANSI palette | `mod ansi` | RESET, BOLD, CYAN, BRIGHT_CYAN, DIM, GREEN (already); add RED, YELLOW (or BRIGHT_YELLOW) for errors and warnings. |
| 3 | Setup UI | `run_interactive_setup_blocking` | Welcome line, double-line box, spacing, flush (done). Ensure Provider/hints use DIM when color. |
| 4 | Agent BANNER | Before agent loop (single-run and REPL) | When stdout is TTY and cli_use_color(): print CYAN then BANNER then RESET. Else print BANNER plain. |
| 5 | REPL prompt | `eprint!("clido> ")` | When cli_use_color(): eprint DIM (or CYAN) + "clido> " + RESET. Else plain. |
| 6 | Doctor | `run_doctor` | ✓ lines: when cli_use_color() wrap in GREEN. ✗ lines: RED. ⚠ lines: YELLOW or DIM. |
| 7 | Sessions list | `run_sessions_list` | Empty state: keep exact copy. Optional: when stdout TTY and color, print a short header (e.g. dim "Sessions") before list. |
| 8 | Workflow output | workflow run/validate/inspect/list | Completion line: optional GREEN. Dry-run and list: keep readable; optional DIM for paths. |
| 9 | Errors | match CliError, eprintln!("Error...") | When cli_use_color(): eprintln RED + "Error [Config]: …" or "Error: …" + RESET. |
| 10 | First-run notice | "No configuration found. Running first-time setup." | When cli_use_color(): eprintln DIM + message + RESET. |
| 11 | Interrupted | "Interrupted." | When cli_use_color(): eprintln DIM or RED + "Interrupted." + RESET. |
| 12 | Permission prompt | StdinAskUser::ask | Keep current copy; when cli_use_color(): eprint DIM before prompt, RESET after. (Future: rich box per CLI spec §7.) |
| 13 | Deprecation warnings | list-sessions, show-session | When cli_use_color(): eprintln YELLOW + "Warning: …" + RESET. |

### 3.2 `scripts/run-in-test-env.sh`

| Step | What | Detail |
|------|------|--------|
| 1 | Intro before `clido init` | Exact copy per ux-requirements §3.2: "Clido will ask 2 questions: provider (1 or 2), then API key (Y/n)." + "Type your answer after each question and press Enter." Optional second line: config path. |

### 3.3 `devdocs/plans/ux-requirements.md`

| Step | What | Detail |
|------|------|--------|
| 1 | §2.2 Header | Document double-line box (╔═╗ ║ ╚═╝) and welcome line "Welcome to Clido." / "Let's set up your environment." as production standard. |
| 2 | §7.2 Box drawing | Allow double-line as alternative: "Use light Unicode box-drawing … or double-line (╔═╗) for setup." |
| 3 | §7.3 Colors | Extend: agent banner (cyan when TTY), doctor (green/red/yellow), errors (red), REPL prompt (dim/cyan). |
| 4 | New §7.5 or appendix | Agent banner, doctor, sessions, workflow, errors: one line each pointing to this plan and CLI spec. |

---

## 4. User stories in full (for QA and copy checks)

- **Setup:** "As a new user, when I run `clido` with no config or `clido init`, I see a welcome line and a clear **aligned** bordered setup box. I can choose the provider with **arrow keys** (or type 1/2 and Enter) and confirm API key with arrow keys or Y/n. After I answer, I see a green success line with the config path and 'Run clido doctor'. No log line appears before the banner when running init."
- **Agent:** "When I run `clido 'fix the test'` in a terminal, I see the Clido ASCII banner (optionally in cyan) and then the agent output. I never see a motionless cursor without explanation."
- **REPL:** "When I run `clido` with no args in a TTY, I see the same banner and then 'clido> '; I can type a prompt and get a result. The prompt is visually distinct (e.g. dim or cyan)."
- **Doctor:** "When I run `clido doctor`, I see ✓ in green for passing checks, ✗ in red for mandatory failures, and ⚠ in yellow/dim for warnings. Copy is actionable (e.g. set API key, run clido doctor)."
- **Sessions:** "When I run `clido sessions list` with no sessions, I see 'No sessions yet. Run clido <prompt> to start one.' When I have sessions, I see id, time, turns, cost, preview in a readable list."
- **Workflow:** "When I run a workflow, I see a clear completion line (e.g. 'Workflow completed: N steps, $X, Y ms'). Dry-run shows step prompts between --- markers."
- **Errors:** "When something goes wrong, I see 'Error [Config]: …' or 'Error: …' on stderr in red when color is on, with an actionable hint where applicable."
- **Script:** "When I run ./scripts/run-in-test-env.sh init, I see two lines explaining that Clido will ask 2 questions and to type answers, then the setup UI."

---

## 5. Checklist for implementors

- [ ] All setup copy matches ux-requirements §2 and this plan (§2.1–2.3).
- [ ] Agent banner and REPL prompt use cli_use_color() and ansi consistently (§3.1 steps 4–5).
- [ ] Doctor uses GREEN/RED/YELLOW for ✓/✗/⚠ when cli_use_color() (§3.1 step 6).
- [ ] Errors and first-run notice use RED/DIM where specified (§3.1 steps 9–11).
- [ ] Deprecation warnings use YELLOW (§3.1 step 13).
- [ ] run-in-test-env.sh intro matches ux-requirements §3.2 (§3.2 step 1).
- [ ] ux-requirements.md updated with double-line box, welcome line, and extended color/agent/doctor/sessions/workflow/errors (§3.3).
- [ ] No interactive moment leaves the user without visible explanation of what to type.

---

## 6. References

- **UX requirements:** [ux-requirements.md](ux-requirements.md) — §2 First-Run, §3 Scripts, §4 Permission, §7 Visual design.
- **CLI spec:** [cli-interface-specification.md](cli-interface-specification.md) — §4 First-Run, §5 Text output, §6 Errors, §7 Permission, §8 REPL.
