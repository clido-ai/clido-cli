# Production-grade agent loop — implementation plan

This document is the **full-scope** engineering plan to harden clido’s agent loop (`crido-agent` / `AgentLoop`) for production: predictable failure semantics, validated tool execution, bounded retries, integrity of history and session logs, and test coverage. **There is no MVP slice:** every section below is **in scope** for completion of this initiative.

**Primary code today:** `crates/clido-agent/src/agent_loop/mod.rs` and related modules (`context.rs`, `security.rs`, …), tool registry in `clido-tools`, providers in `clido-providers`, TUI/repl emitters in `clido-cli`.

---

## 1. Objectives and success criteria

### 1.1 Objectives

1. **No silent failure:** Every abnormal path returns a classified error or an explicit tool/user message; history and session never imply success when execution failed.
2. **Validated tool boundary:** No tool `execute()` runs without **schema validation** and **policy checks** (workspace, permissions already partially exist).
3. **Deterministic control flow:** Caps on turns, wall time, tool calls per user turn, and cost; stall detection beyond `max_turns`.
4. **Typed errors end-to-end:** Replace substring-only retry classification with a **stable error taxonomy** propagated from tools, I/O, and HTTP layers.
5. **Streaming integrity:** Partial assistant output and tool streams cannot corrupt committed history; interrupt semantics are documented and tested.
6. **Observable:** Structured logging and metrics hooks for failures, retries, stalls, validation rejects, and budget events.
7. **Verifiable:** Automated tests for validation, retries, loop guards, rollback, streaming finalization, and representative integration scenarios.

### 1.2 Definition of done (initiative)

- All sections **2–16** implemented, reviewed, and **green** in CI (`cargo test` for affected crates, new integration tests included).
- Public or developer-facing documentation updated (`architecture.md`, this plan marked **completed** with links to ADRs if any).
- No known **P0/P1** gaps against the failure-mode matrix in section 17.

---

## 2. Architecture: explicit phase model

### 2.1 Deliverable

Introduce an internal **turn state machine** (enum + small struct), not necessarily user-visible, that `completion_loop_run` follows each iteration:

| Phase | Responsibility |
|-------|----------------|
| `Guard` | `cancel`, `max_turns`, per-turn wall clock, per-turn tool-call budget, session budget, stall score |
| `AssembleContext` | compaction, proactive summarize, memory/git injection; produce immutable `ContextSnapshot` (id + token estimate) |
| `ModelCall` | `provider.complete` with tool schemas |
| `PostModelGuard` | cancel after return; usage accounting; budget warnings |
| `ParseAssistant` | normalize provider response into `AssistantTurn` (see §5) |
| `ValidateToolCalls` | schema + registry + unknown tool handling |
| `ExecuteTools` | permission gating, hooks, timeout, batching policy |
| `Observe` | build `ToolResult` blocks, doom/stall updates, consecutive error nudges |
| `Commit` | append to `history`, session JSONL, audit; invariants checked |
| `Branch` | continue loop vs return `Ok` vs return `Err` |

### 2.2 Acceptance criteria

- Refactor preserves existing external behavior for **happy path** (golden session replay tests pass).
- Each phase is a **named function** or `impl` method with **single responsibility** (no 400-line inline loop body).
- **Invariant:** assistant message is only committed after successful parse; tool results are only committed after all tools in the batch complete (current behavior preserved unless ADR changes it).

### 2.3 Files

- `crates/clido-agent/src/agent_loop/mod.rs` (split into submodules if needed: `turn.rs`, `execute.rs`, `guard.rs`).
- New: `crates/clido-agent/src/agent_loop/phases.rs` (or equivalent).

---

## 3. Assistant turn model and parsing

### 3.1 Deliverable

Define `AssistantTurn`:

```text
enum AssistantTurn {
  FinalText { text: String, stop: StopKind },
  ToolCalls { text_prefix: Option<String>, calls: Vec<RawToolCall> },
}
```

- `RawToolCall`: `{ id, name, input: serde_json::Value }` from provider blocks.
- **Malformed provider output** (empty tool batch when `StopReason::ToolUse`, duplicate ids, missing id) → **recoverable error path**: do not append broken assistant message; return `ClidoError::MalformedModelOutput { detail }` or inject a single synthetic user message explaining the failure (choose one strategy document-wide and test it).

