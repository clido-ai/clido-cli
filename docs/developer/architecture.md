# Architecture

This page describes the high-level design of clido: how the crates fit together, how data flows from user input to LLM response, and the key abstractions.

## High-level diagram

```
┌───────────────────────────────────────────────────────────┐
│                      clido-cli                            │
│                                                           │
│  ┌─────────────┐  ┌────────────┐  ┌───────────────────┐  │
│  │  TUI (tui)  │  │  run.rs    │  │  subcommands      │  │
│  │  ratatui    │  │  (non-TTY) │  │  sessions/audit/  │  │
│  └──────┬──────┘  └─────┬──────┘  │  memory/index/..  │  │
│         └───────────────┘         └───────────────────┘  │
│                 │                                         │
└─────────────────┼─────────────────────────────────────────┘
                  │  AgentLoop::run(config, provider, tools)
                  ▼
┌───────────────────────────────────────────────────────────┐
│                     clido-agent                           │
│                                                           │
│  ┌─────────────────────────────────────────────────────┐  │
│  │  AgentLoop                                          │  │
│  │  • Manages conversation history (Vec<Message>)      │  │
│  │  • Calls provider.complete(messages, tools)         │  │
│  │  • Dispatches tool calls from response              │  │
│  │  • Emits AgentEvents (tool_start, tool_done, text)  │  │
│  │  • Checks turn + budget limits                      │  │
│  │  • Writes SessionLines to SessionWriter             │  │
│  └──────────────────────┬──────────────────────────────┘  │
└─────────────────────────┼─────────────────────────────────┘
          ┌───────────────┼───────────────────┐
          ▼               ▼                   ▼
┌─────────────────┐ ┌──────────────┐ ┌───────────────────┐
│ clido-providers │ │ clido-tools  │ │  clido-storage    │
│                 │ │              │ │                   │
│  ModelProvider  │ │  Tool trait  │ │  SessionWriter    │
│  trait          │ │  ToolRegistry│ │  AuditLog         │
│                 │ │  Bash/Read/  │ │  SessionReader    │
│  Anthropic      │ │  Write/Edit/ │ │  list_sessions    │
│  OpenAI         │ │  Glob/Grep/  │ └───────────────────┘
│  OpenRouter     │ │  SemanticSearch
│  Local          │ │  McpTool     │
└─────────────────┘ └──────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────┐
│  Supporting crates                                       │
│                                                          │
│  clido-core     — AgentConfig, types, pricing, errors   │
│  clido-context  — Token counting, context compaction    │
│  clido-memory   — SQLite memory store (FTS5)            │
│  clido-index    — File + symbol index (SemanticSearch)  │
│  clido-workflows — YAML workflow executor               │
│  clido-planner  — LLM-based task decomposition (DAG)    │
└─────────────────────────────────────────────────────────┘
```

## Data flow

A single agent turn proceeds as follows:

```
User input (string)
      │
      ▼
AgentLoop
  1. Append UserMessage to history
  2. Inject memory + context (clido-context)
  3. Call provider.complete(history, tool_schemas)
      │
      ▼
  Provider (clido-providers)
  4. Serialise messages to provider wire format
  5. HTTP request to LLM API
  6. Deserialise response → ModelResponse
      │
      ▼
  AgentLoop
  7. Parse response content blocks
  8. If stop_reason == EndTurn: emit text, return
  9. If stop_reason == ToolUse:
     a. Emit on_tool_start events
     b. Execute tool calls (parallel for read-only tools)
     c. Emit on_tool_done events
     d. Append AssistantMessage + ToolResult to history
     e. Write to SessionWriter + AuditLog
    10. Loop back to step 2
```

## Session lifecycle

```
clido "prompt"
  │
  ├─ No --continue / --resume
  │    └─ Create new SessionWriter, generate session_id (UUID)
  │
  ├─ --continue
  │    └─ Find latest session for project dir
  │         └─ Load SessionReader, reconstruct messages
  │
  └─ --resume <id>
       └─ Find session file by ID prefix
            └─ Check stale files (unless --resume-ignore-stale)
                 └─ Load SessionReader, reconstruct messages

Agent runs...

  └─ AgentLoop writes each turn to SessionWriter as JSONL lines:
       UserMessage, AssistantMessage, ToolCall, ToolResult

Session ends (end_turn / turn_limit / budget_limit / error)
  └─ SessionWriter writes Result line (exit_status, cost, turns, duration)
```

## Event system

The `EventEmitter` trait (in `clido-agent`) is the bridge between the agent loop and the UI layer:

```rust
#[async_trait]
pub trait EventEmitter: Send + Sync {
    async fn on_tool_start(&self, name: &str, input: &serde_json::Value);
    async fn on_tool_done(&self, name: &str, is_error: bool, diff: Option<String>);
    async fn on_assistant_text(&self, text: &str);
}
```

The CLI wires up different implementations:

| Mode | Implementation |
|------|---------------|
| TUI | Sends events to a Tokio channel; the Ratatui render loop reads them |
| `--output-format stream-json` | Serialises events to stdout as JSONL |
| `--output-format text` | Prints tool lines with `[Turn N]` prefix |
| `--quiet` | No-op implementation |

## Permission system

The `AskUser` trait is the permission hook:

```rust
#[async_trait]
pub trait AskUser: Send + Sync {
    async fn ask(&self, tool_name: &str, input: &serde_json::Value) -> bool;
}
```

Before executing a state-changing tool (Bash, Write, Edit), the agent calls `ask_user.ask()`. The return value determines whether to proceed.

| Permission mode | `AskUser` implementation |
|----------------|------------------------|
| `default` (TUI) | Shows a modal and waits for `y`/`n` |
| `default` (non-TTY) | Auto-allows (pass-through) |
| `accept-all` | Always returns `true` |
| `plan` | Always returns `false` |

Read-only tools (Read, Glob, Grep, SemanticSearch) bypass the permission check entirely.

## Context management

`clido-context` handles two concerns:

1. **Token estimation** — approximate token counts for messages (used for compaction triggering and cost tracking)
2. **Context assembly** — prepends system prompt, memory injection, and project context before each provider call; compacts old history when the context window nears its limit

The compaction strategy summarises the oldest turns in the conversation into a single compressed message, preserving only the most important content.

## Concurrency model

The agent loop runs in a single Tokio async task. Tool execution uses `tokio::task::spawn` for parallel tool calls, bounded by a `tokio::sync::Semaphore` (size = `max_parallel_tools`).

Only read-only tools run in parallel. Write, Edit, and Bash always run serially to prevent race conditions.

## Agent loop hardening (production plan)

The full-scope plan for a production-grade agent loop is **[agent-loop-production-plan.md](./agent-loop-production-plan.md)**. It is a single non-optional backlog: validation, typed errors, retries, stall/doom detection, streaming semantics, session integrity, metrics, configuration, documentation, and a verifiable failure-mode matrix.
