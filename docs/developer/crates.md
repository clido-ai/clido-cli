# Crate Overview

clido is a Cargo workspace with 11 crates. Each crate has a single, well-defined responsibility.

## Workspace layout

```
crates/
  clido-cli/         — Binary; CLI parsing, TUI, command dispatch
  clido-agent/       — Agent loop, EventEmitter, AskUser traits
  clido-providers/   — LLM provider implementations
  clido-tools/       — Tool trait, registry, built-in tools
  clido-core/        — Shared types, config, pricing, errors
  clido-storage/     — Session JSONL, audit log, paths
  clido-context/     — Token counting, context assembly, compaction
  clido-memory/      — Long-term memory (SQLite + FTS5)
  clido-index/       — Repository file+symbol index (SemanticSearch)
  clido-workflows/   — YAML workflow executor
  clido-planner/     — LLM-based task decomposition (DAG)
```

## `clido-cli`

The binary crate. Contains Clap argument parsing, command dispatch, the Ratatui TUI, and all top-level commands (sessions, audit, memory, index, workflow, etc.).

**Key modules:**

| Module | Purpose |
|--------|---------|
| `cli.rs` | Clap `Cli` struct and all subcommand enums |
| `main.rs` | Entry point, dispatch, terminal restoration |
| `tui.rs` | Ratatui TUI: render loop, input handling, state |
| `run.rs` | Non-TTY agent invocation |
| `setup.rs` | `clido init` wizard |
| `doctor.rs` | `clido doctor` health checks |
| `sessions.rs` | `clido sessions` commands |
| `audit_cmd.rs` | `clido audit` |
| `memory_cmd.rs` | `clido memory` commands |
| `index_cmd.rs` | `clido index` commands |
| `workflow.rs` | `clido workflow` commands |
| `stats.rs` | `clido stats` |
| `config.rs` | `clido config show/set` |
| `models.rs` | `clido list-models`, `fetch-models` |
| `pricing_cmd.rs` | `clido update-pricing` |
| `agent_setup.rs` | Provider + registry construction |
| `agent_loop.rs` | CLI-level agent loop wrapper |

**Dependencies:** `clido-agent`, `clido-tools`, `clido-core`, `clido-storage`, `clido-context`, `clido-providers`, `clido-memory`, `clido-index`, `clido-workflows`, `ratatui`, `crossterm`, `clap`, `tokio`

## `clido-agent`

The agent loop. Manages conversation history, calls the provider, dispatches tool calls, emits events, enforces turn/budget limits, and writes to the session file.

**Key types:**

| Type | Purpose |
|------|---------|
| `AgentLoop` | Main agent struct; holds history, provider, tools, config |
| `AgentLoop::run()` | Execute the agent loop for one session turn batch |
| `EventEmitter` | Trait for observing tool calls in real time |
| `AskUser` | Trait for permission prompts |
| `session_lines_to_messages()` | Reconstruct history from JSONL for resume |
| `SubAgent` | Spawns a child agent for workflow steps |

**Dependencies:** `clido-core`, `clido-providers`, `clido-tools`, `clido-storage`, `clido-context`, `clido-memory`, `tokio`, `async-trait`

## `clido-providers`

LLM provider implementations. Each provider wraps an HTTP client and translates between clido's internal `Message` format and the provider's wire format.

**Key types:**

| Type | Purpose |
|------|---------|
| `ModelProvider` | Trait: `complete()`, `stream_complete()`, `name()` |
| `AnthropicProvider` | Claude API implementation |
| `OpenAIProvider` | OpenAI-compatible implementation |
| `OpenRouterProvider` | OpenRouter wrapper (extends OpenAI) |
| `LocalProvider` | Ollama/local model wrapper |
| `make_provider()` | Factory function: constructs provider from config |

**Dependencies:** `clido-core`, `reqwest`, `serde`, `tokio`

## `clido-tools`

The `Tool` trait, `ToolRegistry`, and all built-in tool implementations.

**Key types:**

| Type | Purpose |
|------|---------|
| `Tool` | Trait: `name()`, `description()`, `schema()`, `execute()` |
| `ToolRegistry` | HashMap of tools; supports allow/disallow filtering |
| `BashTool` | Shell command execution with timeout |
| `ReadTool` | File reading with line-range slicing |
| `WriteTool` | File creation and overwrite |
| `EditTool` | Precise string replacement in files |
| `GlobTool` | File pattern matching |
| `GrepTool` | Regex content search |
| `SemanticSearchTool` | Full-text search over repository index |
| `ExitPlanModeTool` | Signals the agent to end plan-only mode |
| `McpTool` | Wrapper for a single MCP server tool |
| `PathGuard` | Enforces workspace root path restrictions |
| `FileTracker` | Tracks read files for stale detection |
| `default_registry()` | Build the default V1 tool set |

