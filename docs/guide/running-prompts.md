# Running Prompts

clido can be used non-interactively from the command line. This is useful for scripting, CI pipelines, and one-shot tasks where you do not want the TUI.

## Basic usage

Pass your prompt as a positional argument:

```bash
clido "explain the purpose of each function in src/lib.rs"
```

Or use the `run` subcommand (useful in scripts to avoid ambiguity with flags):

```bash
clido run "explain the purpose of each function in src/lib.rs"
```

Both forms are equivalent.

## Reading from stdin

Pipe a prompt through stdin:

```bash
echo "what does this code do?" | clido
cat error.log | clido "explain these errors and suggest fixes"
```

When stdin is not a TTY and no prompt is given on the command line, clido reads stdin as the prompt.

::: tip Combining stdin and file context
You can pass a prompt on the command line and pipe supplementary content in the same invocation, though this is uncommon. Most of the time the agent uses its file-reading tools directly.
:::

## Output formats

Control output format with `--output-format`:

### Text (default)

Human-readable output. The agent's final response is printed, along with tool activity and a cost footer:

```bash
clido "count lines in main.rs"
```

```
[Turn 1] Reading src/main.rs...

src/main.rs has 312 lines.

  Cost: $0.0009  Turns: 1  Time: 2.1s
```

### JSON

A single JSON object with the full session result:

```bash
clido --output-format json "count lines in main.rs"
```

```json
{
  "session_id": "a1b2c3d4e5f6...",
  "result": "src/main.rs has 312 lines.",
  "total_cost_usd": 0.0009,
  "num_turns": 1,
  "duration_ms": 2100,
  "exit_status": "success"
}
```

### Stream JSON

Newline-delimited JSON events emitted as they happen. Useful for integrating clido into a parent process that wants to observe progress:

```bash
clido --output-format stream-json "count lines in main.rs"
```

```json
{"type":"tool_start","tool_name":"Read","input":{"file_path":"src/main.rs"}}
{"type":"tool_done","tool_name":"Read","is_error":false,"duration_ms":12}
{"type":"assistant_text","text":"src/main.rs has 312 lines."}
{"type":"result","exit_status":"success","total_cost_usd":0.0009,"num_turns":1,"duration_ms":2100}
```

See [Output Formats](/docs/reference/output-formats) for the full schema of each format.

## Quiet mode

Suppress spinner, tool lifecycle output, and the cost footer — only print the agent's final response:

```bash
clido --quiet "what is the sum of 2 + 2?"
# 4
```

Quiet mode is useful when the output will be consumed by another tool:

```bash
SUMMARY=$(clido --quiet "summarise CHANGELOG.md in one paragraph")
echo "$SUMMARY" | pbcopy
```

## Print mode

`--print` (or `-p`) is similar to `--quiet` but also disables the REPL. It is intended for non-interactive pipelines where you want to ensure clido never blocks waiting for user input:

```bash
clido --print "list all TODO comments in this repo"
```

## Limiting turns and budget

Prevent runaway agents with explicit limits:

```bash
# Stop after 5 turns
clido --max-turns 5 "refactor the authentication module"

# Stop if cost exceeds $0.50
clido --max-budget-usd 0.50 "generate comprehensive tests for all public APIs"
```

When a limit is reached, clido exits with code 3 (soft limit) and prints whatever the agent has produced so far.

Default values (from config):
- `--max-turns`: 50
- `--max-budget-usd`: 5.0

## Permission modes

Control how the agent handles state-changing tool calls:

| Mode | Behaviour |
|------|-----------|
| `default` | Prompt for permission before each state-changing tool call (TUI) or allow all (non-TUI) |
| `accept-all` | Allow all tool calls without prompting |
| `plan` | No tool calls allowed; agent can only plan and respond with text |

```bash
# Trust the agent completely
clido --permission-mode accept-all "apply all the TODO items in this file"

# Read-only — agent cannot modify files or run commands
clido --permission-mode plan "review this codebase and suggest improvements"
```

::: warning
`accept-all` gives the agent full access to your filesystem and shell. Only use it when you trust the prompt and have reviewed the task scope.
:::

## Overriding provider and model

```bash
# Use a specific model for this invocation
clido --model claude-haiku-4-5 "quick question: what is 17 * 23?"

# Use a different provider
clido --provider openrouter --model anthropic/claude-3-5-sonnet "refactor this"
```

## Working directory

By default clido uses the current directory as the workspace root. Override with `--workdir` (or `-C`):

```bash
clido -C /path/to/project "add type hints to all functions"
```

## Script usage with JSON output

Here is a shell script pattern for using clido's JSON output programmatically:

```bash
#!/usr/bin/env bash
set -euo pipefail

RESULT=$(clido --output-format json --quiet --max-turns 10 \
  "analyse src/ and list any files with cyclomatic complexity > 10")

# Use jq to extract the result text
echo "$RESULT" | jq -r '.result'
EXIT=$(echo "$RESULT" | jq -r '.exit_status')

if [ "$EXIT" != "success" ]; then
  echo "Agent did not complete successfully: $EXIT" >&2
  exit 1
fi
```

## All relevant flags

| Flag | Description |
|------|-------------|
| `--output-format` | `text` (default), `json`, or `stream-json` |
| `--input-format` | `text` (default) or `stream-json` |
| `--quiet` / `-q` | Suppress tool output and cost footer |
| `--print` / `-p` | Non-interactive; no REPL |
| `--max-turns N` | Maximum agent turns (default: 50) |
| `--max-budget-usd N` | Maximum cost in USD |
| `--permission-mode` | `default`, `accept-all`, or `plan` |
| `--model` | Model override |
| `--provider` | Provider override |
| `--workdir` / `-C` | Working directory |
| `--continue` | Resume newest session |
| `--resume ID` | Resume session by ID prefix |

See [All Flags](/docs/reference/flags) for the complete reference.
