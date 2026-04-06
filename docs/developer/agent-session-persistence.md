# Agent loop vs session file ordering

This note documents **commit order** and failure behavior for one outer user turn (`run` / `run_next_turn` / `run_continue`).

## Inner model + tool cycle

1. The loop calls the provider with compacted history and current tool schemas.
2. On success, the assistant message is appended to **in-memory** `history` and persisted as one `assistant_message` JSONL line **before** any tools run.
3. For `stop_reason == ToolUse`, each tool call is written as a `tool_call` line, then tools execute, then each result is written as a `tool_result` line. Tool results are then appended as a synthetic **user** role message in memory.
4. If persisting a line returns `Err`, the loop attempts to roll back: pop the in-memory assistant message if needed, and `truncate_to` the session file to the offset captured before that write (see `pre_assistant_file_offset` and similar patterns in `completion_loop_run`).

## Implications

- The session file can contain an `assistant_message` whose tools were **never** executed if the process crashes after step 2 and before tool lines are written. Resume logic must tolerate or repair such states (current loader skips orphan `tool_call` lines without a preceding assistant in some paths; see `session_lines_to_messages`).
- **Budget** `max_budget_usd` applies to **session-scoped** cumulative spend on the `AgentLoop` (across outer turns until `replace_history`). **`max_budget_usd_per_turn`** caps spend within a single `completion_loop_run` (one user message / continue).
