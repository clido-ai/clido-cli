# Output Formats

clido supports three output formats, controlled by `--output-format`.

## text (default)

Human-readable output. Includes:
- A line per tool call while the agent runs
- The agent's final response text
- A cost/turn/time summary footer

```bash
clido "count lines in src/main.rs"
```

```
[Turn 1] Reading src/main.rs...

src/main.rs has 312 lines.

  Cost: $0.0009  Turns: 1  Time: 2.1s
```

In `--quiet` mode, only the agent's final response is printed; the tool lines and footer are suppressed.

## json

A single JSON object emitted after the agent finishes. Suitable for scripting.

```bash
clido --output-format json "count lines in src/main.rs"
```

### JSON schema

Success:

```json
{
  "schema_version": 1,
  "type": "result",
  "exit_status": "completed",
  "result": "src/main.rs has 312 lines.",
  "session_id": "a1b2c3d4e5f6789abcdef0123456789abcdef01",
  "num_turns": 1,
  "duration_ms": 2100,
  "total_cost_usd": 0.0009,
  "is_error": false,
  "usage": {
    "input_tokens": 1200,
    "output_tokens": 80,
    "cache_read_input_tokens": 0,
    "cache_creation_input_tokens": 0
  }
}
```

Failure (`is_error: true`): same shape; `result` holds the error message and `exit_status` is a classifier (see table below).

### Field descriptions

| Field | Type | Description |
|-------|------|-------------|
| `schema_version` | integer | Format version (currently `1`) |
| `type` | string | Always `result` for the final object |
| `exit_status` | string | Outcome classifier (see table) |
| `result` | string | Final assistant text or error message |
| `session_id` | string | Full 40-character session ID |
| `num_turns` | integer | Number of agent turns |
| `duration_ms` | integer | Total wall time in milliseconds |
| `total_cost_usd` | number | Accumulated cost in USD |
| `is_error` | boolean | `false` on success, `true` on failure |
| `usage` | object | Token counts (`input_tokens`, `output_tokens`, cache read/creation fields) |

### `exit_status` values (`json` and `stream-json`)

| Value | Meaning |
|-------|---------|
| `completed` | Agent finished normally (exit code `0`) |
| `max_turns_reached` | `--max-turns` / config turn cap (exit code `3`) |
| `budget_exceeded` | `--max-budget-usd` / config budget cap (exit code `3`) |
| `interrupted` | User cancelled (often exit `130` before a result line) |
| `rate_limited` | Provider rate limit with no automatic recovery (exit code `1`) |
| `max_wall_time_exceeded` | `[agent]` per-turn wall time exceeded (exit code `1`) |
| `max_tool_calls_per_turn` | `[agent]` tool-call cap for one user turn (exit code `1`) |
| `stall_detected` | Heuristic stall guard tripped (exit code `1`) |
| `malformed_model_output` | Invalid or duplicate `tool_use` blocks (exit code `1`) |
| `doom_loop` | Same tool error repeated beyond the configured threshold (exit code `1`) |
| `error` | Other failures: provider, tool, I/O, etc. (exit code `1`) |

The interactive TUI writes session `Result` lines with `exit_status` too; successful runs usually use `success` (same meaning as `completed` above). See [Exit codes](./exit-codes.md).

## stream-json

Newline-delimited JSON events emitted in real time as the agent runs. Each line is a self-contained JSON object. Suitable for parent processes that want to observe progress.

```bash
clido --output-format stream-json "count lines in src/main.rs"
```

```json
{"type":"tool_start","tool_name":"Read","input":{"file_path":"src/main.rs"},"turn":1}
{"type":"tool_done","tool_name":"Read","is_error":false,"duration_ms":8,"turn":1}
{"type":"assistant_text","text":"src/main.rs has 312 lines.","turn":1}
{"type":"result","session_id":"a1b2c3...","exit_status":"completed","total_cost_usd":0.0009,"num_turns":1,"duration_ms":2100}
```

### Event types

#### `tool_start`

Emitted when a tool call begins.

```json
{
  "type": "tool_start",
  "tool_name": "Bash",
  "input": { "command": "cargo check" },
  "tool_use_id": "toolu_01abc...",
  "turn": 2
}
```

| Field | Type | Description |
|-------|------|-------------|
| `tool_name` | string | Name of the tool |
| `input` | object | Tool input (tool-specific schema) |
| `tool_use_id` | string | Unique ID for this call |
| `turn` | integer | Turn number |

#### `tool_done`

Emitted when a tool call completes.

```json
{
  "type": "tool_done",
  "tool_name": "Bash",
  "is_error": false,
  "duration_ms": 1243,
  "tool_use_id": "toolu_01abc...",
  "turn": 2
}
```

| Field | Type | Description |
|-------|------|-------------|
| `tool_name` | string | Name of the tool |
| `is_error` | boolean | Whether the tool returned an error |
| `duration_ms` | integer | Execution time in milliseconds |
| `tool_use_id` | string | Matches the corresponding `tool_start` |
| `turn` | integer | Turn number |

#### `assistant_text`

Emitted when the assistant emits text (may occur multiple times per turn for streaming models).

```json
{
  "type": "assistant_text",
  "text": "The file has 312 lines.",
  "turn": 1
}
```

#### `result`

The final event, emitted after the agent finishes.

```json
{
  "type": "result",
  "session_id": "a1b2c3d4e5f6789abcdef0123456789abcdef01",
  "exit_status": "completed",
  "result": "src/main.rs has 312 lines.",
  "total_cost_usd": 0.0009,
  "num_turns": 1,
  "duration_ms": 2100,
  "model": "claude-sonnet-4-5",
  "is_error": false
}
```

`stream-json` result lines also include `usage` and `schema_version` in current builds.
```

### Example: consuming stream-json from another program

```bash
# Python example: print each tool call as it happens
clido --output-format stream-json "run all tests and fix any failures" |
  python3 -c "
import sys, json
for line in sys.stdin:
    ev = json.loads(line)
    if ev['type'] == 'tool_start':
        print(f'[{ev[\"tool_name\"]}] {ev[\"input\"]}')
    elif ev['type'] == 'result':
        print(f'Done: {ev[\"exit_status\"]} (${ev[\"total_cost_usd\"]:.4f})')
"
```
