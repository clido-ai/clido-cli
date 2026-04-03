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

```json
{
  "session_id": "a1b2c3d4e5f6789abcdef0123456789abcdef01",
  "result": "src/main.rs has 312 lines.",
  "total_cost_usd": 0.0009,
  "num_turns": 1,
  "duration_ms": 2100,
  "exit_status": "success",
  "model": "claude-sonnet-4-5",
  "provider": "anthropic"
}
```

### Field descriptions

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | string | Full 40-character session ID |
| `result` | string | Agent's final response text |
| `total_cost_usd` | number | Accumulated cost in USD |
| `num_turns` | integer | Number of agent turns |
| `duration_ms` | integer | Total wall time in milliseconds |
| `exit_status` | string | `success`, `max_turns`, `max_budget`, `error`, `interrupted` |
| `model` | string | Model used |
| `provider` | string | Provider used |

### `exit_status` values

| Value | Meaning |
|-------|---------|
| `success` | Agent completed the task |
| `max_turns` | Turn limit reached (exit code 3) |
| `max_budget` | Budget limit reached (exit code 3) |
| `error` | Agent or provider error (exit code 1) |
| `interrupted` | User interrupted with Ctrl+C (exit code 130) |

## stream-json

Newline-delimited JSON events emitted in real time as the agent runs. Each line is a self-contained JSON object. Suitable for parent processes that want to observe progress.

```bash
clido --output-format stream-json "count lines in src/main.rs"
```

```json
{"type":"tool_start","tool_name":"Read","input":{"file_path":"src/main.rs"},"turn":1}
{"type":"tool_done","tool_name":"Read","is_error":false,"duration_ms":8,"turn":1}
{"type":"assistant_text","text":"src/main.rs has 312 lines.","turn":1}
{"type":"result","session_id":"a1b2c3...","exit_status":"success","total_cost_usd":0.0009,"num_turns":1,"duration_ms":2100}
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
  "exit_status": "success",
  "total_cost_usd": 0.0009,
  "num_turns": 1,
  "duration_ms": 2100,
  "model": "claude-sonnet-4-5",
  "provider": "anthropic"
}
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
