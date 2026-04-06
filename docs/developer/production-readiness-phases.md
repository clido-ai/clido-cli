# Production readiness — phased implementation

This document tracks the **production-readiness initiative** (audit → code). Status is updated in-tree as work lands.

## Phase 0 — Stop silent persistence failures

- [x] Fail closed on `ContentBlock` → JSON for session lines (no `filter_map` drops).
- [x] Session writes from the agent loop use `write_line` + `ClidoError::SessionPersistence` (not `log_write_line`).
- [x] Failed turn rollback: `truncate_to` errors return `SessionPersistence` instead of `eprintln` only.
- [x] `try_session_lines_to_messages` — strict JSON decode on resume; `session_lines_to_messages` delegates with explicit error policy.

## Phase 1 — Agent loop clarity

- [x] Per-turn **`turn_correlation_id`** (UUID) refreshed at each outer user invocation / continue / resume entry.
- [x] `stream_aggregate` module — single place to fold `StreamEvent` → `ModelResponse`.
- [x] `invoke_model_completion` in `completion.rs` — throttle + streaming or batch call.

## Phase 2 — Streaming (opt-in)

- [x] `AgentConfig.stream_model_completion` + TOML `[agent] stream-model-completion` (default `false`).
- [x] When `true`, use `complete_stream` + aggregate; when `false`, `complete()`.

## Phase 3 — Tool hardening

- [x] `tool_timeout_secs` in `AgentConfig` (replaces hardcoded 60s).
- [x] `max_tool_output_bytes` — truncate tool text returned to the model with a clear suffix.

## Phase 4 — Session verify

- [x] `clido sessions verify <id>` — strict load via `try_session_lines_to_messages`.
- [x] Resume paths use `try_session_lines_to_messages` and surface errors.

## Phase 5 — TUI run state

- [x] `AppRunState` (`Idle` / `Generating` / `RunningTools`) on `App`, driven by `AgentEvent::RunState`, send paths (`Generating`), `TuiEmitter` tool start/done, and `on_agent_done` / resume (`Idle`).
- [x] Bounded `AgentEvent` channel (capacity 4096) — agent→TUI backpressure via `tokio::sync::mpsc::Sender`; async `.send().await` on hot path.

## Phase 6 — Observability

- [x] `TracingAgentMetrics` — `tracing::debug!` for metrics hook points.
- [x] `CLIDO_TRACE_METRICS=1` enables `TracingAgentMetrics` via `with_optional_trace_metrics` in `agent_setup.rs` for **run**, **TUI**, **REPL**, **`clido workflow`**, and **`clido commit`**.

## Runbook (operators)

- **Session persistence errors:** Check disk space and permissions on `.clido/sessions/`.
- **Verify session:** `clido sessions verify <id>` after suspected crash or manual edit.
- **Streaming:** enable `[agent] stream-model-completion = true` only after validating your provider (Anthropic/OpenAI paths aggregate `StreamEvent`).

## References

- Prior audit: internal reassessment (agent loop, tools, TUI, streaming).
- Interrupt semantics: `agent-loop-interrupt-semantics.md`.
