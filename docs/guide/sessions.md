# Session Management

Every clido conversation is stored as a session. Sessions are persistent JSONL files that allow you to resume any past conversation, review history, and track costs over time.

## What sessions are

A session is a newline-delimited JSON (JSONL) file stored in:

```
~/.local/share/clido/sessions/<project-hash>/<session-id>.jsonl
```

Each line in the file is a typed record: metadata, user messages, assistant messages, tool calls, tool results, and a final result record. See [Session Format](/docs/developer/session-format) for the complete schema.

The first line is always a `meta` record with `schema_version`. If you open a session file written by a newer clido that bumps the schema, an older binary will refuse to load it until you upgrade. The current version is documented in the session format reference.

Sessions are created automatically the first time you run a prompt. There is no explicit "new session" command — just start a new conversation.

## Listing sessions

```bash
clido sessions list
```

```
ID        DATE        PROJECT                  PREVIEW                        COST
a1b2c3    2026-03-21  ~/projects/my-app        "Refactor the parser module"   $0.023
d4e5f6    2026-03-20  ~/projects/my-app        "Add unit tests for lexer"     $0.041
789abc    2026-03-19  ~/projects/lib           "Fix memory leak in pool"      $0.019
```

Sessions are listed most-recent-first and scoped to all projects (not just the current directory).

## Resuming a session

### Continue the newest session

Resume the most recent session for the current working directory:

```bash
clido --continue
clido -c   # short form
```

### Resume by ID

Resume any session by its ID (or a unique prefix):

```bash
clido --resume a1b2c3
```

When the session is loaded, clido reconstructs the full conversation history and continues from where you left off.

## Stale file detection

When a session is resumed, clido checks whether any files that were read or written during the previous session have changed since the session ended. If stale files are detected, you are warned:

```
Warning: 2 files have changed since this session was last active:
  src/main.rs (modified 3 hours ago)
  src/parser.rs (modified 1 hour ago)

The agent's cached knowledge of these files may be outdated.
Continue anyway? [y/N]
```

To skip this check:

```bash
clido --resume a1b2c3 --resume-ignore-stale
```

## Viewing a session

Print the full contents of a session in human-readable form:

```bash
clido sessions show a1b2c3
```

## Verifying a session (strict load)

Check that a session file can be loaded the same way as **resume** (strict JSON / history decode):

```bash
clido sessions verify a1b2c3
```

If this fails, `clido --resume <id>` and TUI resume for that session will also fail until the file is repaired.

## Forking a session

Create a copy of a session with a new ID. The fork starts from the full history of the original, allowing you to explore an alternative conversation path:

```bash
clido sessions fork a1b2c3
# Forked session a1b2c3 → new session f9e8d7
```

Then resume the fork:

```bash
clido --resume f9e8d7
```

## Session statistics

View cost, turn count, and timing for one or all sessions:

```bash
# All sessions
clido stats

# Specific session
clido stats --session a1b2c3

# JSON output for scripting
clido stats --json
```

```
Sessions: 12
Total cost: $0.87
Total turns: 148
Average cost per session: $0.07
Most expensive session: d4e5f6 ($0.041)
```

## Session storage location

Sessions are stored under the clido data directory:

| Platform | Default path |
|----------|-------------|
| Linux | `~/.local/share/clido/sessions/` |
| macOS | `~/Library/Application Support/clido/sessions/` |

Override with the `CLIDO_DATA_DIR` environment variable.

Within the sessions directory, sessions are organised by a hash of the project path:

```
sessions/
  a3f8b2c1d4e5.../       ← hash of /home/user/projects/my-app
    a1b2c3d4e5f6....jsonl
    d4e5f6789abc....jsonl
  9e7f1a3c5b2d.../       ← hash of /home/user/projects/lib
    789abcdef012....jsonl
```

## In the TUI: session picker

Open the session picker inside the TUI with `/sessions`:

```
╭─ Sessions ──────────────────────────────────────────────────────────────────╮
│  > a1b2c3  2026-03-21  "Refactor the parser module"  $0.023                 │
│    d4e5f6  2026-03-20  "Add unit tests for lexer"     $0.041                │
│    789abc  2026-03-19  "Fix memory leak in pool"      $0.019                │
╰─────────────────────────────────────────────────────────────────────────────╯
```

Press `Up` / `Down` to select, `Enter` to open, `Escape` to close. Type to filter by id, title, or preview; `Ctrl+D` deletes the selected session (not plain `d`, so filter text can include any letter).

## Session JSONL format

Sessions are plain text and can be read with any tool:

```bash
# Count tool calls in a session
jq 'select(.type == "tool_call") | .tool_name' \
  ~/.local/share/clido/sessions/*/*.jsonl | sort | uniq -c

# Extract all assistant text
jq -r 'select(.type == "assistant_message") | .content[] | select(.type == "text") | .text' \
  ~/.local/share/clido/sessions/*/a1b2c3*.jsonl
```

See [Session Format](/docs/developer/session-format) for the complete schema with examples.
