# Agent loop: interrupt and partial-turn semantics

This document describes how **user interrupt** interacts with the completion loop and history. It reflects the current `AgentLoop` implementation.

## During `provider.complete`

- A **cancel** flag may be set while the model is generating.
- When the call returns, the loop checks cancel **before** appending the assistant message or running tools.
- If cancelled: the response is **discarded** (no assistant message, no tools) and the loop returns `ClidoError::Interrupted`.

## After `complete`, before tools

- If cancel is set after a successful `complete` but **before** tool execution begins, the loop returns `Interrupted` **without** running tools. The assistant message has **already** been appended for that turn iteration (consistent with a partial assistant turn).

## During or after tool execution

- If cancel fires **after** tools have started, the loop **finishes the current tool batch**, appends all `ToolResult` blocks to history, then returns `Interrupted`. This avoids leaving dangling `tool_use` ids without results in the transcript.

## Malformed `tool_use`

- If the provider reports `StopReason::ToolUse` but content has **no** valid `tool_use` blocks (or duplicate ids), the loop returns `ClidoError::MalformedModelOutput` **without** appending the assistant message (validation runs before commit).

## Session file

- Session JSONL follows the same ordering as in-memory history. Failed turns may truncate history per `ClidoError::should_truncate_history_after_failed_run` (see `clido-core`).
