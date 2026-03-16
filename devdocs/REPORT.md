# CLI Coding Agents: Reverse-Engineering Report

**Purpose:** Reconstruct how modern CLI coding agents (Claude CLI, Cursor agent) operate so an engineer can implement a comparable agent. Claims are backed by **execution traces**, **code evidence**, **binary/protocol analysis**, or **documentation**. Where something cannot be proven, it is marked **UNCERTAIN** and the missing evidence is stated.

**Primary targets:** Claude CLI (`claude`), Cursor agent (`agent`).

**Implementation-level artifacts:** For extracted traces, tool statistics, edit patterns, error handling, context reconstruction, repo exploration heuristics, binary strings, and Cursor bundle snippets, see **[ARTIFACTS.md](./ARTIFACTS.md)**.

---

## Part A — Discovery and binaries

| Binary    | Path / runtime |
|-----------|----------------|
| **claude** | `/opt/homebrew/bin/claude` → Homebrew cask; single Mach-O arm64, ~189 MB. Segments `__jsc_int`, `__jsc_opcodes` (JavaScriptCore). Closed source. |
| **agent**  | `~/.local/bin/agent` → `~/.local/share/cursor-agent/versions/<version>/cursor-agent` (shell) → `node --use-system-ca "$SCRIPT_DIR/index.js" "$@"`. Node.js webpack bundle + chunks. |

**Claude CLI (flags):** `-p`/`--print`, `--output-format` (text|json|stream-json), `--tools`, `--allowedTools`, `--disallowedTools`, `--permission-mode`, `--max-turns`, `--max-budget-usd`, `--system-prompt`, `--resume`, `--continue`, `--mcp-config`, etc.

**Cursor agent (flags):** `-p`/`--print`, `--mode` (plan | ask), `--resume`, `--force`/`--yolo`, `--sandbox`, `--output-format`, `--model`, `--list-models`, etc.

---

## Part B — Evidence-only reconstruction

### 1. Complete execution traces

**Source:** Session files under `~/.claude/projects/<sanitized-path>/<session-id>.jsonl`. Each line is one JSON object. Format matches Agent SDK `message_parser.py`.

**Observed loop:**

| Step | type       | Content shape |
|------|------------|----------------|
| 1    | user       | `message.role`: "user", `message.content`: string (prompt) |
| 2    | assistant  | `message.content`: [{ "type": "text", "text": "..." }] |
| 3    | assistant  | `message.content`: [{ "type": "tool_use", "id": "toolu_...", "name": "Bash"|"Read"|"Edit"|..., "input": {...} }] (possibly multiple) |
| 4    | user       | `message.content`: [{ "tool_use_id": "...", "type": "tool_result", "content": "..." }] per tool |
| 5+   | repeat 2–4 until no tool_use |
| end  | result     | `subtype`, `duration_ms`, `is_error`, `num_turns`, `session_id`, `total_cost_usd`, `usage`, `result` |

Optional: **progress** (hook_progress, PostToolUse), **system** (subtype init, compact_boundary, task_*).

**Evidence:** Session JSONL; SDK `message_parser.py`.

---

### 2. Model request structure

- **API request body:** Not observed (CLI is closed binary). **UNCERTAIN:** system prompt text, message array order, tool schema JSON, tool_choice. **Would need:** HTTP proxy capture or official request docs.
- **Output:** Assistant messages include `usage` (`input_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`, `output_tokens`) — API supports prompt caching. Tool results appear as user content (tool_result blocks). **Evidence:** Session JSONL; message_parser.
- **SDK → CLI:** CLI invoked with `--output-format stream-json`, `--input-format stream-json`, `--verbose`, plus `--tools`, `--allowedTools`, etc. Tool definitions are not sent over stdin; they are inside the CLI. First message from SDK: `control_request` with `subtype: "initialize"`; then stream of user/assistant messages as newline-delimited JSON. **Evidence:** `subprocess_cli.py` `_build_command()`, `query.py` `initialize()`, `stream_input()`.

---

### 3. Tool schema reconstruction

**Claude CLI (from session traces):**

| Tool   | Parameters (observed) |
|--------|------------------------|
| Read   | `file_path` (string). Optional: `offset`, `limit` (line-based). |
| Edit   | `file_path`, `old_string`, `new_string`, `replace_all` (boolean). |
| Write  | `file_path`, `content` (string). |
| Bash   | `command` (string). Optional: `description`, `timeout` (ms). |
| Glob   | `pattern`, `path` (strings). |
| Grep   | `pattern`, `path`. Optional: `output_mode`, `context` (number). |

