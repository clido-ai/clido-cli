# Clido Release Plans

This directory translates the main `development-plan.md` into product-style release definitions with clear scope, rationale, and exit criteria.

## The Release Sequence

| Release | One-line Purpose |
|---------|-----------------|
| V1 | A real, usable agent on a single machine with one provider |
| V1.5 | Safe to automate, cheaper to run, easier to operate |
| V2 | Production-grade: multi-provider, packaged, benchmarked, documented |
| V3 | Advanced capabilities: subagents, memory, MCP, indexing, declarative workflows |
| V4 | Experimental: task graph planner for specific hard workflows |

Each release builds on the previous. Later releases are not planned in detail until earlier ones are shipped and measured.

## Files

- **`CURRENT`** — Current release in progress. Single line (e.g. `v1`). All DoD and fix-loop behavior key off this file. Update when switching focus to a new release.
- **`<release>-dod.yaml`** — Definition of Done for a release (machine-readable). Each item has `id`, `description`, `source` (traceability), `verification` (command / cli / coverage / test), and `status` (DONE or GAP; if GAP, `gap_reason` required).
- **`<release>-dod.md`** — Human-readable DoD; generated from `*-dod.yaml` by `scripts/generate-dod-md.sh`. Do not edit by hand.
- `v1.md` — Core agent loop, six tools, sessions, context, permissions.
- `v1-5.md` — Operator quality: cost tracking, parallelism, secret safety, machine-readable output.
- `v2.md` — Product readiness: multi-provider, sandboxing, telemetry, packaging.
- `v3.md` — Advanced platform: subagents, memory, MCP, repository indexing, declarative workflows.
- `v4.md` — Planner and experimental orchestration for complex task types.

## Definition of Done (DoD)

Each release that is in scope for implementation has a companion DoD:

- **Canonical:** `devdocs/plans/releases/<release>-dod.yaml`. Every item is verifiable (run a command, run `clido` with args, or run coverage). Every item has `source` for traceability to the release plan, development-plan, or **CLI spec**.
- **Derivation rule:** DoD items are **derived from** the CLI spec and the release plan. Every in-scope requirement in those documents must have at least one DoD item that verifies the *behavior* (not only "command exists"). Nothing in scope may ship without a corresponding DoD item (or an explicit GAP with reason). UX requirements ([ux-requirements.md](../ux-requirements.md)) are reflected in DoD items where they affect interactive flows (e.g. first-run/init copy, script intros).
- **Human-readable:** `devdocs/plans/releases/<release>-dod.md`, generated from the YAML by `scripts/generate-dod-md.sh`.
- **Verification:** Run `scripts/verify-dod.sh` from the repo root. It reads `CURRENT`, loads the corresponding `*-dod.yaml`, runs each verification, and exits 0 only if all pass. CI should run this for release validation. The script requires **yq v4+** (e.g. `brew install yq` on macOS).
- **Adding DoD for a new release:** Copy the structure from `v1-dod.yaml`. For each in-scope section of the CLI spec and the release plan, add one or more verifiable items (command / cli / coverage) with `source` pointing to that section. Set all `status` to DONE or GAP with `gap_reason`. Then run `scripts/generate-dod-md.sh <release>` and set `CURRENT` to that release when switching focus.

- **Detailed DoDs:** Each release has a *detailed* DoD: every exit criterion, in-scope phase, and CLI surface item is expanded into one or more verifiable items with traceability (`source`) to the release plan or development-plan. V1 has 35+ items; V1.5, V2, V3, and V4 each have 20–40+ items. The human-readable `*-dod.md` files are generated from the YAML (or kept in sync manually).

## Roadmap Coverage

Every phase from `development-plan.md` is assigned to exactly one release:

| Roadmap Phase | Release |
|---------------|---------|
| Phase 1 — Foundation | V1 |
| Phase 2 — Proof of Concept | V1 |
| Phase 3 — Minimal Viable Agent | V1 |
| Phase 4.2 — Context Engine | V1 |
| Phase 4.3 — Permission System | V1 |
| Phase 4.5 — Plan Mode | V1 |
| Phase 5.1 — Robust Error Handling | V1 |
| Phase 5.2 — Session Recovery | V1 |
| Phase 5.3 — Graceful Shutdown | V1 |
| Phase 5.4 — Integration Test Suite | V1 |
| Phase 5.6 — Edit Safety and Partial Write Detection | V1 |
| Phase 8.4 (basic) — `clido doctor` (API key, session dir, pricing.toml) | V1 |
| Phase 4.4 — Cost Tracking | V1.5 |
| Phase 4.6 — Parallel Tool Execution | V1.5 |
| Phase 6.2 — Context Efficiency | V1.5 |
| Phase 7.2 — Secret Detection | V1.5 |
| Phase 8.2 — JSON and Stream-JSON Output | V1.5 |
| Phase 8.4 (expanded) — `clido doctor` (MCP, connectivity ping) | V1.5 |
| Phase 4.1 — Multi-Provider Support | V2 |
| Phase 4.2.4 — Prompt Caching | V2 |
| Phase 6.1 — Startup Performance | V2 |
| Phase 6.3 — Concurrent Provider Requests | V2 |
| Phase 6.4 — File Read LRU Cache | V2 |
| Phase 7.1 — Bash Sandboxing | V2 |
| Phase 7.3 — Audit Logging | V2 |
| Phase 8.1 — Hooks System | V2 |
| Phase 8.5 — Shell Completion and Man Pages | V2 |
| Phase 8.6 — Live Plan / Progress Visualization | V2 |
| Phase 9.1 — Full Test Coverage | V2 |
| Phase 9.2 — Benchmarks | V2 |
| Phase 9.3 — Structured Telemetry | V2 |
| Phase 9.4 — Documentation | V2 |
| Phase 9.5 — Packaging and Distribution | V2 |
| Phase 9.6 — Production Hardening | V2 |
| Phase 4.7 — Subagent Architecture | V3 |
| Phase 5.5 — Memory System | V3 |
| Phase 8.3 — MCP Support | V3 |
| Phase 8.7 — Repository Indexing | V3 |
| Phase 4.9 — Workflow Engine | V3 |
| Phase 4.9.x — Pre-Flight und dynamische Parameter | V3 |
| Phase 4.8 — Task Graph / Planner | V4 |

## Planning Principles

- The reactive agent loop is always the default execution model.
- Delay complexity that does not clearly improve task success.
- Prefer measurable quality improvements over architectural ambition.
- Optional systems are added only after the core loop is proven.
- Every V3 and V4 feature must define its success metric before implementation begins.

## Source of Truth

These release plans are derived from `devdocs/plans/development-plan.md`.
If there is a conflict between the two, update both documents together so the milestone roadmap and release roadmap stay aligned.

See also: `devdocs/guides/testing-strategy-and-master-test-plan.md` for the testing priorities that correspond to each release.