### 3.2 Acceptance criteria

- Unit tests for: `EndTurn` with only text; `ToolUse` with text + tools; `ToolUse` with zero tools (error); duplicate `tool_use_id` (error).
- Session log writes **only** valid assistant lines.

### 3.3 Files

- `crates/clido-agent/src/agent_loop/parse.rs` (new).
- `crates/clido-core` if shared types must be visible to providers (minimize; prefer agent-local types).

---

## 4. Tool contracts: JSON Schema validation layer

### 4.1 Deliverable

1. **Every** registered tool exposes a **JSON Schema** for its input (already partially true via tool definitions; enforce completeness).
2. Before `execute_tool` / batch execution:

   `validate_tool_input(name: &str, input: &Value) -> Result<(), ValidationError>`

   - Unknown tool → `ValidationError::UnknownTool` (same outcome as today but **before** timeout wrapper).
   - Schema mismatch → `ValidationError::Schema { path, expected, got }` with stable machine-readable payload.

3. **Normalization:** optional per-tool `normalize_input` hook (e.g. trim strings, default fields) runs **after** schema validation if needed (document order: validate canonical shape vs validate post-normalize).

### 4.2 Acceptance criteria

- 100% of first-party tools in `clido-tools` have non-empty, CI-checked schemas (test that walks registry).
- Invalid args never invoke `tool.execute` (verified by mock tool that panics if called with invalid data).
- Validation errors appear in `ToolResult` content in a **consistent format** so the model can self-correct (template string versioned, e.g. `[validation_error] v1 ...`).

### 4.3 Files

- `crates/clido-tools`: ensure schema on each `Tool` trait implementation or central registry map.
- New: `crates/clido-agent/src/agent_loop/validation.rs` using `jsonschema` crate (or equivalent already in tree — add if missing).
- `Cargo.toml` dependencies as needed.

---

## 5. Error taxonomy (typed, end-to-end)

### 5.1 Deliverable

Define `AgentErrorKind` (or extend `ClidoError`) with variants, including:

- `Transport { source }` (network, DNS, TLS)
- `RateLimit { retry_after }`
- `Timeout { operation }` (model vs tool vs permission prompt)
- `Validation { tool, detail }`
- `PermissionDenied { tool }`
- `ToolLogical { tool, message }` (non-retryable)
- `ToolNotFound { name }`
- `BudgetExceeded`
- `MaxTurnsExceeded`
- `MaxWallTimeExceeded`
- `MaxToolCallsPerTurnExceeded`
- `StallDetected { reason }`
- `DoomLoop { tool, fingerprint }`
- `Interrupted`
- `MalformedModelOutput { detail }`

**Tools and providers** must classify failures into this kind at the point of origin where possible:

- New: `ToolOutput` extension or parallel `ToolError { kind, message }` — **full migration** (no half-string half-enum).

### 5.2 Retry mapping table (code + doc)

Central function:

`fn retry_policy(kind: AgentErrorKind, tool_name: &str, attempt: u32) -> RetryDecision`

- `RetryDecision`: `NoRetry | Retry { delay, max_attempts_for_this_call }`
- **Remove** duplicate logic: delete parallel substring-only classification once table is authoritative.

### 5.3 Acceptance criteria

- Every `ToolOutput::err` construction in `clido-tools` is audited and assigns a **kind**.
- HTTP client errors in providers map to `Transport` / `RateLimit` with parsed `retry-after` when present.
- Unit tests per kind for retry policy.

### 5.4 Files

- `crates/clido-core/src/error.rs` (or wherever `ClidoError` lives)
- `crates/clido-agent/src/agent_loop/retry.rs` (new)
- All tool implementations under `crates/clido-tools`

---

## 6. Retry and backoff (production rules)

### 6.1 Deliverable

