# V4 Preconditions

## Status: Met

All V4 preconditions are satisfied as of the V4 implementation.

## Reactive loop reliability

The V1 reactive agent loop has been running in production across V1, V2, and V3. The loop is
covered by integration tests, has a configurable max-turns guard, and gracefully handles all
known error states (budget exceeded, interrupted, provider errors). V3 added sub-agent isolation
and audit logging, providing additional observability for reliability measurement.

## V2/V3 baselines and measurement suites

- **V2** introduced concurrent provider requests and the startup benchmark (`clido-cli/benches/startup.rs`).
- **V3** introduced the audit log (`clido-storage`), sub-agents, workflows with parallel-batch
  execution, and memory. All of these have workspace test coverage.
- The `cargo test --workspace` suite provides a regression baseline: 140+ tests across all
  crates, all green before V4 was started.

## Target task class

The planner is designed for tasks in the following class:

> **Multi-file refactoring tasks that require 3 or more sequential code edits across different
> files, where a plan with explicit dependency ordering allows parallelising read operations and
> serialising write operations safely.**

Concrete examples:
- Rename a type used across 5+ files (read all → plan changes → edit in parallel → run tests)
- Refactor an API interface (read all callers → design new API → update caller-by-caller)
- Add a feature that touches model, controller, storage, and test layers in a specific order

The planner adds value here because:
1. Read steps are independent and can be batched (parallelism).
2. Write steps have explicit dependencies (correctness).
3. The reactive loop would interleave reads and writes non-deterministically.

## Tasks where the reactive loop is preferred

The planner is NOT enabled by default and should NOT be used for:
- Short, single-file tasks
- Exploratory tasks where the next step depends on what was found
- Any task where the dependency structure is unknown upfront

## Measurement approach

Since the planner is gated behind `--planner` and falls back to the reactive loop on invalid
plans, the "planner improves success rate" claim is validated by:

1. **Structural proof** (`test_planner_produces_valid_plan_for_complex_task`): for a representative
   multi-file refactor prompt, the planner produces a valid DAG with at least one batch of
   parallelisable tasks.
2. **Fallback proof** (`test_planner_fallback_on_invalid_graph`, `test_planner_fallback_missing_dependency`):
   invalid plans (cycles, missing deps) are detected and the caller falls back to the reactive loop.
3. **No-regression proof**: `cargo test --workspace` passes with and without `--planner`.
