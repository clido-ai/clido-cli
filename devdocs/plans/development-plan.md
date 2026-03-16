# Alfred — Rust CLI Coding Agent: Development Roadmap

**Project:** `alfred` — a local-first, multi-provider CLI coding agent in Rust
**Based on:** Reverse-engineering of Claude CLI and Cursor agent (see `devdocs/REPORT.md`, `devdocs/ARTIFACTS.md`)
**Target:** Production-ready system that reproduces and improves on modern CLI coding agents

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Rust Workspace Structure](#rust-workspace-structure)
3. [Phase 1 — Foundation Setup](#phase-1--foundation-setup)
4. [Phase 2 — Proof of Concept](#phase-2--proof-of-concept)
5. [Phase 3 — Minimal Viable Agent](#phase-3--minimal-viable-agent)
6. [Phase 4 — Feature Expansion](#phase-4--feature-expansion)
7. [Phase 5 — Reliability Improvements](#phase-5--reliability-improvements)
8. [Phase 6 — Performance Optimization](#phase-6--performance-optimization)
9. [Phase 7 — Security and Sandboxing](#phase-7--security-and-sandboxing)
10. [Phase 8 — Developer Experience](#phase-8--developer-experience)
11. [Phase 9 — Production Readiness](#phase-9--production-readiness)
12. [Dependency Map](#dependency-map)
13. [Recommended Crates Reference](#recommended-crates-reference)

---

## Architecture Overview

```
alfred (workspace)
│
├── crates/
│   ├── alfred-cli/        # CLI entry point (clap, streaming output, plan display)
│   ├── alfred-agent/      # Agent loop, turn management, session state, subagents
│   ├── alfred-tools/      # Tool trait + all tool implementations
│   ├── alfred-context/    # Context assembly, token budgeting, compaction
│   ├── alfred-providers/  # Model provider abstraction + implementations
│   ├── alfred-storage/    # Session persistence, project config
│   ├── alfred-memory/     # Short-term + long-term memory (sqlite/sled)
│   ├── alfred-planner/    # Task graph, planner trait, DAG executor (optional advanced)
│   ├── alfred-index/      # Repository indexing: tree-sitter, symbol index (optional)
│   └── alfred-core/       # Shared types, errors, config structs
```

**Execution flow (from trace evidence):**

```
User input
  → Context engine assembles: system_prompt + tool_guidance + history + tool_results
  → Provider sends request to model
  → Model returns: text block and/or tool_use blocks
  → Tool executor runs tools (parallel for read-only; sequential for state-changing;
    bounded by semaphore)
  → Results appended to history as tool_result user blocks
  → Repeat until no tool_use in response or max_turns reached
  → Emit result (duration_ms, num_turns, total_cost_usd, usage)
```

**Optional planner flow (Phase 4.8+):**

```
User input
  → Planner model call → structured TaskGraph (JSON)
  → Deterministic DAG executor resolves dependencies
  → Tool execution / subagents per task node
  → Reflection step: model reviews results, updates graph if needed
  → Final output
```

---

## Rust Workspace Structure

```
alfred/
├── Cargo.toml                  # workspace manifest
├── Cargo.lock
├── .cargo/
│   └── config.toml             # profile optimizations, target settings
├── crates/
│   ├── alfred-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs        # thiserror error types
│   │       ├── types.rs        # Message, ContentBlock, ToolUse, ToolResult, etc.
│   │       └── config.rs       # AgentConfig, ProviderConfig
│   ├── alfred-tools/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── registry.rs     # ToolRegistry
│   │       ├── cache.rs        # LRU file read cache
│   │       ├── schema.rs       # JSON Schema generation
│   │       ├── read.rs
│   │       ├── write.rs
│   │       ├── edit.rs
│   │       ├── glob.rs
│   │       ├── grep.rs
│   │       ├── bash.rs
│   │       └── mcp.rs          # MCP client tool wrapper
│   ├── alfred-providers/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs     # ModelProvider trait
│   │       ├── anthropic.rs
│   │       ├── openai.rs
│   │       ├── openrouter.rs
│   │       ├── alibaba.rs
│   │       └── local.rs
│   ├── alfred-context/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── builder.rs      # ContextBuilder
│   │       ├── compaction.rs   # token counting + compaction
│   │       ├── guidance.rs     # tool usage guidance prompt injection
│   │       └── project.rs      # ALFRED.md / CLAUDE.md loader
│   ├── alfred-storage/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── session.rs      # JSONL session read/write
│   │       └── paths.rs        # XDG / platform paths
│   ├── alfred-memory/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── short_term.rs   # in-session working memory
│   │       ├── long_term.rs    # cross-session persistent memory (sqlite)
│   │       └── retrieval.rs    # memory lookup by relevance
│   ├── alfred-planner/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── planner.rs      # Planner trait
│   │       ├── task_graph.rs   # Task, TaskGraph, DAG resolution
│   │       └── executor.rs     # TaskExecutor, dependency-ordered execution
│   ├── alfred-index/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── file_index.rs   # file path + metadata index
│   │       ├── symbol_index.rs # tree-sitter symbol extraction
│   │       └── search.rs       # tantivy full-text search
│   ├── alfred-agent/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── loop.rs         # AgentLoop, turn execution
│   │       ├── executor.rs     # tool dispatch, parallelism, semaphore
│   │       ├── subagent.rs     # SubAgent, SubAgentManager
│   │       ├── permissions.rs  # allow/deny, plan mode
│   │       └── events.rs       # AgentEvent stream, hooks
│   └── alfred-cli/
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── commands/
│           │   ├── run.rs
│           │   ├── resume.rs
│           │   ├── list_sessions.rs
│           │   └── doctor.rs
│           └── output/
│               ├── streaming.rs
│               ├── plan_display.rs   # live plan visualization
│               └── json.rs
├── tests/
│   ├── integration/
│   └── fixtures/
└── devdocs/
```

---

## Phase 1 — Foundation Setup

**Goal:** Working Rust workspace with shared types and CI. No agent logic yet.
**Exit criteria:** `cargo build --workspace` succeeds; all crates compile; basic CI runs.

---

### Milestone 1.1 — Workspace Initialization

#### 1.1.1 Initialize the Rust workspace

1. Run `cargo init --name alfred` in the repo root to get a workspace skeleton (or create manually).
2. Replace the root `Cargo.toml` with a workspace manifest:

   ```toml
   [workspace]
   members = [
     "crates/alfred-core",
     "crates/alfred-tools",
     "crates/alfred-providers",
     "crates/alfred-context",
     "crates/alfred-storage",
     "crates/alfred-memory",
     "crates/alfred-planner",
     "crates/alfred-index",
     "crates/alfred-agent",
     "crates/alfred-cli",
   ]
   resolver = "2"
   ```

3. Create `.cargo/config.toml` with:
   - `[profile.dev] opt-level = 1` (faster incremental builds)
   - `[profile.release] lto = "thin"`, `codegen-units = 1`
4. Add a top-level `rust-toolchain.toml` pinning a stable channel (e.g. `channel = "1.78"`).
5. Verify: `cargo metadata --no-deps` outputs all workspace members.

#### 1.1.2 Create all crate skeletons

1. For each crate in `crates/`:
   - `cargo new --lib crates/alfred-core`
   - `cargo new --lib crates/alfred-tools`
   - `cargo new --lib crates/alfred-providers`
   - `cargo new --lib crates/alfred-context`
   - `cargo new --lib crates/alfred-storage`
   - `cargo new --lib crates/alfred-memory`
   - `cargo new --lib crates/alfred-planner`
   - `cargo new --lib crates/alfred-index`
   - `cargo new --lib crates/alfred-agent`
   - `cargo new --bin crates/alfred-cli`
2. Each `lib.rs` starts with `// placeholder` only.
3. Verify: `cargo build --workspace` compiles without errors.

#### 1.1.3 Add workspace-level dependencies

1. In root `Cargo.toml` add `[workspace.dependencies]` block with pinned versions:

   ```toml
   [workspace.dependencies]
   tokio = { version = "1", features = ["full"] }
   serde = { version = "1", features = ["derive"] }
   serde_json = "1"
   anyhow = "1"
   thiserror = "1"
   tracing = "0.1"
   tracing-subscriber = { version = "0.3", features = ["env-filter"] }
   clap = { version = "4", features = ["derive"] }
   reqwest = { version = "0.12", features = ["json", "stream"] }
   tokio-stream = "0.1"
   futures = "0.3"
   uuid = { version = "1", features = ["v4"] }
   chrono = { version = "0.4", features = ["serde"] }
   directories = "5"
   glob = "0.3"
   ignore = "0.4"
   regex = "1"
   grep = "0.3"
   async-trait = "0.1"
   toml = "0.8"
   ```

2. Reference these from child crates with `{ workspace = true }` — no version duplication.
3. Run `cargo check --workspace` to confirm resolution.

---

### Milestone 1.2 — Core Types (`alfred-core`)

**Dependency:** 1.1 complete.

#### 1.2.1 Define the message/content type hierarchy

1. Open `crates/alfred-core/src/types.rs`.
2. Define `Role` enum: `User`, `Assistant`, `System`.
3. Define `ContentBlock` enum (mirrors Anthropic API observed in traces):

   ```rust
   pub enum ContentBlock {
       Text { text: String },
       ToolUse { id: String, name: String, input: serde_json::Value },
       ToolResult { tool_use_id: String, content: String, is_error: bool },
       Thinking { thinking: String },  // for extended thinking support
   }
   ```

4. Derive `Serialize`, `Deserialize`, `Debug`, `Clone` on all types.
5. Define `Message`:

   ```rust
   pub struct Message {
       pub role: Role,
       pub content: Vec<ContentBlock>,
   }
   ```

6. Define `Usage`:

   ```rust
   pub struct Usage {
       pub input_tokens: u64,
       pub output_tokens: u64,
       pub cache_creation_input_tokens: Option<u64>,
       pub cache_read_input_tokens: Option<u64>,
   }
   ```

7. Define `ModelResponse`:

   ```rust
   pub struct ModelResponse {
       pub id: String,
       pub model: String,
       pub content: Vec<ContentBlock>,
       pub stop_reason: StopReason,
       pub usage: Usage,
   }
   ```

8. Define `StopReason` enum: `EndTurn`, `ToolUse`, `MaxTokens`, `StopSequence`.
9. Write unit tests in `types.rs` verifying JSON round-trip for each type.
10. Run `cargo test -p alfred-core`.

#### 1.2.2 Define error types

1. Open `crates/alfred-core/src/error.rs`.
2. Use `thiserror`:

   ```rust
   #[derive(thiserror::Error, Debug)]
   pub enum AlfredError {
       #[error("provider error: {0}")]
       Provider(String),
       #[error("tool error: {tool_name}: {message}")]
       Tool { tool_name: String, message: String },
       #[error("context limit exceeded: {tokens} tokens")]
       ContextLimit { tokens: u64 },
       #[error("session not found: {session_id}")]
       SessionNotFound { session_id: String },
       #[error("permission denied: {tool_name}")]
       PermissionDenied { tool_name: String },
       #[error("planner error: {0}")]
       Planner(String),
       #[error("io error: {0}")]
       Io(#[from] std::io::Error),
       #[error("json error: {0}")]
       Json(#[from] serde_json::Error),
       #[error(transparent)]
       Other(#[from] anyhow::Error),
   }
   pub type Result<T> = std::result::Result<T, AlfredError>;
   ```

3. Add `alfred-core` to `alfred-agent`, `alfred-tools`, `alfred-providers`, `alfred-planner`, `alfred-memory`, `alfred-index` as a dependency.
4. Run `cargo check --workspace`.

#### 1.2.3 Define agent configuration types

1. In `crates/alfred-core/src/config.rs`:
   - Define `AgentConfig`: `max_turns: u32`, `max_budget_usd: Option<f64>`, `model: String`, `system_prompt: Option<String>`, `permission_mode: PermissionMode`, `use_planner: bool`, `use_index: bool`.
   - Define `PermissionMode` enum: `Default`, `AcceptAll`, `PlanOnly`.
   - Define `ProviderConfig`: `provider_type: ProviderType`, `api_key: Option<String>`, `base_url: Option<String>`, `model: String`.
   - Define `ProviderType` enum: `Anthropic`, `OpenAI`, `OpenRouter`, `AlibabaCloud`, `Local`.
2. Derive `Serialize`, `Deserialize`, `Debug`, `Clone`, `Default` where appropriate.
3. Write a unit test loading a `AgentConfig` from a JSON string.

---

### Milestone 1.3 — Tracing and Logging Setup

**Dependency:** 1.2 complete.

#### 1.3.1 Initialize tracing in the CLI

1. In `alfred-cli/src/main.rs`:

   ```rust
   tracing_subscriber::fmt()
       .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
       .init();
   ```

2. Add `RUST_LOG=alfred=debug` guidance in README.
3. Add `tracing::info!`, `tracing::debug!` stubs in `alfred-agent` crate.
4. Test: run `RUST_LOG=debug cargo run -p alfred-cli` and confirm tracing output.

---

### Milestone 1.4 — CI Setup

**Dependency:** 1.1–1.3 complete.

#### 1.4.1 Add GitHub Actions workflow

1. Create `.github/workflows/ci.yml`:
   - `cargo fmt --check`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
   - `cargo build --workspace --release`
2. Add `.rustfmt.toml` with project formatting preferences.
3. Add `.clippy.toml` or `#![allow(...)]` only for intentional exceptions.
4. Verify CI passes on a clean push.

---

## Phase 2 — Proof of Concept

**Goal:** Single hardcoded model call with a single tool, no persistence. Demonstrates the core loop works end-to-end.
**Exit criteria:** Running `cargo run -p alfred-cli -- -p "list files in current directory"` makes one model API call, receives a `tool_use` for `bash`, executes it, feeds result back, and prints final text response.

---

### Milestone 2.1 — Minimal Provider: Anthropic

**Dependency:** Phase 1 complete.

#### 2.1.1 Implement `ModelProvider` trait

1. In `alfred-providers/src/provider.rs`:

   ```rust
   #[async_trait::async_trait]
   pub trait ModelProvider: Send + Sync {
       async fn complete(
           &self,
           messages: &[Message],
           tools: &[ToolSchema],
           config: &AgentConfig,
       ) -> Result<ModelResponse>;

       async fn complete_stream(
           &self,
           messages: &[Message],
           tools: &[ToolSchema],
           config: &AgentConfig,
       ) -> Result<impl Stream<Item = Result<StreamEvent>>>;
   }
   ```

2. Define `ToolSchema`:

   ```rust
   pub struct ToolSchema {
       pub name: String,
       pub description: String,
       pub input_schema: serde_json::Value,  // JSON Schema object
   }
   ```

3. Define `StreamEvent`:

   ```rust
   pub enum StreamEvent {
       TextDelta(String),
       ToolUseStart { id: String, name: String },
       ToolUseDelta { id: String, partial_json: String },
       ToolUseEnd { id: String },
       MessageDelta { stop_reason: StopReason, usage: Usage },
   }
   ```

#### 2.1.2 Implement Anthropic HTTP client

1. In `alfred-providers/src/anthropic.rs`:
   - Define `AnthropicProvider` struct holding `reqwest::Client`, `api_key: String`, `model: String`.
   - Implement `new(api_key: String, model: String) -> Self`.
   - Implement `complete()`:
     - Build request body JSON:

       ```json
       {
         "model": "...",
         "max_tokens": 4096,
         "system": "...",
         "messages": [...],
         "tools": [...]
       }
       ```

     - Map `Message` → Anthropic API format (handle `ToolResult` blocks as `user` role content).
     - Map `ToolSchema` → Anthropic tool definition format.
     - POST to `https://api.anthropic.com/v1/messages`.
     - Set headers: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`.
     - Deserialize response into `ModelResponse`.
   - Return `AlfredError::Provider` on non-200 status with response body.
2. Write a unit test mocking the HTTP response with `wiremock` or `httpmock`.
3. Add integration test behind `#[cfg(feature = "integration")]` that calls real API.

#### 2.1.3 Wire provider into CLI (hardcoded)

1. In `alfred-cli/src/main.rs` (temporary PoC wiring):
   - Read `ANTHROPIC_API_KEY` from environment using `std::env::var`.
   - Construct `AnthropicProvider`.
   - Construct a single `AgentConfig` with defaults.
   - Hard-code the system prompt to `"You are a helpful coding assistant."`.
2. Run `cargo check -p alfred-cli`.

---

### Milestone 2.2 — Minimal Tool: Bash

**Dependency:** 2.1 complete.

#### 2.2.1 Define `Tool` trait

1. In `alfred-tools/src/lib.rs`:

   ```rust
   #[async_trait::async_trait]
   pub trait Tool: Send + Sync {
       fn name(&self) -> &str;
       fn description(&self) -> &str;
       fn schema(&self) -> serde_json::Value;  // JSON Schema for input
       fn is_read_only(&self) -> bool { false }
       async fn execute(&self, input: serde_json::Value) -> ToolOutput;
   }
   pub struct ToolOutput {
       pub content: String,
       pub is_error: bool,
   }
   ```

2. Export `Tool`, `ToolOutput` from `alfred-tools/src/lib.rs`.

#### 2.2.2 Implement `BashTool`

1. In `alfred-tools/src/bash.rs`:
   - Struct `BashTool` with no fields (initially).
   - `name()` → `"Bash"`.
   - `description()` → `"Execute a shell command and return stdout/stderr."`.
   - `is_read_only()` → `false`.
   - `schema()` → JSON Schema with `command` (string, required), `timeout` (integer, optional), `description` (string, optional).
   - `execute()`:
     - Parse `command: String` from `input["command"]`.
     - Parse `timeout_ms: u64` from `input["timeout"].as_u64().unwrap_or(30_000)`.
     - Spawn `tokio::process::Command::new("sh").arg("-c").arg(&command)`.
     - Set timeout using `tokio::time::timeout(Duration::from_millis(timeout_ms), ...)`.
     - Capture stdout and stderr.
     - On exit code 0: return `ToolOutput { content: stdout, is_error: false }`.
     - On non-zero exit: return `ToolOutput { content: format!("Exit code {}\n{}", code, stderr), is_error: true }`.
     - On timeout: return `ToolOutput { content: "Command timed out", is_error: true }`.
2. Test with `echo hello` and `exit 1` and `sleep 100` (with short timeout).

#### 2.2.3 Implement `ToolRegistry`

1. In `alfred-tools/src/registry.rs`:
   - `ToolRegistry` holding `HashMap<String, Box<dyn Tool>>`.
   - `fn register(&mut self, tool: impl Tool + 'static)`.
   - `fn get(&self, name: &str) -> Option<&dyn Tool>`.
   - `fn schemas(&self) -> Vec<ToolSchema>` — iterates all tools, calls `.schema()`.
2. In `alfred-cli` PoC: construct registry, register `BashTool`.

---

### Milestone 2.3 — Minimal Agent Loop

**Dependency:** 2.1, 2.2 complete.

#### 2.3.1 Implement single-turn agent loop (PoC version)

1. In `alfred-agent/src/loop.rs` create `AgentLoop` struct:

   ```rust
   pub struct AgentLoop {
       provider: Arc<dyn ModelProvider>,
       tools: ToolRegistry,
       config: AgentConfig,
       history: Vec<Message>,
   }
   ```

2. Implement `AgentLoop::new(provider, tools, config)`.
3. Implement `async fn run(&mut self, user_input: &str) -> Result<String>`:
   - Push `Message { role: User, content: [Text(user_input)] }` to `self.history`.
   - Loop (up to `config.max_turns`):
     - Get tool schemas from registry.
     - Call `provider.complete(&self.history, &schemas, &self.config)`.
     - Push the assistant `ModelResponse` content into history as assistant message.
     - If `stop_reason == EndTurn` or no `ToolUse` blocks: break, return text content.
     - For each `ToolUse` block in response:
       - Look up tool by name in registry.
       - If not found: push `ToolResult { is_error: true, content: "Tool not found: ..." }`.
       - Else: call `tool.execute(input).await`.
       - Push resulting `ToolResult` as `user` message content block.
   - After loop: return final text or error.
4. Handle `max_turns` exceeded: return `AlfredError::Other("max_turns exceeded")`.

#### 2.3.2 Wire the PoC CLI

1. In `alfred-cli/src/main.rs`:
   - Parse `--print`/`-p` flag using a simple `std::env::args()` loop (not clap yet).
   - Instantiate `AnthropicProvider`, `ToolRegistry` with `BashTool`, `AgentConfig` with defaults.
   - Instantiate `AgentLoop`.
   - Call `loop.run(user_input).await`.
   - Print result to stdout.
2. Run end-to-end PoC: `ANTHROPIC_API_KEY=... cargo run -p alfred-cli -- "list files in current directory"`.
3. Verify the loop executes at least one tool call and returns text.

#### 2.3.3 PoC validation checklist

- [ ] Model is called with correct message format
- [ ] Tool schema is sent to model
- [ ] Model requests `Bash` tool
- [ ] Bash tool executes the command
- [ ] Result is sent back to model as `tool_result`
- [ ] Model returns final text response
- [ ] Total token usage is logged at debug level
- [ ] Non-zero exit codes produce `is_error: true`

---

## Phase 3 — Minimal Viable Agent

**Goal:** Complete tool set, proper CLI with clap, session storage, configuration loading.
**Exit criteria:** `alfred "audit this repository"` completes a multi-step task on a real repo with all six core tools available.

---

### Milestone 3.1 — Complete Tool Set

**Dependency:** Phase 2 complete.

#### 3.1.1 Implement `ReadTool`

1. In `alfred-tools/src/read.rs`:
   - `name()` → `"Read"`.
   - `is_read_only()` → `true`.
   - Parameters: `file_path` (string, required), `offset` (integer, optional, 1-based line number), `limit` (integer, optional, number of lines).
   - Implementation:
     - Read file with `tokio::fs::read_to_string(&file_path).await`.
     - If `offset` or `limit` specified: collect lines, slice `[offset-1 .. offset-1+limit]`.
     - Prefix each line with `"     N→"` format (right-aligned 6-char field, `→` separator) matching observed Claude format.
     - Return full prefixed content as `ToolOutput`.
   - Error cases: file not found → `is_error: true`, `"File does not exist. Note: your current working directory is {cwd}."`.
   - Error case: path is a directory → `is_error: true`, `"EISDIR: illegal operation on a directory, read '{path}'"`.
2. Tests:
   - Read a fixture file without offset/limit: verify line prefix format.
   - Read with offset=3, limit=5: verify correct slice.
   - Read non-existent file: verify is_error and message contains cwd.
   - Read directory path: verify EISDIR error.

#### 3.1.2 Implement `WriteTool`

1. In `alfred-tools/src/write.rs`:
   - `is_read_only()` → `false`.
   - Parameters: `file_path` (string, required), `content` (string, required).
   - Implementation:
     - Create parent directories with `tokio::fs::create_dir_all(parent)`.
     - Write with `tokio::fs::write(&file_path, &content).await`.
     - Return `"File written successfully."` on success.
   - Error: return `is_error: true` with IO error message.
2. Tests:
   - Write a new file: verify contents on disk.
   - Write to a nested path that doesn't exist: verify parent dirs created.
   - Write to a read-only path: verify is_error.

#### 3.1.3 Implement `EditTool`

1. In `alfred-tools/src/edit.rs`:
   - `is_read_only()` → `false`.
   - Parameters: `file_path`, `old_string`, `new_string`, `replace_all` (boolean, default false).
   - Implementation:
     - Read file content.
     - If `replace_all`: use `content.replace(&old_string, &new_string)`.
     - Else: replace first occurrence only. If `old_string` not found → `is_error: true`, `"<tool_use_error>String to replace not found in file.\nString: {old_string}</tool_use_error>"` (exact format from traces).
     - Write updated content back.
     - Return `"The file {path} has been updated successfully."` on success.
   - Generate a unified diff patch using `similar` crate; store as `toolUseResult` metadata in session recording.
2. Tests:
   - Edit a known string: verify file updated.
   - Edit with replace_all=true: verify all occurrences replaced.
   - Edit with string not found: verify is_error and exact error format.
   - Edit to empty string (deletion): verify works.

#### 3.1.4 Implement `GlobTool`

1. In `alfred-tools/src/glob.rs`:
   - `is_read_only()` → `true`.
   - Parameters: `pattern` (string, required), `path` (string, optional, defaults to cwd).
   - Implementation:
     - Use the `ignore` crate (`WalkBuilder`) or the `glob` crate.
     - Walk from `path`, match entries against `pattern`.
     - Sort results by modification time descending (matches Claude behavior).
     - Return newline-joined list of matching paths.
2. Tests:
   - Glob `**/*.rs` in workspace: verify expected files found.
   - Glob with specific directory path: verify scoped results.
   - Glob on non-existent path: verify is_error.

#### 3.1.5 Implement `GrepTool`

1. In `alfred-tools/src/grep.rs`:
   - `is_read_only()` → `true`.
   - Parameters (from Cursor bundle analysis and observed error messages):
     - `pattern` (string, required)
     - `path` (string, optional)
     - `output_mode` (string, optional): `"content"` | `"files_with_matches"` | `"count"` — default `"files_with_matches"`.
     - `context` (integer, optional): lines before and after match.
     - `-A` (integer): lines after.
     - `-B` (integer): lines before.
     - `-C` (integer): alias for context.
     - `-i` (boolean): case insensitive.
     - `-n` (boolean): include line numbers.
     - `glob` (string): filter files by glob.
     - `type` (string): file type filter (e.g. `"rs"`, `"js"`).
     - `multiline` (boolean): multiline mode.
     - `head_limit` (integer): limit output lines.
   - Implementation: use the `grep` crate (part of ripgrep family) for matching, or spawn `rg` subprocess if available.
   - Input validation: reject unknown parameters with `is_error: true`, `"InputValidationError: Grep failed due to the following issue:\nAn unexpected parameter \`{key}\` was provided"` (exact format from traces).
2. Tests:
   - Search for a known string in a fixture directory.
   - Test `output_mode: "files_with_matches"`.
   - Test `output_mode: "content"` with context=2.
   - Test case-insensitive match.
   - Test unknown parameter → InputValidationError.

#### 3.1.6 Register all tools in CLI

1. In `alfred-cli/src/main.rs`:
   - Build `ToolRegistry` with all six tools: `ReadTool`, `WriteTool`, `EditTool`, `GlobTool`, `GrepTool`, `BashTool`.
2. Integration test: run agent with `"show me the first 5 lines of Cargo.toml"` — verify Read tool used.

---

### Milestone 3.2 — Proper CLI with `clap`

**Dependency:** 3.1 complete.

#### 3.2.1 Replace PoC arg parsing with `clap`

1. In `alfred-cli/src/main.rs`, define `Cli` struct with `clap::Parser`:

   ```rust
   #[derive(Parser, Debug)]
   #[command(name = "alfred", version, about = "Local-first CLI coding agent")]
   struct Cli {
       /// Task to execute (positional)
       prompt: Option<String>,
       /// Non-interactive / print mode
       #[arg(short = 'p', long)]
       print: bool,
       /// Output format: text (default), json, stream-json
       #[arg(long, default_value = "text")]
       output_format: OutputFormat,
       /// Model override
       #[arg(long)]
       model: Option<String>,
       /// Resume a previous session
       #[arg(long)]
       resume: Option<String>,
       /// Maximum number of turns
       #[arg(long)]
       max_turns: Option<u32>,
       /// Maximum cost in USD
       #[arg(long)]
       max_budget_usd: Option<f64>,
       /// System prompt override
       #[arg(long)]
       system_prompt: Option<String>,
       /// Permission mode: default, accept-all, plan
       #[arg(long, default_value = "default")]
       permission_mode: PermissionMode,
       /// Allowed tools (comma-separated)
       #[arg(long)]
       allowed_tools: Option<String>,
       /// Disallowed tools (comma-separated)
       #[arg(long)]
       disallowed_tools: Option<String>,
   }
   ```

2. Add `OutputFormat` and `PermissionMode` as `clap::ValueEnum`.
3. Wire parsed flags into `AgentConfig`.
4. Read prompt from stdin if not provided as arg and not `--print` mode (interactive REPL stub).
5. Test: `alfred --help` prints usage; `alfred -p "hello"` runs.

#### 3.2.2 Add interactive REPL mode

1. If no prompt given and not `--print`: enter interactive mode.
2. Print `alfred> ` prompt, read line from stdin with `rustyline` crate (for history/editing).
3. On each input, run agent loop with preserved history.
4. Exit on Ctrl-C or `exit`/`quit` input.
5. Test: `alfred` without args enters REPL; multi-turn conversation works.

#### 3.2.3 Implement `list-sessions` subcommand

1. Add `#[command(subcommand)] command: Option<Commands>` to `Cli`.
2. Define `Commands` enum: `ListSessions`, `ShowSession { session_id: String }`.
3. Implement in `alfred-cli/src/commands/list_sessions.rs`:
   - Read session directory (see 3.3).
   - Print session_id, first message preview, timestamp, turn count.

---

### Milestone 3.3 — Session Storage

**Dependency:** 3.2 complete.

#### 3.3.1 Define session data structures

1. In `alfred-storage/src/session.rs`:
   - `SessionEnvelope`:

     ```rust
     pub struct SessionEnvelope {
         pub session_id: String,
         pub created_at: DateTime<Utc>,
         pub project_path: PathBuf,
         pub messages: Vec<SessionLine>,
     }
     ```

   - `SessionLine` — wraps a type-tagged line matching JSONL format:

     ```rust
     #[serde(tag = "type", rename_all = "snake_case")]
     pub enum SessionLine {
         User { message: Message },
         Assistant { message: Message },
         Result {
             subtype: String,
             duration_ms: u64,
             is_error: bool,
             num_turns: u32,
             total_cost_usd: f64,
             usage: Usage,
             result: Option<String>,
         },
         System { subtype: String },
         Progress { data: serde_json::Value },
     }
     ```

#### 3.3.2 Implement session write (JSONL)

1. Compute session directory: `{data_dir}/projects/{sanitized_cwd}/{session_id}.jsonl`.
   - Use `directories::ProjectDirs::from("", "", "alfred")` → `data_dir()`.
   - Sanitize cwd: replace `/` with `-`, strip leading `-`.
2. Implement `SessionWriter`:
   - Opens/creates JSONL file on first write.
   - `fn append(&mut self, line: &SessionLine)` — serializes to JSON, writes line + `\n`.
   - Uses `tokio::fs::OpenOptions` with append mode.
3. Write to session file after each user message, each assistant response, and after result.

#### 3.3.3 Implement session read (resume)

1. Implement `SessionReader::load(session_id: &str, project_path: &Path) -> Result<Vec<SessionLine>>`:
   - Locate JSONL file by session_id.
   - Read lines, deserialize each, collect `SessionLine`s.
   - Reconstruct `Vec<Message>` from user/assistant lines (skip result/system/progress).
2. Wire `--resume {session_id}` in CLI: load session, prepend history to new `AgentLoop`.
3. Test: run task → kill process → resume with session_id → verify history is restored and agent continues.

#### 3.3.4 Implement session directory discovery

1. `fn list_sessions(project_path: &Path) -> Result<Vec<SessionSummary>>`:
   - Enumerate `*.jsonl` files in project session dir.
   - For each: read first user line for preview, get file modified time.
   - Return sorted by modified time descending.
2. Wire into `alfred list-sessions` command.

---

### Milestone 3.4 — Configuration Loading

**Dependency:** 3.3 complete.

#### 3.4.1 Load configuration from file

1. Config file location: `~/.config/alfred/config.toml` (using `directories`).
2. Define `FileConfig` with serde deserialization:

   ```toml
   [provider]
   type = "anthropic"         # anthropic | openai | openrouter | local
   api_key_env = "ANTHROPIC_API_KEY"
   model = "claude-opus-4-6"
   base_url = "https://api.anthropic.com"

   [agent]
   max_turns = 50
   max_budget_usd = 5.0

   [tools]
   allowed = []               # empty = all allowed
   disallowed = ["Bash"]      # example
   ```

3. Implement `Config::load() -> Result<Config>`:
   - Check `$ALFRED_CONFIG_FILE`, then `~/.config/alfred/config.toml`.
   - Parse with `toml` crate.
   - Fall back to built-in defaults if file absent.
4. Merge: CLI flags override config file values.

#### 3.4.2 Load project instructions (`ALFRED.md` or `CLAUDE.md`)

1. In `alfred-context/src/project.rs`:
   - Walk from cwd upward to find `ALFRED.md` or `CLAUDE.md`.
   - Read content; treat as additional system prompt prefix.
   - Notify user when project instructions are loaded.
2. Prepend project instructions to system prompt in context builder.
3. Test: create `ALFRED.md` in fixture dir, verify it's included in system prompt.

#### 3.4.3 Inject tool usage guidance into system prompt

1. In `alfred-context/src/guidance.rs`:
   - Define `fn build_tool_guidance(registry: &ToolRegistry) -> String`.
   - Output a structured block appended to the system prompt:

     ```
     ## Tool Usage Guidelines

     Use these tools to accomplish tasks:

     - **Glob**: Explore repository structure. Example: Glob(pattern="**/*.rs", path=".")
     - **Read**: Inspect file contents. Use offset/limit for large files.
     - **Grep**: Search for patterns across files. Use output_mode="content" with context for
       code search.
     - **Bash**: Run shell commands. Prefer read-only commands (git log, cargo check) before
       writing.
     - **Edit**: Modify existing files using exact string replacement.
       Always Read the file first to get the exact old_string.
     - **Write**: Create new files or fully replace file contents.

     Exploration pattern: Glob → Read → Grep → Read → Edit/Write
     ```

2. Append tool guidance block to system prompt in `ContextBuilder::build()`.
3. Make guidance generation dynamic: regenerate if tool registry changes (e.g. MCP tools added).
4. Test: build context with known registry → verify guidance block appears in system message.

---

### Milestone 3.5 — Streaming Output

**Dependency:** 3.2, 3.4 complete.

#### 3.5.1 Implement Anthropic streaming

1. Add `stream: true` to Anthropic API request.
2. Use `reqwest` streaming response with `bytes_stream()`.
3. Parse SSE (`text/event-stream`) line-by-line:
   - `event: content_block_delta` with `delta.type: "text_delta"` → emit `TextDelta`.
   - `event: content_block_start` with `type: "tool_use"` → emit `ToolUseStart`.
   - `event: content_block_delta` with `delta.type: "input_json_delta"` → emit `ToolUseDelta`.
   - `event: content_block_stop` → emit `ToolUseEnd`.
   - `event: message_delta` with `usage` → emit `MessageDelta`.
4. Assemble partial tool JSON in `ToolUseDelta`: maintain a `HashMap<id, String>` buffer; append each `partial_json` fragment; on `ToolUseEnd` parse the accumulated string into `serde_json::Value`.
5. Return `impl Stream<Item = Result<StreamEvent>>` from provider.
6. Update `ModelProvider::complete_stream()` method signature.

#### 3.5.2 Wire streaming to CLI output

1. In `alfred-cli/src/output/streaming.rs`:
   - For `text` output format: print `TextDelta` chunks directly to stdout without newline.
   - For `stream-json` format: print each event as a JSON object on its own line.
   - For `json` format: buffer everything, print final JSON object.
2. Show tool call status lines during execution: `"⏳ Bash: ls -la"` → `"✓ Bash: ls -la"`.
3. Test: streaming output appears incrementally in terminal.

---

## Phase 4 — Feature Expansion

**Goal:** Multi-provider support, context compaction, permission system, subagents, bounded concurrency, task graph planner (optional), plan mode.
**Exit criteria:** All providers work; context compaction triggers on long sessions; permission prompts work; subagents can run in parallel; bounded concurrency prevents runaway tool calls.

---

### Milestone 4.1 — Multi-Provider Support

**Dependency:** Phase 3 complete.

#### 4.1.1 Implement OpenAI provider

1. In `alfred-providers/src/openai.rs`:
   - Struct `OpenAIProvider` with `client`, `api_key`, `base_url`, `model`.
   - Map `Message`/`ContentBlock` to OpenAI chat completions format:
     - `tool_use` → `assistant` message with `tool_calls` array.
     - `tool_result` → `tool` role message with `tool_call_id` and `content`.
   - Map `ToolSchema` → OpenAI function format: `{ "type": "function", "function": { "name": ..., "description": ..., "parameters": ... } }`.
   - POST to `/v1/chat/completions`.
   - Map response back to `ModelResponse`.
2. Test: mock HTTP server, verify request format and response parsing.

#### 4.1.2 Implement OpenRouter provider

1. In `alfred-providers/src/openrouter.rs`:
   - OpenRouter uses OpenAI-compatible API at `https://openrouter.ai/api/v1`.
   - Reuse `OpenAIProvider` implementation with configurable `base_url`.
   - Add `HTTP-Referer` and `X-Title` headers (OpenRouter requirements).
   - Support model name as passed (e.g. `anthropic/claude-opus-4-6`).
2. Test: verify headers are sent.

#### 4.1.3 Implement local model provider (Ollama)

1. In `alfred-providers/src/local.rs`:
   - Target: Ollama at `http://localhost:11434/v1` (OpenAI-compatible endpoint).
   - Tool call support: depends on model. Implement best-effort JSON parsing for function calling.
   - Fallback: if model doesn't support tool use natively, inject tool schemas into system prompt as JSON and parse tool calls from text output.
2. Test: integration test with a running Ollama instance (behind feature flag).

#### 4.1.4 Implement Alibaba Cloud (Qwen) provider

1. In `alfred-providers/src/alibaba.rs`:
   - Alibaba Cloud DashScope API: `https://dashscope.aliyuncs.com/compatible-mode/v1` (OpenAI-compatible).
   - Add `Authorization: Bearer {api_key}` header.
   - Support Qwen model names (e.g. `qwen-max`, `qwen-plus`).
2. Reuse `OpenAIProvider` with `base_url` override.

#### 4.1.5 Provider factory

1. In `alfred-providers/src/lib.rs`:
   - `fn build_provider(config: &ProviderConfig) -> Result<Arc<dyn ModelProvider>>`:
     - Match on `config.provider_type`.
     - Read API key from `config.api_key` or env var (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `DASHSCOPE_API_KEY`).
2. Wire into CLI config loading.
3. Test: provider factory returns correct type for each config variant.

---

### Milestone 4.2 — Context Engine

**Dependency:** Phase 3 complete.

#### 4.2.1 Implement token counter

1. In `alfred-context/src/compaction.rs`:
   - Use `tiktoken-rs` crate for cl100k/o200k token counting as a rough estimate.
   - Note: Claude uses a different tokenizer; counts are approximate (±10%). Use conservatively.
   - Implement `fn count_tokens(messages: &[Message]) -> usize`.
   - Serialize each message to its JSON API format, then count tokens.
2. Test: known text → known token count (within ±10%).

#### 4.2.2 Implement context builder

1. In `alfred-context/src/builder.rs`:

   ```rust
   pub struct ContextBuilder {
       max_tokens: usize,
       compaction_threshold: f64,  // e.g. 0.85 = compact at 85% of max
   }
   impl ContextBuilder {
       pub fn build(&self, config: &AgentConfig, history: &[Message]) -> Vec<Message>
   }
   ```

2. `build()` logic:
   - Include system prompt as first message (or as API `system` field).
   - Walk history from oldest to newest; count tokens as you add.
   - If adding the next message would exceed `max_tokens * compaction_threshold`: trigger compaction.
   - Otherwise: include all messages.

#### 4.2.3 Implement context compaction

1. When context exceeds threshold:
   - Take the oldest N messages (those that won't fit).
   - Call a secondary "summarize" request to the model: `"Summarize the following conversation history in 3-5 sentences, focusing on decisions made and context established: {oldest_messages}"`.
   - Replace oldest messages with a single `system` message: `"[Compacted history] {summary}"`.
   - Emit a `SessionLine::System { subtype: "compact_boundary" }` to the session file.
2. Test: create a fixture with a very long conversation; verify compaction triggers and summary is included.

---

### Milestone 4.3 — Permission System

**Dependency:** 4.1 complete.

#### 4.3.1 Implement permission checker

1. In `alfred-agent/src/permissions.rs`:
   - `PermissionChecker` struct with `mode: PermissionMode`, `allowed: Vec<ToolPattern>`, `disallowed: Vec<ToolPattern>`.
   - `async fn check(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision`.
   - `PermissionDecision` enum: `Allow`, `Deny`, `AskUser`.

#### 4.3.2 Implement permission modes

1. `PermissionMode::AcceptAll` → always return `Allow`.
2. `PermissionMode::PlanOnly` → allow only: `Read`, `Glob`, `Grep`, `Bash` read-only patterns.
   - For `Bash`: only allow if command matches read-only patterns (e.g. `ls`, `cat`, `find`, `grep`, `git diff`, `git log`). Otherwise `Deny`.
3. `PermissionMode::Default` → check allow/disallow lists; if state-changing tool not in allow list → `AskUser`.

#### 4.3.3 Implement interactive permission prompt

1. When `AskUser` returned in interactive mode:
   - Print: `"Allow tool {name} with input: {pretty_printed_input}? [y/N/a(lways)/d(isallow)]"`.
   - Read stdin response.
   - `y` → `Allow` this time; `a` → add to allow list for session; `d` → deny.
2. When in `--print` mode (non-interactive): treat `AskUser` as `Deny`, log warning.
3. Test: mock stdin with known responses; verify tool is blocked or allowed.

---

### Milestone 4.4 — Cost Tracking

**Dependency:** 4.1 complete.

#### 4.4.1 Track cumulative cost per session

1. In `alfred-agent/src/loop.rs`, add:
   - `cumulative_cost_usd: f64`
   - After each model call: estimate cost from usage tokens × per-token price.
   - Store a `ModelPricing` table (hardcoded to major models: Claude Opus, Sonnet, Haiku, GPT-4o, etc.).
2. After each turn, check: `if cumulative_cost_usd > config.max_budget_usd → return error`.
3. Print cost summary in result message: `"Total cost: $0.0123 USD"`.
4. Store `total_cost_usd` in `SessionLine::Result`.

---

### Milestone 4.5 — Plan Mode

**Dependency:** 4.3 complete.

#### 4.5.1 Implement plan mode flag

1. `--permission-mode plan` → only read-only tools allowed.
2. In `PermissionChecker`: `PlanOnly` restricts to `Read`, `Glob`, `Grep` (no Bash, no Write, no Edit).
3. Agent still loops; model can still request state-changing tools, but they'll be denied with `tool_result { is_error: true, content: "Tool not available in plan mode." }`.
4. Test: run in plan mode; verify Write/Edit/Bash are blocked.

---

### Milestone 4.6 — Parallel Tool Execution with Bounded Concurrency

**Dependency:** 4.1, 3.1 complete.

#### 4.6.1 Detect read-only tools

1. The `is_read_only()` method is already defined on `Tool` trait (Phase 2.2.1).
2. Ensure all tools return the correct value:
   - `Read`, `Glob`, `Grep` → `true`.
   - `Write`, `Edit`, `Bash` → `false`.

#### 4.6.2 Parallelize read-only tool calls

1. In `alfred-agent/src/executor.rs`:
   - When model returns multiple `ToolUse` blocks:
     - If ALL are read-only: execute them concurrently with `futures::future::join_all`.
     - If ANY is state-changing: execute all sequentially in order.
   - Collect results preserving original tool call ordering.
2. Test: model requests `[Read, Read, Read]` → verify all three execute concurrently.

#### 4.6.3 Add semaphore-based concurrency limit

1. In `alfred-agent/src/executor.rs`, add a `Semaphore` to `ToolExecutor`:

   ```rust
   pub struct ToolExecutor {
       tools: Arc<ToolRegistry>,
       semaphore: Arc<Semaphore>,  // default: 10 permits
   }
   ```

2. Before every tool execution (parallel or sequential): acquire a semaphore permit.
3. Release permit after tool completes (permit dropped on scope exit).
4. Configure max concurrency in `config.toml`: `[tools] max_concurrent = 10`.
5. Test: spawn 20 simultaneous Read calls → verify at most 10 execute concurrently (measure with a slow mock tool).
6. Verify: no deadlock when all 10 permits are held and a sequential (state-changing) tool is queued.

---

### Milestone 4.7 — Subagent Architecture

**Dependency:** 4.6 complete, 6.3 (Arc wrapping) can be done early.

**Rationale:** Claude Code uses subagents heavily (traces show `Agent` tool with 39 calls, second-highest after Read/Bash/Edit). Subagents allow isolated parallel reasoning tasks (e.g., parallel file analysis, running tests while editing).

#### 4.7.1 Define SubAgent types

1. In `alfred-agent/src/subagent.rs`:

   ```rust
   pub struct SubAgentConfig {
       pub prompt: String,
       pub description: Option<String>,    // human-readable label
       pub subagent_type: SubAgentType,
       pub max_turns: u32,
   }

   pub enum SubAgentType {
       GeneralPurpose,
       ReadOnly,      // plan mode — only read tools
       Custom(String),
   }

   pub struct SubAgentHandle {
       pub id: String,
       pub task: tokio::task::JoinHandle<Result<String>>,
   }
   ```

2. Derive `Serialize`, `Deserialize`, `Debug`, `Clone` on config types.

#### 4.7.2 Implement SubAgentManager

1. In `alfred-agent/src/subagent.rs`:

   ```rust
   pub struct SubAgentManager {
       provider: Arc<dyn ModelProvider>,
       tools: Arc<ToolRegistry>,
       active: HashMap<String, SubAgentHandle>,
   }

   impl SubAgentManager {
       pub async fn spawn(&mut self, config: SubAgentConfig) -> Result<String>;
       pub async fn wait(&mut self, id: &str) -> Result<String>;
       pub async fn wait_all(&mut self) -> Vec<Result<String>>;
   }
   ```

2. `spawn()`:
   - Create a new `AgentLoop` with shared `Arc<dyn ModelProvider>` and `Arc<ToolRegistry>`.
   - Apply `subagent_type` restrictions (ReadOnly → PlanOnly permission mode).
   - Spawn with `tokio::task::spawn(async move { agent.run(&config.prompt).await })`.
   - Store handle in `active`.
   - Return subagent id.
3. `wait()`: await the join handle for a given id, return result.
4. `wait_all()`: `futures::future::join_all` over all active handles.

#### 4.7.3 Implement `AgentTool` (spawns subagents from within the loop)

1. Add `AgentTool` to `alfred-tools/src/`:
   - Parameters: `prompt` (string, required), `description` (string, optional), `subagent_type` (string, optional).
   - `is_read_only()` → `false` (subagents may write).
   - `execute()`: calls `SubAgentManager::spawn()` and immediately waits (synchronous from the outer agent's perspective).
   - Returns subagent's final text output as `ToolOutput`.
2. Register `AgentTool` in the default tool registry.
3. Test: outer agent spawns a subagent via `AgentTool`; subagent reads a file; outer agent receives file summary.

#### 4.7.4 Subagent session recording

1. Subagents write their own JSONL session file under `{session_id}/subagents/{subagent_id}.jsonl`.
2. Parent session file records `AgentTool` use and result as normal tool_use/tool_result blocks.
3. Test: verify subagent session file exists after subagent completes.

---

### Milestone 4.8 — Task Graph / Planner (Advanced, Optional)

**Dependency:** 4.7 complete.

**Design note:** The reactive agent loop (Phases 2–4.7) is the primary execution model and handles most tasks well. The planner is an *optional overlay* for tasks where the user or agent benefits from a structured upfront plan. It does not replace the reactive loop — it pre-structures it. The planner call is itself non-deterministic (it is a model call), so the "determinism" is only in the execution layer once a valid graph is produced.

**Trade-offs:**
- Adds one extra model call per task (latency + cost).
- Fails gracefully: if the planner produces an invalid graph, fall back to the reactive loop.
- Best suited for large, well-defined tasks (e.g., "refactor all usages of X across 50 files").
- Not recommended for open-ended exploration where each step depends on findings.

#### 4.8.1 Define task graph types

1. In `alfred-planner/src/task_graph.rs`:

   ```rust
   pub struct Task {
       pub id: String,
       pub tool: String,
       pub input: serde_json::Value,
       pub depends_on: Vec<String>,
       pub description: Option<String>,
   }

   pub struct TaskGraph {
       pub tasks: Vec<Task>,
   }

   impl TaskGraph {
       /// Topological sort; returns tasks in execution order with parallelism groups.
       pub fn execution_order(&self) -> Result<Vec<Vec<Task>>>;
       /// Validate: no cycles, all depends_on ids exist, all tool names valid.
       pub fn validate(&self, registry: &ToolRegistry) -> Result<()>;
   }
   ```

2. Tests:
   - Valid linear graph: verify topological order.
   - Valid parallel graph (two tasks with no dependency): verify both in same group.
   - Cyclic graph: verify `validate()` returns error.
   - Unknown tool name: verify `validate()` returns error.

#### 4.8.2 Define Planner trait

1. In `alfred-planner/src/planner.rs`:

   ```rust
   #[async_trait::async_trait]
   pub trait Planner: Send + Sync {
       async fn plan(
           &self,
           prompt: &str,
           context: &[Message],
           registry: &ToolRegistry,
       ) -> Result<TaskGraph>;
   }
   ```

2. Implement `ModelPlanner`:
   - Builds a system prompt instructing the model to output a JSON `TaskGraph`.
   - Calls `provider.complete()` with a single user message containing the task prompt.
   - Parses response as JSON; deserializes into `TaskGraph`.
   - Validates the graph; on failure returns `AlfredError::Planner`.
   - System prompt example:

     ```
     You are a task planner. Given a user request, output a JSON task graph.
     Each task has: id, tool, input (matching the tool's schema), depends_on (list of ids).
     Available tools: {tool_list}
     Output only valid JSON. No markdown, no explanation.
     ```

3. Test: mock provider returning a known JSON graph → verify correct `TaskGraph` parsed.

#### 4.8.3 Implement TaskExecutor

1. In `alfred-planner/src/executor.rs`:

   ```rust
   pub struct TaskExecutor {
       tools: Arc<ToolRegistry>,
       semaphore: Arc<Semaphore>,
   }

   impl TaskExecutor {
       pub async fn execute(
           &self,
           graph: TaskGraph,
           results: &mut HashMap<String, ToolOutput>,
       ) -> Result<ExecutionSummary>;
   }
   ```

2. `execute()` logic:
   - Call `graph.execution_order()` to get groups.
   - For each group (tasks with satisfied dependencies): execute all tasks in the group concurrently (respecting semaphore).
   - Store results in `results` map keyed by task id.
   - If a task fails (`is_error: true`): record failure, continue executing independent tasks; collect all errors in `ExecutionSummary`.

#### 4.8.4 Implement plan-refine loop

1. After `TaskExecutor::execute()` completes, feed `ExecutionSummary` back to the model:
   - Message: `"I executed the following plan. Here are the results. Do you need to revise the plan or are you done? Reply with 'DONE: {final answer}' or an updated task graph JSON."`.
   - If model replies `DONE: ...`: extract final answer and return.
   - If model replies with a new task graph: validate, execute, repeat (up to `max_refinement_iterations`, default 3).
2. This is the "plan-refine loop" — note it is structurally equivalent to the reactive loop but with graph structure per iteration.
3. Test: mock a failing first execution; verify model refines the plan and second execution succeeds.

#### 4.8.5 Wire planner into CLI

1. Add `--planner` flag to `alfred-cli`.
2. If `--planner` flag present or `config.use_planner = true`:
   - Run `ModelPlanner::plan()` first.
   - Display the plan to user (see Milestone 8.6).
   - Ask confirmation: `"Execute this plan? [Y/n]"`.
   - Run `TaskExecutor::execute()`.
   - Feed results to reflection step.
3. On planner failure: log warning, fall back to reactive loop automatically.
4. Test: planner mode with mock provider → verify plan displayed, confirmed, executed.

---

## Phase 5 — Reliability Improvements

**Goal:** Robust error handling, retry logic, session recovery, graceful shutdown, persistent memory.

---

### Milestone 5.1 — Robust Error Handling

**Dependency:** Phase 4 complete.

#### 5.1.1 Handle provider rate limits and transient errors

1. In `alfred-providers/src/anthropic.rs`:
   - On HTTP 429 (rate limit): parse `Retry-After` header.
   - Implement exponential backoff: 1s, 2s, 4s, 8s, max 3 retries.
   - Use `tokio::time::sleep` between retries.
   - Log each retry attempt at `warn!` level.
2. On HTTP 5xx: retry up to 3 times.
3. On HTTP 4xx (except 429): do not retry, return `AlfredError::Provider`.
4. Test: mock server returning 429 → verify retry behavior.

#### 5.1.2 Handle tool execution failures gracefully

1. From trace evidence: failed tool results (`is_error: true`) are passed to the model as normal `tool_result` messages; the model decides how to recover.
2. Ensure `ToolOutput` with `is_error: true` is always returned (never panic).
3. Add catch-all in `execute()`: `tokio::task::spawn(async { tool.execute(input).await }).await.unwrap_or_else(|e| ToolOutput { content: format!("Internal error: {e}"), is_error: true })`.
4. Test: tool that panics → verify panic is caught and returned as is_error.

#### 5.1.3 Validate tool input schemas before execution

1. In `alfred-tools/src/registry.rs`:
   - Before calling `tool.execute(input)`, validate `input` against the tool's `schema()` using `jsonschema` crate.
   - If validation fails: return `ToolOutput { is_error: true, content: "InputValidationError: ..." }` without executing.
2. Test: call Grep with unknown parameter → verify InputValidationError (matching observed format).

#### 5.1.4 Handle malformed JSON in model tool calls

1. When parsing `ToolUse.input`: use `serde_json::from_str` with fallback.
2. If JSON is malformed (streaming partial): wait for `ToolUseEnd`, retry parse.
3. If still malformed after assembly: return `is_error: true`, `"Malformed tool input JSON"`.
4. Test: inject a partial JSON fragment; verify it's assembled correctly.

---

### Milestone 5.2 — Session Recovery

**Dependency:** 3.3, 5.1 complete.

#### 5.2.1 Checkpoint writes

1. Write each session line immediately after it's generated (not buffered).
2. On crash/kill: session file contains all turns up to crash point.
3. `--resume` reconstructs history from file and continues.
4. Test: kill the process mid-session; resume with `--resume`; verify continuation.

#### 5.2.2 Implement session fork

1. Add `SessionStorage::fork(session_id: &str) -> Result<String>`:
   - Copies session JSONL to a new session_id file.
   - Returns new session_id.
2. Expose via `alfred fork {session_id}` subcommand.
3. Test: fork a session; run both; verify they diverge independently.

---

### Milestone 5.3 — Graceful Shutdown

**Dependency:** Phase 4 complete.

#### 5.3.1 Handle Ctrl-C

1. Install `tokio::signal::ctrl_c()` handler in `alfred-cli/src/main.rs`.
2. On Ctrl-C:
   - If tool is currently executing: let it finish or timeout after 2s.
   - Print `"\nInterrupted. Session saved to {session_id}."`.
   - Flush session file.
   - Exit cleanly.
3. Test: interrupt during Bash execution → verify session is flushed.

---

### Milestone 5.4 — Integration Test Suite

**Dependency:** 5.1–5.3 complete.

#### 5.4.1 Build integration test harness

1. In `tests/integration/`:
   - Create a mock model provider that returns pre-scripted responses.
   - Fixture repository: `tests/fixtures/sample-project/` with known files.
2. Test scenarios:
   - Single-turn: model returns text → verify output.
   - Tool use: model requests Read → verify file returned → model returns text.
   - Edit: model requests Edit → verify file changed on disk.
   - Error recovery: model requests bad Edit → verify is_error → model requests new Edit.
   - Max turns: model keeps requesting tools → verify loop stops at max_turns.
   - Cost limit: inject high usage → verify cost limit stops loop.
   - Semaphore: 20 concurrent Read calls with semaphore=5 → verify bounded.
   - Subagent: outer agent spawns subagent → subagent reads file → outer receives result.

---

### Milestone 5.5 — Memory System

**Dependency:** 5.1 complete, 3.3 complete.

**Rationale:** Session history covers a single run. Memory persists knowledge across sessions: repo structure, user preferences, past findings, recurring patterns.

#### 5.5.1 Define memory types

1. In `alfred-memory/src/lib.rs`:

   ```rust
   pub enum MemoryType {
       ShortTerm,  // in-session working notes
       LongTerm,   // cross-session persistent
   }

   pub struct MemoryEntry {
       pub id: String,
       pub content: String,
       pub tags: Vec<String>,
       pub created_at: DateTime<Utc>,
       pub session_id: Option<String>,
   }
   ```

2. Define `MemoryStore` trait:

   ```rust
   #[async_trait::async_trait]
   pub trait MemoryStore: Send + Sync {
       async fn save(&self, entry: &MemoryEntry) -> Result<()>;
       async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;
       async fn list_recent(&self, limit: usize) -> Result<Vec<MemoryEntry>>;
       async fn delete(&self, id: &str) -> Result<()>;
   }
   ```

#### 5.5.2 Implement short-term memory (in-session)

1. In `alfred-memory/src/short_term.rs`:
   - `InMemoryStore`: `HashMap<String, MemoryEntry>` guarded by `tokio::sync::RwLock`.
   - Simple `search()`: substring match on `content`.
   - Used for: noting intermediate findings within a session that should be referenced later.
2. Test: save 5 entries; search by keyword; verify correct entries returned.

#### 5.5.3 Implement long-term memory (SQLite)

1. In `alfred-memory/src/long_term.rs`:
   - Use `rusqlite` (sync, via `tokio::task::spawn_blocking`) or `sqlx` with SQLite async feature.
   - Schema:

     ```sql
     CREATE TABLE memories (
         id TEXT PRIMARY KEY,
         content TEXT NOT NULL,
         tags TEXT,          -- JSON array
         created_at TEXT,
         session_id TEXT
     );
     CREATE VIRTUAL TABLE memories_fts USING fts5(content, tags);
     ```

   - `save()`: insert into both tables.
   - `search()`: use FTS5 for full-text search on content + tags.
   - `list_recent()`: `ORDER BY created_at DESC LIMIT ?`.
2. DB path: `{data_dir}/memory.db`.
3. Test: save 100 entries; full-text search; verify correct results; verify persistence across process restart.

#### 5.5.4 Wire memory into agent loop

1. Add `memory: Arc<dyn MemoryStore>` to `AgentLoop`.
2. At session start: load recent memories (top 10 by recency), inject as system prompt section:

   ```
   ## Relevant Memory
   - {entry.content}
   ```

3. Implement `MemoryTool` in `alfred-tools/`:
   - Parameters: `action` (`save` | `search` | `list`), `content` (for save), `query` (for search).
   - Register in tool registry.
   - Model can explicitly save insights: `MemoryTool(action="save", content="This repo uses X pattern for Y")`.
4. Test: save a memory in session 1; start session 2; verify memory appears in context.

---

## Phase 6 — Performance Optimization

**Goal:** Fast startup, low memory, efficient context building, file read caching.

---

### Milestone 6.1 — Startup Performance

**Dependency:** Phase 5 complete.

#### 6.1.1 Profile startup time

1. Add `tracing::span!` around startup steps: config load, provider init, tool registry build.
2. Use `cargo flamegraph` or `samply` to profile a simple `alfred -p "hello"` run.
3. Target: `< 200ms` to first API request.

#### 6.1.2 Lazy provider initialization

1. Wrap provider in `tokio::sync::OnceCell`; initialize on first use.
2. Do not perform any I/O in `ProviderFactory::build()` — only construct structs.
3. Move connection test (if any) to an optional `alfred doctor` command.

#### 6.1.3 Optimize JSON serialization

1. Replace `.to_string()` + `from_str()` round-trips with direct serialization where possible.
2. Use `serde_json::to_writer` instead of `to_string` when writing to files.
3. Profile: `cargo bench` for serialization hot paths.

---

### Milestone 6.2 — Context Efficiency

**Dependency:** 4.2 complete.

#### 6.2.1 Implement smart file content truncation

1. For `Read` tool results exceeding 50k characters: truncate with a note.
   - Exact truncation: show first N lines, add `"\n[... truncated {remaining} lines ...]"`.
   - Allow model to use `offset`/`limit` params to read specific sections.
2. Test: read a large file; verify truncation is applied and noted.

#### 6.2.2 Deduplicate repeated file reads in context

1. Track which file paths have been Read in this session.
2. On a second Read of the same path with same content: return `"[File content unchanged from previous read]"` if unchanged.
3. This reduces context bloat on repeated reads.

---

### Milestone 6.3 — Concurrent Provider Requests

**Dependency:** 6.1 complete.

#### 6.3.1 Ensure AgentLoop is cheaply constructable for subagents

1. `AgentLoop` should hold only `Arc` references so construction is O(1).
2. Wrap shared state (tool registry, provider, memory store) in `Arc`.
3. Test: two concurrent `AgentLoop::run()` calls on the same provider → no data races.

---

### Milestone 6.4 — File Read LRU Cache

**Dependency:** 6.1 complete.

**Rationale:** Agents frequently re-read the same files within a session (trace pairs: Read→Read→Read = 537 occurrences). Caching avoids redundant disk I/O and can serve unchanged content instantly.

#### 6.4.1 Implement in-memory LRU cache for file reads

1. In `alfred-tools/src/cache.rs`:
   - Use `lru` crate: `LruCache<PathBuf, (String, SystemTime)>` — path → (content, mtime).
   - Cache capacity: configurable, default 50 entries.
   - `get(path)` → check mtime against cached mtime; if file modified since cache: invalidate.
   - `insert(path, content, mtime)` → store in LRU.
2. Wrap in `Arc<Mutex<FileCache>>` for shared use.

#### 6.4.2 Wire cache into ReadTool

1. `ReadTool` gets a `cache: Arc<Mutex<FileCache>>` reference (passed at construction).
2. Before reading from disk: check cache.
   - Cache hit and file unmodified: return cached content (mark in response: no additional note needed, this is transparent).
   - Cache miss or file modified: read from disk, insert into cache.
3. Test: read the same file twice; verify second read is served from cache (mock disk to confirm no second syscall).
4. Test: modify file between reads; verify cache is invalidated and fresh content returned.

#### 6.4.3 Disable cache for large files

1. Files over 1 MB: do not cache (to avoid excessive memory use).
2. Make threshold configurable: `[tools] cache_max_file_bytes = 1048576`.

---

## Phase 7 — Security and Sandboxing

**Goal:** Safe Bash execution, path traversal prevention, permission auditing.

---

### Milestone 7.1 — Bash Sandboxing

**Dependency:** Phase 5 complete.

#### 7.1.1 Restrict Bash environment

1. Strip sensitive env vars before spawning subprocesses: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `AWS_*`, `GITHUB_TOKEN`, etc.
2. Allow list of env vars that pass through (configurable).
3. Test: `echo $ANTHROPIC_API_KEY` in Bash → verify empty string returned.

#### 7.1.2 Platform sandboxing (macOS)

1. On macOS: wrap Bash commands in `sandbox-exec` with a restrictive profile:
   - Allow: read from cwd subtree, write to cwd subtree, exec `/bin/sh`, `/usr/bin/*`, `/usr/local/bin/*`.
   - Deny: network access, write outside cwd, access to `~/.config`, `~/.ssh`, `~/.gnupg`.
2. Gate behind `--sandbox` CLI flag (opt-in initially).
3. Test: command attempting network access inside sandbox → verify denied.

#### 7.1.3 Platform sandboxing (Linux)

1. On Linux: use `seccomp-bpf` via the `seccomp` crate to restrict syscalls in child process.
2. Alternatively: spawn inside a minimal Docker container if `docker` is available and `--docker-sandbox` flag is set.
3. Test: verify sandboxed process cannot write outside cwd.

#### 7.1.4 Path traversal prevention for file tools

1. In `ReadTool`, `WriteTool`, `EditTool`:
   - Canonicalize path.
   - Optionally enforce a "root" directory (cwd by default).
   - If path escapes root: return `is_error: true`, `"Access denied: path outside working directory."`.
2. Allow user to configure trusted paths in `config.toml`.
3. Test: `Read { file_path: "/etc/passwd" }` → verify denied when cwd-restriction active.

---

### Milestone 7.2 — Secret Detection

**Dependency:** 7.1 complete.

#### 7.2.1 Scan Write/Edit content for secrets

1. Before writing content to disk: scan for patterns matching API keys, tokens, private keys:
   - Regex patterns: `sk-[A-Za-z0-9]{48}`, `ghp_[A-Za-z0-9]{36}`, `-----BEGIN (RSA |EC )?PRIVATE KEY-----`, etc.
2. If match found: warn user; require confirmation before writing.
3. Gate: only active if `[security] scan_writes = true` in config.
4. Test: write a fixture containing a fake API key → verify warning.

---

### Milestone 7.3 — Audit Logging

**Dependency:** 7.1 complete.

#### 7.3.1 Implement tool audit log

1. Every tool call (name, input summary, result summary, is_error, duration_ms) → append to `{data_dir}/audit.jsonl`.
2. This is separate from the session file; the audit log is append-only and never compacted.
3. Expose via `alfred audit` subcommand: tail or search the audit log.

---

## Phase 8 — Developer Experience

**Goal:** Polished CLI, hooks system, MCP compatibility, live plan display, repo indexing, documentation.

---

### Milestone 8.1 — Hooks System

**Dependency:** Phase 5 complete.

#### 8.1.1 Define hook interface

1. Hooks fire at: `PreToolUse`, `PostToolUse`, `SessionStart`, `SessionEnd`.
2. In `config.toml`:

   ```toml
   [[hooks]]
   event = "PostToolUse"
   tool = "Edit"
   command = "git diff --stat"
   ```

3. Hook receives env vars: `ALFRED_TOOL_NAME`, `ALFRED_TOOL_INPUT`, `ALFRED_TOOL_RESULT`, `ALFRED_SESSION_ID`.

#### 8.1.2 Implement hook executor

1. In `alfred-agent/src/events.rs`:
   - After each tool call, check for matching hooks.
   - For matching hooks: spawn subprocess `sh -c {hook_command}` with env vars.
   - Capture output; log at `debug!` level.
   - Hook failures do not stop the agent (logged only).
2. Test: configure a hook → run agent → verify hook fires.

#### 8.1.3 Emit progress events (PostToolUse)

1. Match observed JSONL: emit `SessionLine::Progress { data: { type: "hook_progress", hookEvent: "PostToolUse", hookName: "PostToolUse:{tool_name}", toolUseID, parentToolUseID } }` after each tool.
2. This makes session files compatible with tools that read Claude-format JSONL.

---

### Milestone 8.2 — JSON and Stream-JSON Output

**Dependency:** 3.5 complete.

#### 8.2.1 Implement `--output-format json`

1. Buffer all events; on completion, output:

   ```json
   {
     "type": "result",
     "result": "...",
     "session_id": "...",
     "num_turns": 5,
     "duration_ms": 12345,
     "total_cost_usd": 0.0045,
     "is_error": false,
     "usage": { "input_tokens": 1234, "output_tokens": 567 }
   }
   ```

2. On error: `"is_error": true`, `"error": "..."`.

#### 8.2.2 Implement `--output-format stream-json`

1. Each event emitted as a newline-delimited JSON object:
   - `{ "type": "text", "text": "..." }` for each text chunk.
   - `{ "type": "tool_use", "name": "Bash", "input": {...} }` when tool starts.
   - `{ "type": "tool_result", "name": "Bash", "is_error": false, "content": "..." }` when tool finishes.
   - `{ "type": "result", ... }` at end.
2. Test: pipe `alfred --output-format stream-json` to `jq` → verify each line is valid JSON.

---

### Milestone 8.3 — MCP (Model Context Protocol) Support

**Dependency:** Phase 5 complete.

#### 8.3.1 Implement MCP client

1. MCP tools expose a JSON-RPC interface over stdio or HTTP.
2. In `alfred-tools/src/mcp.rs`:
   - `McpClient` struct: connects to an MCP server process (stdio) or HTTP endpoint.
   - `fn list_tools(&self) -> Vec<ToolSchema>` — calls MCP `tools/list`.
   - `fn call_tool(&self, name: &str, input: Value) -> ToolOutput` — calls MCP `tools/call`.
3. In `alfred-tools/src/registry.rs`: allow dynamic registration of MCP tools at startup.

#### 8.3.2 Wire MCP config

1. In `config.toml`:

   ```toml
   [[mcp_servers]]
   name = "filesystem"
   command = "npx"
   args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
   ```

2. At startup: spawn configured MCP servers, register their tools.
3. Implement `alfred-cli/src/commands/run.rs` MCP startup routine.
4. Tool guidance (Milestone 3.4.3) regenerates to include MCP tools after registration.

---

### Milestone 8.4 — `alfred doctor` Command

**Dependency:** 4.5 complete.

#### 8.4.1 Implement health check command

1. `alfred doctor` checks:
   - Rust/Cargo version (from `rustc --version`).
   - API key presence for configured provider.
   - Provider connectivity (simple API call).
   - MCP servers (if configured): spawn and ping.
   - Session storage directory permissions.
   - Memory DB accessible.
   - Optional: repo index present and up to date.
2. Print `✓` / `✗` for each check.

---

### Milestone 8.5 — Shell Completion and Man Pages

**Dependency:** 3.2 complete.

#### 8.5.1 Generate shell completions

1. Add `build.rs` to `alfred-cli` using `clap_complete`:

   ```rust
   generate(Bash, &mut app, "alfred", &mut std::io::stdout());
   ```

2. Generate completions for: bash, zsh, fish.
3. Install instructions in README.

#### 8.5.2 Generate man page

1. Use `clap_mangen` in `build.rs` to generate `alfred.1`.
2. Add `alfred man` subcommand that prints the man page.

---

### Milestone 8.6 — Live Plan / Progress Visualization

**Dependency:** 4.8 complete (planner), 3.5 complete (streaming).

**Rationale:** Showing the user what the agent is doing at each step is a major usability win. Both Claude CLI and Cursor display tool progress inline.

#### 8.6.1 Implement live tool progress display

1. In `alfred-cli/src/output/streaming.rs`, enhance existing tool status display:
   - When tool starts: print `"⏳ {tool_name}: {input_summary}"` on a new line.
   - When tool ends (success): overwrite/replace with `"✓ {tool_name}: {input_summary} ({duration_ms}ms)"`.
   - When tool ends (error): `"✗ {tool_name}: {input_summary} — {error_preview}"`.
   - Use ANSI escape codes to update in-place when terminal supports it (check `TERM` and `NO_COLOR`).
2. Test: run agent with a multi-step task; verify each tool prints status in sequence.

#### 8.6.2 Implement plan display for planner mode

1. In `alfred-cli/src/output/plan_display.rs`:
   - `fn display_plan(graph: &TaskGraph)` — renders the task graph as a numbered list:

     ```
     PLAN
     ────────────────────────────────────
      1. [Glob]  Explore repository structure
      2. [Grep]  Find unsafe code  (depends on: 1)
      3. [Read]  Read affected files  (depends on: 2)
      4. [Edit]  Apply fixes  (depends on: 3)
      5. [Bash]  Run tests  (depends on: 4)
     ```

   - Use Unicode box-drawing chars; degrade gracefully on non-UTF-8 terminals.
2. Implement `fn update_plan_status(id: &str, status: TaskStatus)` for live updates:
   - Pending: ` ○`
   - In progress: `⏳`
   - Done: ` ✓`
   - Failed: ` ✗`
3. Wire into planner execution path (Milestone 4.8.5).
4. Test: mock plan with 5 tasks; verify display updates correctly as each completes.

#### 8.6.3 Add `--verbose` flag for detailed tool I/O

1. `alfred --verbose` prints full tool inputs and outputs inline.
2. Without `--verbose`: show only name and a short input summary.
3. Test: verify verbose mode shows full `old_string`/`new_string` for Edit calls.

---

### Milestone 8.7 — Repository Indexing (Optional Advanced Feature)

**Dependency:** Phase 5 complete, `alfred-index` crate created in Phase 1.

**Rationale:** For large repositories (>10k files), Glob/Grep over raw files is slow. A pre-built index enables sub-millisecond symbol lookup and semantic search without touching every file.

**Note:** This is gated behind `alfred index build` and `config.use_index = true`. The agent works without it; the index is purely a performance and capability enhancement.

#### 8.7.1 Implement file index

1. In `alfred-index/src/file_index.rs`:
   - Walk repository using `ignore::WalkBuilder` (respects .gitignore).
   - Store: path, size, mtime, language (from extension).
   - Persist as JSON file: `{data_dir}/index/{sanitized_path}/files.json`.
2. Implement `fn update_incremental()`: only reindex files changed since last build (compare mtime).
3. Test: index a fixture repo; verify all files present; add a file; run incremental update; verify new file appears.

#### 8.7.2 Implement symbol extraction with tree-sitter

1. In `alfred-index/src/symbol_index.rs`:
   - Add `tree-sitter` + language grammars as optional dependencies:
     - `tree-sitter-rust`, `tree-sitter-javascript`, `tree-sitter-python`, `tree-sitter-typescript`.
   - For each supported file: parse with tree-sitter, extract:
     - Function definitions (name, file, line range)
     - Struct/class definitions
     - Trait/interface definitions
     - Constants and type aliases
   - Store in SQLite: `symbols(name, kind, file_path, start_line, end_line, language)`.
2. Expose `fn search_symbol(query: &str) -> Vec<SymbolEntry>`.
3. Test: index a known Rust file; search for a function name; verify correct location returned.

#### 8.7.3 Implement full-text search with tantivy

1. In `alfred-index/src/search.rs`:
   - Use `tantivy` crate for full-text indexing of file contents.
   - Fields: `path`, `content`, `language`.
   - `fn index_file(path: &Path, content: &str, language: &str)`.
   - `fn search(query: &str, limit: usize) -> Vec<SearchResult>`.
2. Store tantivy index at `{data_dir}/index/{sanitized_path}/tantivy/`.
3. Test: index 100 fixture files; search for a unique token; verify correct file returned.

#### 8.7.4 Add `SymbolSearchTool` and `IndexSearchTool`

1. `SymbolSearchTool`:
   - Parameters: `query` (string, required), `kind` (string, optional: function/struct/trait), `language` (string, optional).
   - `is_read_only()` → `true`.
   - Calls `symbol_index.search_symbol()`.
2. `IndexSearchTool`:
   - Parameters: `query` (string, required), `limit` (integer, optional, default 10).
   - `is_read_only()` → `true`.
   - Calls `search.search()`.
3. Register both in tool registry (only when index is enabled and built).
4. Add to tool guidance prompt when index tools are available.

#### 8.7.5 Wire index CLI commands

1. `alfred index build` — build index for current project.
2. `alfred index update` — incremental update.
3. `alfred index status` — show index freshness, file count, symbol count.
4. On `alfred` startup with `use_index = true`: check if index exists; warn if stale (mtime > 10 min).

---

## Phase 9 — Production Readiness

**Goal:** Stable release, full test coverage, documentation, packaging, telemetry.

---

### Milestone 9.1 — Full Test Coverage

**Dependency:** All previous phases complete.

#### 9.1.1 Unit test coverage targets

1. `alfred-core`: ≥ 95% coverage (pure data types, easiest).
2. `alfred-tools`: ≥ 90% coverage (test each tool with fixture files).
3. `alfred-providers`: ≥ 85% coverage (use `wiremock` for HTTP mocking).
4. `alfred-agent`: ≥ 80% coverage (use mock provider and mock tools).
5. `alfred-context`: ≥ 85% coverage (token counting, compaction, guidance logic).
6. `alfred-storage`: ≥ 85% coverage (JSONL read/write with tempfiles).
7. `alfred-memory`: ≥ 85% coverage (both short-term and SQLite long-term).
8. `alfred-planner`: ≥ 80% coverage (task graph validation, topological sort, executor).
9. `alfred-index`: ≥ 75% coverage (file walk, symbol extraction, tantivy integration).

#### 9.1.2 End-to-end integration tests

1. Test against real API (CI, needs secret): `cargo test --features integration`.
2. Scenarios:
   - `"Create a hello world Rust file"` → verify `main.rs` created with valid Rust.
   - `"Fix the syntax error in tests/fixtures/broken.rs"` → verify file fixed.
   - `"What files are in this project?"` → verify Glob/Read used, correct answer returned.
   - Session resume: run task halfway, kill, resume → verify completion.
   - Memory: save memory in session 1; verify it appears in session 2 context.
   - Planner: run with `--planner` flag; verify plan displayed and executed.

#### 9.1.3 Property-based tests

1. Use `proptest` for:
   - Token counting: any valid UTF-8 string → count never panics.
   - Edit tool: any old/new string combination → either succeeds or returns is_error, never panics.
   - Session JSONL: any `SessionLine` → serialization round-trip is lossless.
   - TaskGraph: any list of tasks with random dependencies → `validate()` + `execution_order()` never panics.

---

### Milestone 9.2 — Benchmarks

**Dependency:** 9.1 complete.

#### 9.2.1 Add criterion benchmarks

1. In `alfred-core/benches/`:
   - Benchmark JSON serialization of large `Message` arrays (simulating 50-turn context).
2. In `alfred-context/benches/`:
   - Benchmark token counting for 100k-token contexts.
3. In `alfred-tools/benches/`:
   - Benchmark Grep over a large directory (e.g. the Linux kernel source).
   - Benchmark file read cache hit vs. miss.
4. In `alfred-index/benches/`:
   - Benchmark symbol search over a 1000-symbol index.
5. Run: `cargo bench`. Track in CI as regression check.

---

### Milestone 9.3 — Structured Telemetry

**Dependency:** 9.1 complete.

**Rationale:** Production systems need observability: which tools are slow, which model calls fail, what the per-task cost distribution looks like, where retries happen.

#### 9.3.1 Define telemetry event types

1. In `alfred-core/src/telemetry.rs`:

   ```rust
   pub struct TelemetryEvent {
       pub session_id: String,
       pub event_type: TelemetryEventType,
       pub timestamp: DateTime<Utc>,
       pub duration_ms: Option<u64>,
       pub metadata: serde_json::Value,
   }

   pub enum TelemetryEventType {
       ToolCall,
       ProviderRequest,
       ProviderRetry,
       SessionStart,
       SessionEnd,
       PlannerCall,
       CompactionTriggered,
       MemorySave,
       MemorySearch,
   }
   ```

#### 9.3.2 Instrument key code paths

1. In `alfred-agent/src/executor.rs`:
   - Before tool call: record start time.
   - After tool call: emit `TelemetryEvent { event_type: ToolCall, duration_ms, metadata: { tool_name, is_error } }`.
2. In `alfred-providers/src/anthropic.rs`:
   - Emit `ProviderRequest` event with `{ model, input_tokens, output_tokens, cost_usd, duration_ms }`.
   - Emit `ProviderRetry` event on each retry attempt.
3. In `alfred-context/src/compaction.rs`:
   - Emit `CompactionTriggered` with `{ tokens_before, tokens_after }`.
4. In `alfred-planner/src/planner.rs`:
   - Emit `PlannerCall` with `{ task_count, duration_ms }`.

#### 9.3.3 Telemetry output targets

1. Default: emit to `tracing::info!` spans (already wired to `tracing-subscriber`).
2. Optional: write structured JSON to `{data_dir}/telemetry.jsonl` (append-only).
3. `alfred stats` subcommand:
   - Read `telemetry.jsonl` for current session or all sessions.
   - Display: tool call frequency table, average latency per tool, total model cost, retry rate.
4. Test: run a multi-step task; verify `alfred stats` outputs correct tool counts.

---

### Milestone 9.4 — Documentation

**Dependency:** 9.2 complete.

#### 9.4.1 Public API documentation

1. Add `//!` module-level doc comments to every `lib.rs`.
2. Add `///` doc comments to every public type and function.
3. Run `cargo doc --workspace --no-deps`; verify no warnings.
4. Add doc tests (`///` examples) for key public functions.

#### 9.4.2 User documentation

1. Write `README.md`:
   - Installation (`cargo install alfred` or binary releases).
   - Quick start (5 lines to first working agent call).
   - Configuration reference.
   - Tool reference (same format as `devdocs/REPORT.md` tool table).
   - Provider setup (API keys, local models).
   - Session management.
   - MCP configuration.
   - Memory system usage.
   - Planner mode.
   - Repo indexing.
2. Write `CONTRIBUTING.md`: how to add a new tool, how to add a new provider.

#### 9.4.3 Architecture documentation

1. Write `docs/architecture.md`:
   - Module dependency graph (mermaid diagram).
   - Data flow diagram (agent loop → context engine → provider → tool executor).
   - Session JSONL format spec.
   - Tool schema format spec.
   - Task graph JSON schema.
   - Memory DB schema.

---

### Milestone 9.5 — Packaging and Distribution

**Dependency:** 9.3 complete.

#### 9.5.1 Cross-platform builds

1. Add GitHub Actions matrix: `ubuntu-latest`, `macos-latest`, `windows-latest`.
2. Build static binaries with `musl` target on Linux: `x86_64-unknown-linux-musl`.
3. Build universal binary on macOS: `x86_64-apple-darwin` + `aarch64-apple-darwin`, then `lipo`.
4. Upload release artifacts to GitHub Releases.

#### 9.5.2 Homebrew formula

1. Create `alfred.rb` formula:
   - Download release tarball.
   - `bin.install "alfred"`.
   - Install shell completions.
2. Publish to a Homebrew tap.

#### 9.5.3 `cargo install` support

1. Verify `cargo install alfred` (from crates.io) works.
2. Publish `alfred-core`, `alfred-tools`, `alfred-providers`, `alfred-context`, `alfred-storage`, `alfred-memory`, `alfred-planner`, `alfred-index`, `alfred-agent`, `alfred-cli` to crates.io in dependency order.

---

### Milestone 9.6 — Production Hardening

**Dependency:** 9.4 complete.

#### 9.6.1 Memory leak and resource auditing

1. Run `valgrind` (Linux) or `Instruments` (macOS) on a long agent session.
2. Verify no unbounded memory growth over 100 turns.
3. Verify all spawned subprocesses are reaped.
4. Verify all file handles are closed after tool execution.

#### 9.6.2 Fuzz testing

1. Use `cargo-fuzz` on:
   - `SessionReader::load()` — fuzz JSONL input.
   - `EditTool::execute()` — fuzz file content + old/new strings.
   - `GrepTool::execute()` — fuzz regex patterns.
   - `TaskGraph::validate()` — fuzz task graph JSON.
2. Run for 1 hour; fix any panics or crashes found.

#### 9.6.3 Security audit

1. Run `cargo audit` to check for known CVEs in dependencies.
2. Run `cargo deny check` with a policy file (`licenses`, `bans`, `advisories`).
3. Review all `unsafe` blocks (should be zero outside FFI if using pure Rust).
4. Review all subprocess spawning for injection vulnerabilities.

#### 9.6.4 Release checklist

- [ ] All tests pass on all platforms
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] `cargo audit` clean
- [ ] `cargo deny check` clean
- [ ] No `unwrap()` or `expect()` in non-test code (replaced with `?` or explicit error handling)
- [ ] Version bumped in all `Cargo.toml` files
- [ ] `CHANGELOG.md` updated
- [ ] GitHub Release created with binaries
- [ ] Homebrew formula updated
- [ ] crates.io published

---

## Dependency Map

```
Phase 1 (Foundation)
  └─ Phase 2 (PoC)
       └─ Phase 3 (MVA)
            ├─ Phase 4 (Feature Expansion)
            │    ├─ Phase 5 (Reliability)
            │    │    ├─ Phase 6 (Performance)
            │    │    ├─ Phase 7 (Security)
            │    │    └─ Phase 8 (DX)
            │    │         └─ Phase 9 (Production)
            │    └─ Phase 7 (Security)
            └─ Phase 4

Task-level dependencies (critical path):
1.1 Workspace Init
  → 1.2 Core Types
    → 1.3 Tracing
      → 2.1 Anthropic Provider
        → 2.2 Bash Tool (Tool trait + is_read_only)
          → 2.3 Agent Loop PoC
            → 3.1 All Tools
              → 3.2 Clap CLI
                → 3.3 Session Storage
                  → 3.4 Config Loading + Tool Guidance Prompts (3.4.3)
                    → 3.5 Streaming (partial JSON assembly)
                      → 4.1 Multi-Provider
                      → 4.2 Context Engine
                      → 4.3 Permissions
                        → 4.5 Plan Mode
                      → 4.6 Parallel Tools + Semaphore (4.6.3)
                        → 4.7 Subagents
                          → 4.8 Task Graph / Planner
                            → 8.6 Live Plan Visualization
                      → 5.1 Error Handling
                        → 5.2 Session Recovery
                        → 5.3 Graceful Shutdown
                        → 5.5 Memory System
                          → 6.x Performance
                            → 6.4 File Read Cache
                          → 7.x Security
                          → 8.x DX
                            → 8.3 MCP
                            → 8.7 Repo Indexing (optional)
                              → 9.x Production
                                → 9.3 Telemetry
```

---

## Recommended Crates Reference

| Crate | Version | Purpose |
|-------|---------|---------|
| `tokio` | 1.x | Async runtime (full features) |
| `clap` | 4.x | CLI argument parsing (derive feature) |
| `serde` + `serde_json` | 1.x | Serialization |
| `thiserror` | 1.x | Structured error types |
| `anyhow` | 1.x | Ad-hoc error chaining |
| `tracing` + `tracing-subscriber` | 0.1/0.3 | Structured logging and telemetry spans |
| `reqwest` | 0.12 | HTTP client (json + stream features) |
| `tokio-stream` | 0.1 | Async streaming |
| `futures` | 0.3 | Future combinators (join_all, etc.) |
| `uuid` | 1.x | Session ID generation (v4) |
| `chrono` | 0.4 | Timestamps (serde feature) |
| `directories` | 5.x | XDG/platform config and data paths |
| `glob` | 0.3 | File glob matching |
| `ignore` | 0.4 | gitignore-aware directory walking |
| `regex` | 1.x | Regular expression matching |
| `grep` | 0.3 | ripgrep-family grep engine |
| `similar` | 2.x | Unified diff generation for Edit tool |
| `tiktoken-rs` | 0.5 | Token counting (cl100k/o200k, approximate) |
| `async-trait` | 0.1 | Async trait methods |
| `toml` | 0.8 | Config file parsing |
| `rustyline` | 14.x | Interactive REPL with history |
| `wiremock` | 0.6 | HTTP mock server for tests |
| `jsonschema` | 0.17 | JSON Schema validation for tool inputs |
| `proptest` | 1.x | Property-based testing |
| `criterion` | 0.5 | Benchmarking |
| `clap_complete` | 4.x | Shell completion generation |
| `clap_mangen` | 0.2 | Man page generation |
| `rusqlite` or `sqlx` | latest | SQLite for long-term memory |
| `lru` | 0.12 | LRU cache for file reads |
| `tree-sitter` | 0.22 | Code parsing for symbol extraction (optional) |
| `tree-sitter-rust` | 0.21 | Rust grammar for tree-sitter (optional) |
| `tantivy` | 0.22 | Full-text search for repo indexing (optional) |
| `seccomp` | 0.1 | Linux syscall sandboxing (optional) |

---

*Generated from: `devdocs/REPORT.md`, `devdocs/ARTIFACTS.md`, `devdocs/Instructions.md`*
*Updated: 2026-03-16 — added bounded concurrency (4.6.3), tool guidance prompts (3.4.3), subagent architecture (4.7), task graph planner (4.8), memory system (5.5), file read LRU cache (6.4), live plan visualization (8.6), repo indexing (8.7), structured telemetry (9.3)*