**Evidence:** Parsed tool_use blocks from session JSONL under `~/.claude/projects`. Official list: [code.claude.com/docs/en/tools-reference](https://code.claude.com/docs/en/tools-reference).

**Cursor (from bundle 414.index.js):** Proto tool cases: shellToolCall, grepToolCall, semSearchToolCall, editToolCall, readToolCall (path, offset, limit), deleteToolCall, lsToolCall, globToolCall, readLintsToolCall, mcpToolCall, webSearchToolCall, webFetchToolCall, taskToolCall, askQuestionToolCall, switchModeToolCall, createPlanToolCall, applyAgentDiffToolCall, etc. Edit result: `beforeFullFileContent`, `afterFullFileContent`. **UNCERTAIN:** Full proto definitions; only client-side handling is visible.

---

### 4. Context assembly

- **Claude:** Linear user/assistant history; tool results as user content (tool_result blocks). System prompt via CLI flags; CLAUDE.md when using setting sources. Compaction: older messages summarized, system message `compact_boundary`. **Evidence:** Session structure; SDK docs; message_parser.
- **Cursor:** UserMessage has `text`, `selectedContext`, `messageId`, `mode`. Resource context truncated to 20k chars (constant `P=2e4`). Conversation state: `agentStore.getConversationStateStructure()` sent to backend. **Evidence:** 414.index.js `processPrompt`, `buildPromptResourceContext`, `truncatePromptResourceContent`, `agentClient.run(...)`.

**UNCERTAIN:** Default system prompt text (Claude); how backend builds full message array (Cursor).

---

### 5. Repository navigation

- **Claude (traces):** Bash → multiple Read → Glob/Grep → Read → Edit. Glob: `pattern` + `path`. Grep: `pattern`, `path`, optional `output_mode`, `context`. Read: absolute paths, optional offset/limit. **UNCERTAIN:** Whether CLI injects file tree into first request.
- **Cursor:** semSearchToolCall (query), globToolCall, grepToolCall, lsToolCall, readToolCall in bundle. **UNCERTAIN:** Ranking/selection logic (likely server-side).

---

### 6. Code edit strategy

- **Claude:** Edit = search-replace (`old_string`, `new_string`, `replace_all`). Success: "The file ... has been updated successfully." Write = full `content`. **Evidence:** Session Edit/Write tool_use and tool_result.
- **Cursor:** editToolCall result has `beforeFullFileContent`, `afterFullFileContent`; separate `applyAgentDiffToolCall`. **UNCERTAIN:** Diff format and application logic.

**UNCERTAIN (Claude):** Retry when old_string not found; conflict handling.

---

### 7. Decision and planning logic

- **Claude:** Same model produces text and tool_use; no separate planning call. Plan mode = read-only tools only (SDK docs). Read-only tools (Read, Glob, Grep) can run in parallel; Edit, Write, Bash sequentially (SDK docs). **Evidence:** Trace sequence; [Agent SDK agent loop](https://platform.anthropic.com/docs/en/agent-sdk/agent-loop).
- **Cursor:** createPlanToolCall, switchModeToolCall (plan / agent / ask). **Evidence:** 414.index.js.

---

### 8. Error recovery

- **Claude:** tool_result with `is_error: true` observed: content "File does not exist. Note: your current working directory is ...". SDK: ToolResultBlock(`is_error`), ResultMessage(`is_error`). Permission denial: control_request `can_use_tool` → SDK responds deny → CLI passes as tool result. **Evidence:** Session scan for is_error; message_parser.py; query.py.
- **Cursor:** Tool results include success/failure/error (e.g. shell exitCode, stdout, stderr; grep error message). **Evidence:** 414.index.js extractToolCallOutput. **UNCERTAIN:** Agent retry or repair behaviour.

---

### 9. Prompt discovery

- **Claude:** Override via `--system-prompt`, `--append-system-prompt`, `--system-prompt-file`; CLAUDE.md with setting sources. **UNCERTAIN:** Default system prompt and tool instructions (inside CLI).
- **Cursor:** **UNCERTAIN:** Prompts built on backend; CLI sends UserMessage (text, selectedContext, mode).

---

### 10. Cursor architecture — where the "brain" lives

- **Entry:** Shell → `node index.js`. **Evidence:** cursor-agent script.
- **Run flow:** `agentClient.run(t, getConversationStateStructure(), new ConversationAction({ userMessageAction: UserMessage({ text, selectedContext, messageId, mode }) }), currentModel, callbacks, resources, ...)`. Callbacks: sendUpdate (textDelta, partialToolCall, toolCallStarted, toolCallCompleted, thinkingDelta), query (askQuestion, createPlan, webFetch, webSearch). **Evidence:** 414.index.js (agent-session.ts).
- **Conclusion:** Model and agent loop run in the **backend** (agentClient). CLI is a thin client: builds UserMessage, sends it, displays stream. local-exec, shell-exec, hooks-exec, mcp-agent-exec used for **local** tool execution. **Evidence:** agentClient.run; session-resources; shared-services (aiserver_connect, agent-client).

**Internal packages (from bundle):** agent-core, agent-client, agent-transcript, agent-kv, context, proto (agent/v1, aiserver/v1), local-exec, shell-exec, hooks-exec, mcp-agent-exec, cursor-config, ink, sandbox-gate, decision providers (always-approve, always-deny).

---

### 11. Implementation checklist (from proven facts)

1. **Loop:** User message → model (system + tools + history) → if tool_use then execute (parallel read-only, sequential state-changing) → append tool_result to conversation → repeat until no tool_use → emit result.
2. **Wire (subprocess):** Newline-delimited JSON; first message control_request initialize; handle can_use_tool, hook_callback, mcp_message from child.
3. **Tools (minimal):** Read(file_path; offset?, limit?), Edit(file_path, old_string, new_string, replace_all), Write(file_path, content), Bash(command; description?, timeout?), Glob(pattern, path), Grep(pattern, path; output_mode?, context?) with types from traces.
4. **Context:** System prompt + tool definitions + full conversation (including tool_result). Truncate/summarize when near limit; emit compact_boundary-style event.
5. **Edit:** Search-replace; success message; on failure return tool_result with is_error and message.
6. **Errors:** Pass tool failure to model as tool_result (is_error, content); no automatic retry required by evidence.
7. **Session:** session_id, resume, optional file checkpointing and rewind (SDK supports rewind_files).

---

## Part C — Comparison and design takeaways

**Why Claude CLI and Cursor agent are stronger (evidence-based):**

- **Explicit, granular tools** with permission semantics (and optional sandbox): Read, Edit, Write, Bash, Glob, Grep, Web*, MCP; plan/ask modes restrict to read-only.
- **Session identity:** session_id, resume, fork; Cursor resume by chatId.
- **Headless path:** `-p`, JSON/stream-json, max_turns, max_budget_usd for scripting/CI.
- **Context discipline:** compaction, project instructions (CLAUDE.md), subagents (Claude); Cursor sends conversation state to backend.
- **Worktree isolation** (both); sandbox for Bash (Claude documented; Cursor sandbox-gate in bundle).
- **Cursor:** Backend runs model and loop; CLI does UI, permissions, local tool execution — allows multi-model and central updates.

**Engineering principles for a new CLI agent:**

- Single agent loop (prompt → model → tools → results → repeat); named tools at least Read, Edit, Write, Bash, Glob, Grep; permission model (allow/deny, plan mode).
- Pre-edit checkpoints; optional OS sandbox for Bash; tool-level allow/deny patterns.
- Session ID, resume, optional fork; compaction when near context limit; optional project file (e.g. CLAUDE.md).
- Non-interactive flag (`-p`), JSON/stream output, max_turns/max_budget; MCP or equivalent; PreToolUse/PostToolUse hooks.
- Start with SDK-style single-process loop (like Claude Agent SDK), then add backend if needed (Cursor-style).

---

## References

- **Traces:** `~/.claude/projects/` session `.jsonl` files.
- **SDK:** [anthropics/claude-agent-sdk-python](https://github.com/anthropics/claude-agent-sdk-python) — `_internal/transport/subprocess_cli.py`, `_internal/query.py`, `_internal/message_parser.py`.
- **Docs:** [code.claude.com/docs/en/tools-reference](https://code.claude.com/docs/en/tools-reference), [platform.anthropic.com/docs/en/agent-sdk/agent-loop](https://platform.anthropic.com/docs/en/agent-sdk/agent-loop), [code.claude.com/docs/en/cli-reference](https://code.claude.com/docs/en/cli-reference).
- **Cursor bundle:** `~/.local/share/cursor-agent/versions/2026.02.27-e7d2ef6/` — 414.index.js (agent-session.ts), session-resources, shared-services.
