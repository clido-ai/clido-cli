# Harness mode (structured long-running work)

Harness mode turns clido into a **file-backed task protocol**: work is tracked in JSON under `.clido/harness/`, progress is append-only, and **pass** is only reachable through a **separate reviewer tool** that the main agent cannot call.

## Enable

- CLI: `clido --harness` (or env `CLIDO_HARNESS=1` if wired in your shell profile)
- Config: `[agent] harness = true` in `config.toml` (merged with project config)

## On-disk layout

| Path | Role |
|------|------|
| `.clido/harness/tasks.json` | Canonical tasks (`id`, `description`, `steps`, `acceptance_criteria`, `status` = `fail` \| `pass`, optional `verification`) |
| `.clido/harness/progress.ndjson` | Append-only session / decision log |

Use **`HarnessControl`** tool op **`read`** for a snapshot (tasks + progress tail + `git log` snippet).

## Roles (enforced in code)

1. **Planner (main agent)** — `planner_append_tasks` only for new work. Every task needs non-empty `acceptance_criteria`. New tasks are always `fail`.
2. **Executor (main agent)** — `executor_set_focus`, `executor_register_attempt`, `progress_append`, etc. The executor’s `HarnessControl` **cannot** run `evaluator_mark_pass` (the tool rejects it).
3. **Evaluator (SpawnReviewer sub-agent)** — A **different** `HarnessControl` instance with only **`read`** and **`evaluator_mark_pass`**. After independent verification, the reviewer records pass with structured evidence.

## TodoWrite

In harness mode, **`TodoWrite` is not registered**. Use harness tasks only, so the TUI plan strip shows **Harness** rows from `tasks.json` instead of ad-hoc todos.

## TUI

With harness enabled, the progress strip defaults to **on** and shows task ids, descriptions, and focus (`›` on the focused failing task). Toggle visibility with **`/progress on|off|auto`**.

## Limits (honest)

- Verification is still **structured trust**: the harness checks payload shape, criterion strings, and minimum evidence length — it does not re-run your test suite automatically.
- The reviewer is another LLM call (same stack as the main agent unless you change models); correlation is possible — pair with CI for hard gates.

See also: [Configuration reference](/reference/config) (`harness` under `[agent]`), [Flags](/reference/flags) (`--harness`).
