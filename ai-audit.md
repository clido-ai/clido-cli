# clido CLI Agent — Comprehensive Audit & Improvement Plan

**Date:** 2026-04-15
**Scope:** Full codebase audit of `crates/clido-cli`, `crates/clido-agent`, `crates/clido-tools`, `crates/clido-workflows`
**Method:** Line-by-line reading of all source files + runtime behavior analysis

---

## Table of Contents

1. [Agent Loop — Critical Issues](#1-agent-loop--critical-issues)
2. [TUI — User Experience Problems](#2-tui--user-experience-problems)
3. [Tool System — Insufficient Implementation](#3-tool-system--insufficient-implementation)
4. [Workflows — Fragile Execution](#4-workflows--fragile-execution)
5. [Session Management — Data Integrity Risks](#5-session-management--data-integrity-risks)
6. [Command System — Poor Discoverability](#6-command-system--poor-discoverability)
7. [Permissions — Repetitive & Cumbersome](#7-permissions--repetitive--cumbersome)
8. [Configuration — Missing Features](#8-configuration--missing-features)
9. [Code Quality & Architecture](#9-code-quality--architecture)
10. [Missing Features — Industry Standard](#10-missing-features--industry-standard)

---

## 1. Agent Loop — Critical Issues

### 1.1 No streaming to TUI chat
**File:** `crates/clido-agent/src/agent_loop/completion.rs`
**Severity:** HIGH

The agent calls `provider.complete()` (blocking) and only emits the full response text at the end. There is **no token-by-token streaming** to the TUI. The user stares at a frozen UI for 10-60 seconds while the LLM generates.

```rust
// completion.rs — single blocking call
let response = provider.complete(&to_send, &schemas, &req_config).await?;
// Full response only available after 10-60s of waiting
```

**Fix:** Use `provider.complete_stream()` (which exists on the trait but is `unimplemented!()` in the mock provider) to stream tokens to the TUI in real-time. The `EventEmitter::on_assistant_text` callback already exists — it just needs to be fed incrementally.

### 1.2 Thinking blocks silently discarded
**File:** `crates/clido-agent/src/agent_loop/completion.rs` line ~200-230

The `StreamEvent::Content` handler filters out `ContentBlock::Thinking` blocks entirely. The agent's reasoning process is completely invisible to the user. For models that output thinking/reasoning (Claude, o-series), this is a poor UX.

**Fix:** Stream thinking blocks to a collapsible section in the TUI or show them as dimmed/faint text.

### 1.3 Consecutive tool errors counter never resets between user messages
**File:** `crates/clido-agent/src/agent_loop/mod.rs` line ~1828-1841

The `consecutive_tool_errors` counter is incremented but only resets when a turn has zero errors. If the user sends a new message while the counter is at 2, it carries over from the previous turn.

**Fix:** Reset `consecutive_tool_errors = 0` at the start of each outer user turn (`run` / `run_next_turn`).

### 1.4 Tool recovery nudge is generic and unhelpful
**File:** `crates/clido-agent/src/agent_loop/mod.rs` line ~56-81, `crates/clido-agent/src/prompts.rs`

The `prepend_tool_recovery_nudge` prepends a generic "the following tools failed" message. It doesn't categorize errors, suggest alternatives, or explain what went wrong.

**Fix:** Make the recovery nudge actionable: categorize errors (file not found, permission denied, syntax error, etc.) and provide specific guidance per category.

### 1.5 Auto-checkpoint uses `--no-verify` and stages everything
**File:** `crates/clido-agent/src/agent_loop/mod.rs` line ~85-138

The `maybe_create_checkpoint` function runs `git add -A` followed by `git commit --no-verify`. This:
- Stages and commits untracked files the user didn't intend to commit
- Skips pre-commit hooks (linting, formatting checks)
- Only runs once per session (first write), missing subsequent dangerous operations

**Fix:**
- Don't stage untracked files — only commit already-tracked changes
- Don't use `--no-verify` — let hooks run
- Or make this configurable via `[agent] auto-checkpoint = true/false`

### 1.6 Permission timeout message says 60s but actually 900s
**File:** `crates/clido-agent/src/agent_loop/mod.rs` line ~846-860

The timeout uses `Duration::from_secs(900)` (15 minutes) but the error message says `"Permission request timed out after 60s"`.

**Fix:** Make the message match the actual timeout, or reduce the timeout to 60s.

### 1.7 Budget estimation uses hardcoded defaults for unknown providers
**File:** `crates/clido-agent/src/agent_loop/mod.rs` line ~1366-1381

When pricing table doesn't have an entry for the current model, it falls back to:
```rust
const DEFAULT_INPUT_USD_PER_1M: f64 = 3.0;
const DEFAULT_OUTPUT_USD_PER_1M: f64 = 15.0;
```
This is wildly inaccurate for cheap models (e.g., GPT-4o-mini at $0.15/$0.60 per 1M) and overpriced for others.

**Fix:** Fetch pricing dynamically from provider APIs, or maintain a proper pricing table.

### 1.8 Doom detection threshold too low for legitimate permission flows
**File:** `crates/clido-agent/src/agent_loop/doom.rs`

The doom tracker fires when 3 consecutive identical permission requests occur. Reading files from an external directory triggers this — exactly the issue the user reported. Our fix (matching by directory) helps but the threshold is still fragile.

**Fix:** 
- Increase default threshold from 3 to 5
- Exclude `PermissionRequest` events from doom detection entirely (they're user-interaction, not agent loops)
- Or add a separate "permission loop" detector with higher threshold

### 1.9 Stall tracker only observes read-only tools
**File:** `crates/clido-agent/src/agent_loop/stall.rs`

The stall tracker's `observe_batch` only increments scores for tools that are `parallel_safe_in_model_batch()` — which excludes Write, Edit, Bash, etc. So the stall detector never fires for the tools most likely to cause stalls.

**Fix:** Include all tools in stall observation, or specifically track Bash loops (the most common stall pattern).

---

## 2. TUI — User Experience Problems

### 2.1 Chat rendering truncates long responses with no scroll indicator
**File:** `crates/clido-cli/src/tui/render/mod.rs`

The chat area uses a fixed `chat_h` calculation but doesn't show a "more below" indicator when content overflows. Users don't know there's more content to scroll to.

**Fix:** Add a `▼` or `...more` indicator at the bottom of the chat area when scroll offset < max.

### 2.2 Status panel shows stale tool execution info
**File:** `crates/clido-cli/src/tui/render/status_panel.rs`

The status panel tracks `last_tool` and `tool_duration` but these are only updated when `ToolStart`/`ToolDone` events arrive. If a tool fails silently or the channel drops, the panel shows the previous tool as "still running."

**Fix:** Add a timeout-based indicator — if no `ToolDone` event arrives within `tool_timeout_secs`, show a timeout warning.

### 2.3 No visual feedback when typing in complex mode
**File:** `crates/clido-cli/src/tui/state.rs` line ~70-80, `crates/clido-cli/src/tui/input/mod.rs`

The `Complexity` enum (Simple/Complex) controls whether input is a single line or multi-line, but there's no visible indicator showing which mode is active. Users type multi-line messages thinking they're single-line and vice versa.

**Fix:** Show `[multi-line]` or `[single-line]` indicator in the input bar. Shift+Enter already toggles — make the state visible.

### 2.4 Session list shows "empty" for sessions with content
**File:** `crates/clido-cli/src/tui/render/session_list.rs`

The session picker reads `session_id` from the directory name (which is the first message text) but many sessions have garbled or empty names due to how session IDs are generated.

**Fix:** Parse the first user message from the session JSONL file to extract a proper title, or store session titles in a separate metadata file.

### 2.5 No undo/redo for text input
**File:** `crates/clido-cli/src/tui/input/mod.rs`

The text input has cursor navigation, clipboard, and editing but no undo/redo. Accidental deletions are permanent.

**Fix:** Add a simple undo stack (store text snapshots on each modification) with Ctrl+Z / Ctrl+Shift+Z.

### 2.6 Copy mode requires awkward Ctrl+Shift+C
**File:** `crates/clido-cli/src/tui/copy.rs`

The copy mode (for selecting and copying text from the chat) uses Ctrl+Shift+C which conflicts with terminal emulator shortcuts. It also requires entering a separate "mode" rather than just selecting with mouse.

**Fix:** Support mouse selection natively (bracketed paste mode), or use a simpler shortcut like Ctrl+O.

### 2.7 Profile picker doesn't show model names
**File:** `crates/clido-cli/src/tui/input/profile.rs`

The profile picker shows profile names but not the underlying model names. Users don't know which model a profile uses without checking the config file.

**Fix:** Show `Profile Name (model-name)` in the picker list.

### 2.8 Plan panel shows stale plan after task completion
**File:** `crates/clido-cli/src/tui/render/plan.rs`

After the agent finishes a task, the plan panel still shows the old plan with completed steps. There's no indication that the task is done or a way to dismiss the plan.

**Fix:** Fade out or collapse the plan panel after task completion, or add a "Task complete" banner.

### 2.9 No keyboard shortcuts reference/help
**File:** N/A (missing entirely)

There is no `?` or `/help` command that shows available keyboard shortcuts. Users must read the README or discover shortcuts by accident.

**Fix:** Add a `?` keybinding that shows a modal with all available shortcuts, or add a `/shortcuts` slash command.

### 2.10 Workflow input form has no field validation feedback
**File:** `crates/clido-cli/src/tui/state.rs` line ~263-286

The `WorkflowInputField` struct has a `validation_message` field but it's never populated or displayed. Users can submit invalid values with no feedback.

**Fix:** Show validation errors inline below each field, and prevent submission until all fields pass validation.

---

## 3. Tool System — Insufficient Implementation

### 3.1 Bash tool has no output streaming
**File:** `crates/clido-tools/src/bash.rs`

The Bash tool collects all stdout/stderr into a buffer and returns it at the end. For long-running commands, the user sees nothing until the command completes (which could be minutes).

**Fix:** Stream partial output every N seconds or every N lines. Show "running..." indicator with elapsed time.

### 3.2 Bash tool timeout is silent
**File:** `crates/clido-tools/src/bash.rs` line ~130-150

When a command times out, the error message is generic: `"Command timed out after {n} seconds"`. It doesn't show partial output or suggest increasing the timeout.

**Fix:** Include partial stdout/stderr in the timeout message and suggest `timeout: 120` in the tool schema.

### 3.3 Read tool has no encoding detection
**File:** `crates/clido-tools/src/read.rs`

The Read tool uses `String::from_utf8_lossy()` which silently replaces non-UTF8 bytes with ``. Binary files (images, PDFs, compiled artifacts) are corrupted in the output.

**Fix:** Detect binary files early and return a clear error: `"Binary file detected — use 'file' command to inspect"`.

### 3.4 Read tool caches indefinitely
**File:** `crates/clido-tools/src/read.rs` line ~30-50, `crates/clido-tools/src/file_tracker.rs`

The `FileTracker` tracks mtime but only for Write staleness checks. Read results are cached with no TTL or size limit. In long sessions, the cache grows unbounded.

**Fix:** Add a cache size limit (e.g., 100 files, 10MB total) with LRU eviction.

### 3.5 Edit tool's `old_string` matching is fragile
**File:** `crates/clido-tools/src/edit.rs`

The Edit tool uses `replacen(old_string, new_string, 1)` which:
- Fails silently if `old_string` appears multiple times (only replaces first)
- Fails completely if `old_string` doesn't match exactly (whitespace differences, encoding)
- No fuzzy matching or "show me similar strings" fallback

**Fix:**
1. If exact match fails, try whitespace-normalized match
2. If multiple matches, return an error listing all occurrences with line numbers
3. Add a `--force` flag that replaces all occurrences

### 3.6 Write tool's content preview can be massive
**File:** `crates/clido-tools/src/write.rs` line ~146-155

The content preview shows the first 15 lines of written content. For large files (e.g., generated code, data dumps), this floods the chat with irrelevant text.

**Fix:** 
- Cap preview at ~200 characters or 5 lines for large files
- Only show first/last 3 lines for files > 100 lines
- Or make preview configurable

### 3.7 No image/viewer tool
**File:** N/A (missing entirely)

There's no tool to preview images, render markdown, or view HTML. The agent can generate images or charts but has no way to display them.

**Fix:** Add a `Preview` tool that opens images/markdown/HTML in the appropriate viewer.

### 3.8 Grep tool has no line number context option
**File:** `crates/clido-tools/src/grep_tool.rs`

The Grep tool returns matching lines but doesn't include surrounding context. Users (and the agent) can't understand matches without reading the full file.

**Fix:** Add `context_lines: 3` parameter (like `grep -C 3`) as default.

### 3.9 Glob tool doesn't respect .gitignore
**File:** `crates/clido-tools/src/glob_tool.rs`

The Glob tool uses `std::fs::read_dir` and manual filtering. It doesn't respect `.gitignore`, `.clidoignore`, or other ignore files.

**Fix:** Use the `ignore` crate (same as ripgrep) to respect `.gitignore` patterns.

### 3.10 WebFetch has no caching or rate limiting
**File:** `crates/clido-tools/src/web_fetch.rs`

WebFetch fetches the full page on every call. Repeated fetches of the same URL waste time and money.

**Fix:** Cache fetched URLs with a 5-minute TTL. Add rate limiting (max 10 requests per minute).

---

## 4. Workflows — Fragile Execution

### 4.1 Workflow halt state doesn't properly reset
**File:** `crates/clido-cli/src/tui/event_loop.rs` line ~860-880

When a workflow fails, `workflow_halted` is set to true. But the reset logic only fires on explicit `/workflow stop` — not on user messages. This means a failed workflow permanently blocks the session until explicitly stopped.

**Fix:** Reset `workflow_halted` when the user sends a new message, or add a "retry" option to the error message.

### 4.2 Foreach iteration has no progress indicator
**File:** `crates/clido-cli/src/tui/event_loop.rs` line ~3100-3130

When running a `foreach` workflow over a large list (e.g., 50 files), there's no progress bar or "X of Y completed" indicator. The user has no idea if it's halfway done or just starting.

**Fix:** Emit `RunState::Running` updates with progress: `"Processing item 5 of 50..."`.

### 4.3 Workflow input form doesn't support multi-line fields
**File:** `crates/clido-cli/src/tui/state.rs` line ~263-286

The `WorkflowInputField` struct has no multi-line support. Complex prompts or long file paths can't be entered comfortably.

**Fix:** Add a `multiline: bool` field to `WorkflowInputField` and use a multi-line editor for those fields.

### 4.4 Workflow error recovery is manual
**File:** `crates/clido-cli/src/tui/event_loop.rs`

When a workflow step fails, the entire workflow halts. There's no "retry failed step", "skip and continue", or "edit input and retry" option.

**Fix:** On workflow failure, present options:
- `[r]etry` — re-run the failed step
- `[s]kip` — skip this step and continue
- `[e]dit` — open the step's input for editing
- `[a]bort` — stop the workflow

### 4.5 Workflow YAML loader has no schema validation
**File:** `crates/clido-workflows/src/loader.rs`

The YAML loader parses workflows but doesn't validate against a schema. Typos in field names (e.g., `tool:` instead of `tool_name:`) silently create broken workflows.

**Fix:** Add a YAML schema validator (using `schemars` or manual validation) that reports clear errors for unknown fields and missing required fields.

### 4.6 Workflow variables can't reference each other
**File:** `crates/clido-workflows/src/executor.rs`

In the workflow executor, variables are resolved independently. You can't do:
```yaml
inputs:
  - name: base_dir
  - name: log_file
    default: "{{ base_dir }}/output.log"  # doesn't work
```

**Fix:** Add a second pass of variable resolution so that later inputs can reference earlier ones.

---

## 5. Session Management — Data Integrity Risks

### 5.1 Session files can grow unbounded
**File:** `crates/clido-storage/src/session.rs`

Sessions are stored as JSONL files with no size limit. Long sessions with many tool calls (especially Read returning large files) can grow to hundreds of MB.

**Fix:** 
- Implement session file rotation (split into chunks)
- Compact tool outputs in session files (store paths instead of full content)
- Add a `max-session-size` config option

### 5.2 Session resume can restore corrupted state
**File:** `crates/clido-agent/src/agent_loop/history.rs`

The `session_lines_to_messages` function doesn't validate the integrity of stored JSONL lines. If a session file is truncated or corrupted mid-write, resume attempts to parse garbage.

**Fix:** Add validation when loading sessions — skip malformed lines, report errors, and gracefully degrade.

### 5.3 No session export/import
**File:** N/A (missing entirely)

Users can't share, backup, or migrate sessions. Sessions are locked to the local filesystem.

**Fix:** Add `/session export` and `/session import` commands that serialize sessions to a portable format.

### 5.4 Session title generation is a waste of tokens
**File:** `crates/clido-agent/src/agent_loop/mod.rs` (title generation via LLM)

Every session triggers an LLM call to generate a title. This costs tokens and adds latency for what could be a simple heuristic (first 50 chars of user input).

**Fix:** Use a simple heuristic for titles by default, only use LLM for complex/multi-topic sessions.

---

## 6. Command System — Poor Discoverability

### 6.1 Slash commands require exact prefix match
**File:** `crates/clido-cli/src/tui/commands.rs` line ~85-100

The `slash_completions` function uses `starts_with` for matching. If you type `/modl` (typo), you get zero results instead of fuzzy matches for `/model`.

**Fix:** Add fuzzy matching (like `skim` or `fuzzy-matcher` crate) so `/modl` matches `/model`.

### 6.2 No command descriptions in completion list
**File:** `crates/clido-cli/src/tui/commands.rs`

The completion popup shows command names only, without descriptions. Users don't know what `/compact` does vs `/context` without reading docs.

**Fix:** Show `Command — short description` in the completion list.

### 6.3 Commands with args have no inline help
**File:** `crates/clido-cli/src/tui/input/mod.rs`

When you select `/model` or `/workflow`, there's no inline hint showing available arguments. Users must memorize or look up the syntax.

**Fix:** After selecting a command that takes args, show an inline hint: `/model <model-name>` or `/workflow <run|list|show|stop>`.

### 6.4 No command aliases or shortcuts
**File:** N/A

There are no aliases like `/s` for `/stop`, `/c` for `/compact`, or `/n` for `/new`. Power users type out full command names.

**Fix:** Add common aliases in the command registry.

---

## 7. Permissions — Repetitive & Cumbersome

### 7.1 Folder permissions still require individual file canonicalization
**File:** `crates/clido-tools/src/path_guard.rs`

Our fix adds `allowed_dirs` to `PathGuard`, but the `is_in_allowed_external` check still canonicalizes every path. On macOS, canonicalization can be slow for deep directory trees.

**Fix:** Pre-canonicalize allowed directories once at startup, cache the results.

### 7.2 No persistent permissions across sessions
**File:** `crates/clido-cli/src/tui/input/mod.rs`

The `[a]lways` option only grants permission for the current session. After restart, the user must re-grant access to the same external paths.

**Fix:** Store persistent permissions in `~/.config/clido/allowed-paths.toml` and load them at startup. Add a way to view and revoke persistent permissions.

### 7.3 Permission prompt blocks the entire TUI
**File:** `crates/clido-cli/src/tui/event_loop.rs` line ~3600-3620

When a permission prompt appears, the entire TUI is blocked waiting for user input. The agent can't continue with other work while waiting.

**Fix:** Allow the agent to continue with read-only tools while waiting for write permission. Only block the specific tool, not the entire loop.

---

## 8. Configuration — Missing Features

### 8.1 No per-project config
**File:** `crates/cli/src/config.rs`

Configuration is global (`~/.config/clido/config.toml`). There's no `.clido/config.toml` for project-specific settings (different model, different tools, different rules).

**Fix:** Add project-level config that overrides global config. Load order: global → project → CLI flags.

### 8.2 No environment variable interpolation in config
**File:** `crates/cli/src/config.rs`

Config values are literal strings. You can't do `api_key = "${OPENAI_API_KEY}"` or `workspace = "${HOME}/projects"`.

**Fix:** Add `${VAR}` interpolation in config values with fallback: `${VAR:default}`.

### 8.3 No config validation on startup
**File:** `crates/cli/src/config.rs`

Invalid config values (e.g., negative timeout, invalid model name) are only detected when they're used, leading to confusing error messages mid-session.

**Fix:** Validate all config values on startup and report errors immediately.

### 8.4 No way to view current config from TUI
**File:** N/A

There's no `/config` command or `--show-config` flag. Users must open the config file in an editor.

**Fix:** Add `/config` command that shows the current effective configuration (merged from all sources).

---

## 9. Code Quality & Architecture

### 9.1 Massive event_loop.rs (3600+ lines)
**File:** `crates/clido-cli/src/tui/event_loop.rs`

The event loop is a monolithic file that handles: agent lifecycle, tool execution, permission prompts, workflow management, session management, profile switching, model switching, and more. This is hard to navigate and test.

**Fix:** Split into modules:
- `event_loop/agent.rs` — agent start/stop/resume
- `event_loop/tools.rs` — tool execution and display
- `event_loop/permissions.rs` — permission handling
- `event_loop/workflows.rs` — workflow orchestration
- `event_loop/sessions.rs` — session management

### 9.2 Duplicate code in agent loop entry points
**File:** `crates/clido-agent/src/agent_loop/mod.rs`

`run`, `run_next_turn`, `run_with_extra_blocks`, `run_next_turn_with_extra_blocks`, `run_continue` all share 80% identical code (session checkpoint, history push, completion loop, rollback).

**Fix:** Extract a single inner function and have all entry points call it with different parameters.

### 9.3 Error types lack context
**File:** `crates/clido-core/src/lib.rs` (error types)

Many error variants carry only a string message without structured context (file path, tool name, line number). This makes debugging and error display harder.

**Fix:** Add structured fields to error types:
```rust
ToolExecution { tool: String, error: String, input: Value }
FileAccess { path: PathBuf, error: String, kind: Read | Write }
```

### 9.4 No integration tests for TUI
**File:** `crates/clido-cli/` — tests/

There are no integration tests for the TUI. The only tests are unit tests for individual components.

**Fix:** Add integration tests using `crossterm`'s test backend or a mock terminal.

### 9.5 Tool registry rebuild on every workdir change
**File:** `crates/clido-cli/src/tui/event_loop.rs` line ~900-940

When the working directory changes, the entire tool registry is rebuilt (new PathGuard, new file trackers, etc.). This is wasteful — only the path guard needs updating.

**Fix:** Allow in-place update of the path guard and file tracker without rebuilding the entire registry.

---

## 10. Missing Features — Industry Standard

### 10.1 No multi-file edit (like Claude's multi-file editing)
The agent can only edit one file at a time. Common workflows (refactor across files, rename symbol) require multiple sequential edits.

**Fix:** Add a `BatchEdit` tool that accepts multiple file edits and applies them atomically.

### 10.2 No "apply patch from clipboard" workflow
Users often have patches from code review tools (GitHub PR diffs, GitLab MR diffs) that they want to apply. There's no tool to parse and apply unified diffs from clipboard.

**Fix:** Add a `/apply-patch` command that reads a unified diff from clipboard and applies it using the existing `ApplyPatch` tool.

### 10.3 No terminal sharing (like Cursor's terminal integration)
The Bash tool runs commands but doesn't support interactive terminal sessions. The user can't `ssh` into a server and interact with the shell through clido.

**Fix:** Add an interactive terminal mode that shares a PTY between the user and the agent.

### 10.4 No codebase indexing for semantic search
The `SemanticSearch` tool exists but is a stub. There's no actual embedding or vector search implementation.

**Fix:** Implement local codebase indexing using sentence-transformers or a lightweight embedding model. Store embeddings in SQLite and query for semantic matches.

### 10.5 No diff review mode for the TUI
The `DiffReview` permission mode exists but the TUI doesn't render diffs inline. Users see raw JSON input instead of a proper unified diff.

**Fix:** Render the computed diff (from `compute_diff_for_tool`) inline in the permission prompt, showing exactly what will change.

### 10.6 No MCP (Model Context Protocol) support
Industry standard tools like Claude Desktop support MCP servers for extensibility. Clido has no MCP integration.

**Fix:** Add MCP server/client support to allow connecting external tools (databases, APIs, browsers) as MCP servers.

### 10.7 No agent-to-agent handoff
The sub-agent system exists but there's no way to delegate a complex task to a sub-agent with a specific role/system prompt and get back a structured result.

**Fix:** Add a `Delegate` tool that spawns a sub-agent with a custom system prompt, waits for completion, and returns the result.

### 10.8 No test-driven development loop
The agent can run tests but doesn't automatically: run tests → read failures → fix → re-run. This is a common workflow for coding agents.

**Fix:** Add a TDD mode where the agent:
1. Runs the test suite
2. Reads failure output
3. Identifies failing files
4. Reads and fixes them
5. Re-runs tests
6. Repeats until all pass

### 10.9 No git integration beyond checkpointing
The agent can create checkpoints but can't: create branches, commit with proper messages, push, create PRs, or review diffs.

**Fix:** Add git tools:
- `GitBranch` — create/switch branches
- `GitCommit` — commit with message
- `GitDiff` — show working tree diff
- `GitPush` — push to remote
- `GitPR` — create a PR (via gh CLI or API)

### 10.10 No browser automation
There's no way for the agent to interact with a web browser — fill forms, click buttons, screenshot pages, extract data from dynamic content.

**Fix:** Add browser automation tools using Playwright or CDP (Chrome DevTools Protocol).

---

## Priority Matrix

### P0 — Fix Immediately (broken UX)
| # | Issue | Impact |
|---|-------|--------|
| 1.1 | No streaming to TUI chat | User stares at frozen UI |
| 2.1 | No scroll indicator in chat | User misses content |
| 4.1 | Workflow halt doesn't reset | Session permanently blocked |
| 1.6 | Permission timeout mismatch | Confusing error message |
| 7.2 | No persistent permissions | Re-grant every session |

### P1 — High Value (major UX improvement)
| # | Issue | Impact |
|---|-------|--------|
| 3.1 | Bash no output streaming | Long commands are blind |
| 3.5 | Edit tool fragile matching | Silent edit failures |
| 2.9 | No keyboard shortcuts help | Unusable for new users |
| 6.1 | No fuzzy command matching | Typos break workflow |
| 1.8 | Doom detection too aggressive | False positives on permissions |

### P2 — Medium Value (quality of life)
| # | Issue | Impact |
|---|-------|--------|
| 3.3 | Read no encoding detection | Corrupt binary output |
| 4.2 | Foreach no progress | No feedback on long workflows |
| 8.1 | No per-project config | Inflexible for multi-project users |
| 2.3 | No complexity mode indicator | Confusing input state |
| 3.8 | Grep no context lines | Hard to understand matches |

### P3 — Strategic (competitive features)
| # | Issue | Impact |
|---|-------|--------|
| 10.6 | No MCP support | Can't integrate external tools |
| 10.4 | No semantic search | Missing core feature |
| 10.9 | No git integration | Manual git work required |
| 10.10 | No browser automation | Can't interact with web |
| 10.7 | No agent delegation | Can't parallelize work |

---

## Estimated Effort

| Category | Issues | Estimated Hours |
|----------|--------|-----------------|
| P0 Fixes | 5 | 8-12 hours |
| P1 Improvements | 5 | 15-20 hours |
| P2 Quality of Life | 5 | 10-15 hours |
| P3 Strategic Features | 5 | 40-60 hours |
| Code Refactoring | 3 | 20-30 hours |
| **Total** | **23** | **93-137 hours** |

---

*End of audit. This document covers 23 distinct issues across 10 categories, ranging from critical UX bugs to strategic feature gaps.*