1. **Per-tool-call** retry counter (already exists) driven by **typed** policy, not string match.
2. **Exponential backoff with full jitter** for `Transport` and `RateLimit` (caps documented: max delay, max attempts per call).
3. **No retry** for `Validation`, `PermissionDenied`, `ToolLogical`, `ToolNotFound`.
4. **Idempotency:** document that retries for read-only tools are safe; for any future idempotent writes, require explicit tool flag before allowing retry.
5. **Global** rate-limit coordination (optional word forbidden — **required**): per-provider semaphore or token bucket in the agent or provider layer to avoid thundering herd across parallel sessions (minimum: single-process coordinator with `Mutex` + last-request time).

### 6.2 Acceptance criteria

- Property-style tests: backoff delays increase within bounds; jitter in range.
- Integration test with mock provider returning 429 then success.

### 6.3 Files

- `crates/clido-agent/src/agent_loop/retry.rs`
- `crates/clido-providers` (HTTP layer)

---

## 7. Loop guardrails (beyond max_turns)

### 7.1 Deliverable

Per **user turn** (one `run` / `run_next_turn` entry), track:

| Limit | Default source | Behavior on exceed |
|-------|----------------|-------------------|
| `max_turns` | existing `AgentConfig` | `ClidoError::MaxTurnsExceeded` |
| `max_wall_time_per_turn` | **new** `AgentConfig` field | `ClidoError::MaxWallTimeExceeded` |
| `max_tool_calls_per_turn` | **new** `AgentConfig` field | `ClidoError::MaxToolCallsPerTurnExceeded` |
| Stall score | **new** heuristic | `ClidoError::StallDetected` |

**Stall heuristic (v1, fully specified):**

- Maintain `stall_score` for the turn: increment when **all** of the following hold: (a) at least one tool call in the iteration, (b) **no** tool returned `is_error == false` with non-empty “progress signal” **or** no successful read of workspace path (define progress signal table per tool: e.g. `Write`/`Edit` success counts; `Read` success with changed `content_hash` vs previous read of same path counts).
- Increment +2 if **same** `(tool_name, normalized_args)` repeats compared to previous iteration.
- When `stall_score >= STALL_THRESHOLD` (config, default e.g. 6), fail the turn.

Document limitations; tune defaults from dogfooding — **still ship** with tests using mocked tool outputs.

### 7.2 Acceptance criteria

- Unit tests for stall scoring with scripted tool result sequences.
- Config validates positive integers and sensible upper bounds at startup.

### 7.3 Files

- `crates/clido-core` config structs + deserialization
- `crates/clido-agent/src/agent_loop/stall.rs` (new)
- `docs/reference/configuration.md` (new fields documented)

---

## 8. Doom loop detection v2

### 8.1 Deliverable

Replace single-key `tool + first 120 chars of error` with:

1. **Normalized error fingerprint:** lowercase, strip digits/uuids/path noise (regex table), collapse whitespace.
2. **Track last N** `(tool_name, fingerprint, normalized_args_hash)` entries.
3. Trigger `DoomLoop` when **K** consecutive entries match on `(tool_name, fingerprint)` **or** **M** matches on `(tool_name, normalized_args_hash)` within **W** window.

Constants `K`, `M`, `W` in config with safe defaults.

### 8.2 Acceptance criteria

- Tests: slightly different error text still triggers doom when fingerprint matches.
- Tests: same tool different args does not false-positive on fingerprint path.

### 8.3 Files

- `crates/clido-agent/src/agent_loop/doom.rs` (new)

---

## 9. Parallel tool execution policy

### 9.1 Deliverable

1. **Audit** every tool’s `is_read_only()` for correctness (spreadsheet in repo: tool name → proof or test).
2. **Forbidden:** parallel execution if **any** tool in batch is missing from registry (today `unwrap_or(false)` — **change** to sequential safe path or fail validation).
3. **Logging:** when parallel batch runs, debug log tool names + ids.

### 9.2 Acceptance criteria

- CI test: registry completeness — every tool name in `ToolRegistry` has metadata row in audit file.
- Regression test for “unknown tool in batch” behavior.

### 9.3 Files

- `crates/clido-tools` (audit + fixes)
- `crates/clido-agent/src/agent_loop/mod.rs` (batch branch)

---

## 10. Streaming and interrupt semantics

### 10.1 Deliverable

