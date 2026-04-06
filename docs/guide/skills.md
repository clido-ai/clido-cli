# Skills

**Skills** are reusable instructions the agent can follow when a task matches their purpose. They are ordinary files on disk (with optional YAML front matter), **you control** which ones are active, and they are injected into the system prompt inside a `<skills>` block.

## Where skills live

Skills are loaded from these locations **in order**; the first occurrence of a given **id** wins (workspace overrides global):

| Source | Path | Notes |
|--------|------|--------|
| Workspace | `<project>/.clido/skills/` | Per-repository skills |
| User | `~/.clido/skills/` | Available in every project |
| Config | `[skills] extra-paths` | Absolute paths, or relative to the workspace root; `~/` is expanded |
| Environment | `CLIDO_SKILL_PATHS` | Extra directories, separated by `:` (macOS/Linux) or `;` (Windows) |

Supported file extensions: **`.md`** and **`.txt`**.

## Skill file format

### With YAML front matter (recommended)

The body after the closing `---` is the instruction text the model follows.

```markdown
---
id: my-skill
name: Human readable title
description: One-line summary for lists
purpose: When to apply this skill
inputs: What you need from the user or repo
outputs: What you should deliver
tags: [rust, refactor]
version: "0.1.0"
---

## Steps
1. …
```

- **`id`** — Stable identifier (defaults to the file stem if omitted).
- Other fields are optional but help the model choose when to use the skill.

### Without front matter

The whole file is the body. **id** and **name** default from the filename.

## Config: `[skills]`

Merged from global and project `config.toml` (project overrides where noted).

| Key | Type | Description |
|-----|------|-------------|
| `disabled` | list of strings | Skill ids to never inject |
| `enabled` | list of strings | If non-empty, **only** these ids are injected (whitelist) |
| `extra-paths` | list of strings | More directories to scan |
| `no-skills` | boolean | If true, skip all skill injection |
| `auto-suggest` | boolean (optional) | When true (default), the prompt encourages suggesting matching skills the user could enable |
| `registry-urls` | list of strings | **Reserved** for future remote registries; not fetched yet |

Example:

```toml
[skills]
disabled = ["experimental-skill"]
# enabled = ["only-this-one"]   # whitelist mode
# extra-paths = ["~/my-shared-skills"]
# no-skills = false
# auto-suggest = true
```

## CLI

```bash
clido skills list              # ids, summaries, active vs disabled
clido skills paths             # resolved search paths
clido skills disable <id>      # append to project [skills].disabled
clido skills enable <id>       # remove from project disabled list
```

Changes to config require a **new agent session** (restart TUI or re-run CLI) to refresh the system prompt.

## TUI

| Command | Action |
|---------|--------|
| `/skills` or `/skills list` | List discovered skills and whether each is active |
| `/skills paths` | Show search directories (and configured registry URLs) |
| `/skills disable <id>` | Update project `.clido/config.toml` |
| `/skills enable <id>` | Remove id from disabled list |

## Agent behavior

When `<skills>` is present, the bundled system prompt tells the model to:

- Apply a skill when the task matches its purpose.
- **Name the skill id** when applying it.
- **Not** invent steps that are not in the loaded text.
- **Suggest** skills only when appropriate and when `auto-suggest` behavior is enabled — never claim a skill ran if it was not loaded.

## See also

- [Configuration](/docs/guide/configuration) — merging global and project config
- [config.toml reference](/docs/reference/config) — all `[skills]` keys
- [Slash commands](/docs/reference/slash-commands) — TUI command list
