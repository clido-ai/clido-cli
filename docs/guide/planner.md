# Plan Mode

Plan mode decomposes a complex task into a structured, editable step-by-step plan before executing it. It is meaningfully different from competitors: the plan is **interactive** — you can rename tasks, delete steps, add notes, reorder, skip, and save before a single tool call runs.

::: tip
Plan mode is available with `--plan` (or the alias `--planner`). It opens a full-screen TUI editor so you can review and modify the generated plan before anything executes.
:::

---

## Quick start

```bash
clido --plan "refactor the auth module to use JWT"
```

1. clido calls the LLM to generate a task graph (JSON)
2. The **plan editor** opens as a full-screen overlay
3. Review and edit the plan — press `x` to execute, `Esc` to abort

---

## Plan editor

The plan editor is a full-screen TUI overlay:

```
╔══ Plan: Refactor auth module to JWT (3 tasks · complexity: high) ══╗
║                                                                      ║
║   t1  [low]   Read existing auth module and identify patterns        ║
║   t2  [low]   Add jwt crate to Cargo.toml              →needs t1     ║
║ ▶ t3  [high]  Implement JWT token generation           →needs t2     ║ ← selected
║                                                                      ║
║  Progress: 0/3                                                       ║
║                                                                      ║
║  Enter=edit  d=delete  n=new  Space=skip  ↑↓=move  s=save  x=execute  Esc=abort
╚══════════════════════════════════════════════════════════════════════╝
```

### Keybindings

| Key | Action |
|-----|--------|
| `↑` / `↓` | Move task selection |
| `Enter` | Open inline edit form for selected task |
| `d` | Delete selected task (blocked if other tasks depend on it) |
| `n` | Add a new task at the end, then open edit form |
| `Space` | Toggle skip on selected task |
| `r` | Move selected task up one position (reorder) |
| `s` | Save plan to `.clido/plans/<id>.json` |
| `x` | Execute the plan (sends tasks as a structured prompt to the agent) |
| `Esc` | Abort — close editor without executing |

### Inline task edit form

Press `Enter` on a task to open the edit form:

```
  Edit task

  Description: [Implement JWT token generation         ]
  Notes:        [Use HS256, expiry = 24h               ]
  Complexity:   ● low  ● medium  ● high

  Tab=next field  Enter=save  Esc=cancel
```

| Key | Action |
|-----|--------|
| `Tab` | Move to next field (Description → Notes → Complexity) |
| `←` / `→` | Cycle complexity when Complexity field is focused |
| `Enter` | Save the edits and return to task list |
| `Esc` | Discard edits and return to task list |

---

## Flags

| Flag | Description |
|------|-------------|
| `--plan` | Enable plan mode (also `--planner`) |
| `--plan-dry-run` | Generate and show the plan editor, but never execute (even if you press `x`) |
| `--plan-no-edit` | Skip the editor and execute immediately (CI-friendly) |

Examples:

```bash
# Standard interactive plan mode
clido --plan "add unit tests for the parser module"

# Preview the plan without running it
clido --plan --plan-dry-run "refactor database layer"

# Generate a plan and execute immediately (no editor)
clido --plan --plan-no-edit "fix all clippy warnings"
```

---

## Saving and resuming plans

Save the plan from within the editor (`s`) or via `/plan save` in the TUI. Plans are stored in `.clido/plans/<id>.json`.

```bash
# List saved plans
clido plan list

# Show a plan's tasks and status
clido plan show plan-abc12345

# Execute a saved plan (skips already-done tasks)
clido plan run plan-abc12345

# Delete a saved plan
clido plan delete plan-abc12345
```

---

## TUI slash commands

When `--plan` is active:

| Command | Effect |
|---------|--------|
| `/plan` | Show the current plan as a text tree in the chat |
| `/plan edit` | Re-open the plan editor overlay |
| `/plan save` | Save current plan to `.clido/plans/` |
| `/plan list` | List all saved plans in the chat |

---

## Plan JSON schema

Plans are stored as plain JSON in `.clido/plans/`:

```json
{
  "meta": {
    "id": "plan-a1b2c3d4",
    "goal": "Refactor auth module to use JWT",
    "created_at": "2026-03-22T10:00:00Z"
  },
  "tasks": [
    {
      "id": "t1",
      "description": "Read existing auth module",
      "depends_on": [],
      "complexity": "low",
      "skip": false,
      "notes": "",
      "status": "pending"
    },
    {
      "id": "t2",
      "description": "Add jwt crate to Cargo.toml",
      "depends_on": ["t1"],
      "complexity": "low",
      "skip": false,
      "notes": "",
      "status": "pending"
    }
  ]
}
```

**`status`** values: `pending` | `running` | `done` | `failed` | `skipped`
**`complexity`** values: `low` | `medium` | `high`

---

## Fallback behaviour

If the LLM returns unparseable JSON or a graph with cycles, clido silently falls back to the reactive agent loop. No error is shown; the task just runs without a plan.

---

## When to use plan mode

Best suited for:

- Multi-file refactors where ordering matters
- Tasks you want to review before running (code generation, large changes)
- Workflows you save and re-run across multiple sessions
- CI pipelines where `--plan-no-edit` generates consistent execution plans