**Dependencies:** `clido-core`, `clido-context`, `clido-index`, `async-trait`, `serde_json`, `tokio`

## `clido-core`

Shared types, configuration structs, pricing data, and error types. This crate has no dependencies on other clido crates.

**Key types:**

| Type | Purpose |
|------|---------|
| `AgentConfig` | Runtime agent configuration |
| `ProviderConfig` | Provider credentials and model |
| `PermissionMode` | Enum: `Default`, `AcceptAll`, `PlanOnly` |
| `HooksConfig` | Pre/post tool hooks |
| `ConfigFile` | Deserialised config.toml structure |
| `LoadedConfig` | Merged config (global + project) |
| `ProfileEntry` | One provider+model+credentials entry |
| `Message`, `ContentBlock`, `Role` | LLM message types |
| `ModelResponse`, `StopReason`, `Usage` | Provider response types |
| `ToolSchema` | Tool description for the LLM |
| `PricingTable` | Model pricing data |
| `ClidoError` | Top-level error enum |

**Dependencies:** `serde`, `toml`, `directories` (no other clido crates)

## `clido-storage`

Session JSONL files, the audit log, and filesystem paths.

**Key types:**

| Type | Purpose |
|------|---------|
| `SessionLine` | Enum of all JSONL line variants |
| `SessionWriter` | Append-only writer for a session file |
| `SessionReader` | Iterator over session lines |
| `SessionSummary` | Summary for `sessions list` |
| `AuditLog` | Append-only audit log writer |
| `AuditEntry` | One audit log record |
| `data_dir()` | Platform data directory |
| `session_file_path()` | Full path for a session file |
| `list_sessions()` | All sessions, sorted by recency |
| `stale_paths()` | Files modified since session end |

**Dependencies:** `clido-core`, `serde`, `serde_json`, `chrono`, `directories`

## `clido-context`

Token counting and context assembly (system prompt + memory + history) before each provider call.

**Key functions:**

| Function | Purpose |
|----------|---------|
| `assemble()` | Build final message list for provider call |
| `estimate_tokens_str()` | Approximate token count for a string |
| `DEFAULT_MAX_CONTEXT_TOKENS` | Default context window (200,000) |
| `DEFAULT_COMPACTION_THRESHOLD` | Default compaction trigger (**0.58**) |

Also contains a `ReadCache` for deduplicating file reads within a session.

**Dependencies:** `clido-core`, `clido-memory`

## `clido-memory`

Long-term memory store backed by SQLite with FTS5 full-text search.

**Key types:**

| Type | Purpose |
|------|---------|
| `MemoryStore` | SQLite-backed memory store |
| `MemoryStore::search()` | FTS5 query |
| `MemoryStore::insert()` | Add a memory record |
| `MemoryStore::list()` | Most-recent-first listing |
| `MemoryStore::prune()` | Keep N most recent, delete rest |
| `MemoryStore::reset()` | Delete all memories |

**Dependencies:** `rusqlite`, `serde`, `chrono`

## `clido-index`

Repository file and symbol index for the `SemanticSearch` tool.

**Key types:**

| Type | Purpose |
|------|---------|
| `IndexStore` | SQLite-backed index |
| `IndexStore::build()` | Incremental index build |
| `IndexStore::search()` | FTS5 symbol search |
| `IndexStore::stats()` | File/symbol counts |
| `IndexStore::clear()` | Delete all index data |

**Dependencies:** `rusqlite`, `tree-sitter` (for symbol extraction), `walkdir`

## `clido-workflows`

YAML workflow parser, validator, and executor.

**Key types:**

| Type | Purpose |
|------|---------|
| `Workflow` | Deserialised workflow YAML |
| `WorkflowStep` | One step definition |
| `WorkflowRunner` | Executes a workflow against an agent |
| `WorkflowRunner::validate()` | Schema + dependency checks |
| `WorkflowRunner::inspect()` | Returns the step DAG |
| `WorkflowRunner::check()` | Preflight checks |

**Dependencies:** `clido-agent`, `clido-core`, `serde_yaml`, `tokio`

## `clido-planner`

Experimental task decomposition. Sends a single LLM call to decompose the user's prompt into a DAG of subtasks.

**Key types:**

| Type | Purpose |
|------|---------|
| `Planner` | Makes the planning LLM call |
| `Plan` | Deserialised task graph (nodes + edges) |
| `PlanNode` | One task in the graph |
| `Planner::plan()` | Returns `Ok(Plan)` or error (triggers fallback) |

**Dependencies:** `clido-providers`, `clido-core`, `serde_json`, `tokio`