1. **Document** precisely: what is committed to `history` when user cancels during (a) streaming text, (b) after complete before tools, (c) during tools, (d) after tools.
2. **Implement** consistency:

   - If provider exposes streaming API: buffer until a **complete** assistant message is assembled **or** cancel: then either **discard** partial assistant block (no history append) or append with `[cancelled]` marker — **pick one** and test.
   - Tool `on_tool_start` / `on_tool_done` events must not leave UI implying a tool finished if it was aborted mid-flight (align `EventEmitter` contract).

3. **TUI/repl:** ensure interrupt does not strand spinner state; verify `event_loop.rs` and emitters.

### 10.2 Acceptance criteria

- Integration tests with mock streaming provider and cancel at defined points.
- Manual test checklist in `docs/developer/agent-loop-manual-qa.md` (new) — filled as part of sign-off.

### 10.3 Files

- `crates/clido-providers` (streaming adapter)
- `crates/clido-cli/src/tui/event_loop.rs`, emitter traits
- `docs/developer/agent-loop-manual-qa.md`

---

## 11. State integrity and session atomicity

### 11.1 Deliverable

1. **Invariant checks** (debug assertions + release logs on violation):

   - Every `ToolUse` id in the last assistant message has matching `ToolResult` in the following user message before next assistant.
   - History length monotonicity per iteration.

2. **SessionWriter:** define transactional append: either **both** assistant line and tool lines write, or rollback to checkpoint (extend beyond current “failed run” rollback to cover mid-turn crash simulation where feasible).

3. **Id generation:** ensure `tool_use_id` uniqueness per session (provider-generated; if not guaranteed, agent wraps with session-scoped prefix).

### 11.2 Acceptance criteria

- Tests that inject failure between session writes and assert file state matches spec.
- `apply_failed_turn_rollback` covered for **new** error kinds.

### 11.3 Files

- `crates/clido-storage` / session writer module
- `crates/clido-agent/src/agent_loop/mod.rs`

---

## 12. Security and permissions (hardening)

### 12.1 Deliverable

1. Centralize path extraction for rules and external path checks — **one** module, used everywhere.
2. **Symlink / traversal:** confirm workspace checks resolve paths consistently (tests on `../` and symlink escapes).
3. Prompt injection detection: keep heuristics; add **tests** for false positives/negatives; log structured `injection_score` if applicable.

### 12.2 Acceptance criteria

- Security-focused integration tests in `clido-agent` or `clido-tools`.

### 12.3 Files

- `crates/clido-agent/src/agent_loop/security.rs`
- `crates/clido-tools` workspace validation

---

## 13. Observability

### 13.1 Deliverable

1. Structured logging (`tracing` fields): `turn`, `phase`, `tool_name`, `tool_use_id`, `error_kind`, `retry_attempt`, `latency_ms`, `session_id` (if available).
2. **Metrics trait** (optional word forbidden — **required**): `AgentMetrics` with counters (implemented as no-op in OSS, hookable for enterprise build if needed):

   - `model_calls_total`, `tool_calls_total`, `tool_failures_total{kind}`, `retries_total`, `validation_failures_total`, `stall_detected_total`, `doom_loop_total`, `budget_hits_total`.

3. Wire metrics calls at phase boundaries.

### 13.2 Acceptance criteria

- Log snapshot tests or integration test that asserts key events fire (using `tracing_subscriber` test layer).

### 13.3 Files

- New: `crates/clido-agent/src/agent_loop/metrics.rs`

---

## 14. Configuration surface

### 14.1 Deliverable

New `AgentConfig` / config file keys (names illustrative, finalize in implementation):

- `agent.max_wall_time_per_turn_sec` (default: e.g. 900)
- `agent.max_tool_calls_per_turn` (default: e.g. 200)
- `agent.stall_threshold` (default: e.g. 6)
- `doom.consecutive_same_error` (K)
- `doom.same_args_window` (M, W)
- `retry.max_attempts` (unify with existing `max_tool_retries` or deprecate one name with migration)
- `retry.backoff_max_ms`, `retry.jitter_ratio`

Validate on load; **fail fast** on invalid combinations.

### 14.2 Acceptance criteria

- Config tests + documentation in `docs/reference/configuration.md`.

---

## 15. Testing strategy (full)

### 15.1 Unit tests (required)

- Validation: schema pass/fail per tool fixture.
- Parse assistant turn: all branches.
- Retry policy: matrix per `AgentErrorKind`.
- Stall and doom modules: pure functions with fixed inputs.
- Normalization fingerprinting.

### 15.2 Integration tests (required)

- Mock `ModelProvider` returning scripted sequences: tool use → results → final text.
- Failure injection: tool timeout, 429, validation error, doom sequence, stall sequence.
- Session file integrity after rollback.
- Parallel vs sequential batch selection.

### 15.3 Golden / replay tests (required)

- Capture **anonymized** session JSONL snippets as test vectors; replay through parser/validator only (no live API).

### 15.4 CI (required)

- `cargo clippy -D warnings` on touched crates.
- `cargo test -p clido-agent -p clido-tools -p clido-core -p clido-providers` (adjust scope to all affected).

---

## 16. Documentation and migration

### 16.1 Deliverable

1. Update `docs/developer/architecture.md` with phase diagram and error taxonomy.
2. ADR (short): “Assistant turn parse failures — discard vs synthetic user message.”
3. Changelog / release notes: new config keys; any behavior change on interrupt/streaming.
4. Migration: if `max_tool_retries` renamed or semantics change, support **one release** of backward-compatible alias reading old key.

### 16.2 Acceptance criteria

- Docs PR merged same release as code; no orphan config keys.

---

## 17. Failure-mode matrix (verification checklist)

Every row must have **automated** coverage or a **documented manual QA** step with owner sign-off.

| # | Scenario | Detection | Mitigation implemented | Test id / doc |
|---|----------|-----------|------------------------|---------------|
| 1 | Invalid tool JSON vs schema | `ValidationError` | Reject pre-execute | §15 |
| 2 | Unknown tool | `ToolNotFound` | Error result | existing + §15 |
| 3 | Provider 429 | `RateLimit` | Backoff + metrics | §15 |
| 4 | Provider timeout | `Timeout` | Classify + retry policy | §15 |
| 5 | Tool hang | `Timeout` | 60s or per-tool | existing + §15 |
| 6 | Permission deny | `PermissionDenied` | No retry | §15 |
| 7 | Doom repetition | `DoomLoop` | v2 detector | §8 |
| 8 | No progress | `StallDetected` | stall scorer | §7 |
| 9 | Turn too long | `MaxWallTimeExceeded` | wall clock | §7 |
| 10 | Too many tools | `MaxToolCallsPerTurnExceeded` | counter | §7 |
| 11 | Malformed model output | `MalformedModelOutput` | parse phase | §3 |
| 12 | Cancel mid-flight | `Interrupted` | streaming policy | §10 |
| 13 | Budget | `BudgetExceeded` | existing + metrics | §13 |
| 14 | Session partial write | invariant + rollback | transactional rules | §11 |
| 15 | Parallel unsafe batch | validation of registry | sequential fallback | §9 |

---

## 18. Work sequencing (recommended order)

1. **§5 Error taxonomy** + tool/provider propagation (foundation).
2. **§4 Validation layer** (depends on stable error types).
3. **§6 Retry** rewrite to use taxonomy.
4. **§3 Assistant turn parse** + **§2 Phase refactor** (large refactor; do after errors validate cleanly).
5. **§8 Doom v2**, **§7 Stall + wall time + tool call cap**.
6. **§9 Parallel audit**.
7. **§10 Streaming/interrupt**.
8. **§11 Session atomicity**.
9. **§12 Security tests** hardening.
10. **§13 Metrics**, **§14 Config**, **§15–17** complete coverage and matrix closure.
11. **§16 Docs** and release.

---

## 19. Estimate and staffing (planning only)

- **Engineering:** roughly **4–8 weeks** for one senior Rust engineer familiar with the codebase, **parallelizable** after §5–§4 land (second engineer on streaming/TUI and integration tests).
- **Risk:** provider API differences for strict JSON / streaming — allocate buffer in schedule.

---

*End of plan. Mark this document with a completion date and link to the merge commit when the initiative ships.*
