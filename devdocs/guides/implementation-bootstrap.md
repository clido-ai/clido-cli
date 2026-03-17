# Implementation bootstrap

This guide is the **starting point for building Clido**. It tells contributors which docs are authoritative, what the locked decisions are, and what order to implement the system in.

It is intentionally short. Read this first, then go to the referenced source documents for detail.

**Current V1 status:** The V1 completion plan has been implemented: resume/continue, config and pricing, context compaction, permissions and ExitPlanMode, doctor and init, tool fixes, shutdown and retries, tests, and docs. Scope and exit criteria: [v1.md](../plans/releases/v1.md).

---

## 1. Source-of-truth order

When documents overlap, use this order:

1. **User-facing behavior:** [`devdocs/plans/cli-interface-specification.md`](../plans/cli-interface-specification.md)
2. **Release scope and what must ship now:** [`devdocs/plans/releases/v1.md`](../plans/releases/v1.md) and [`devdocs/plans/releases/README.md`](../plans/releases/README.md)
3. **Architecture and milestone order:** [`devdocs/plans/development-plan.md`](../plans/development-plan.md)
4. **Validation and coverage expectations:** [`devdocs/guides/testing-strategy-and-master-test-plan.md`](testing-strategy-and-master-test-plan.md)
5. **Concrete schemas and references:** `devdocs/schemas/*.md`
6. **Forward-looking ideas:** `devdocs/ideas/*.md` are non-binding unless promoted into the roadmap/spec

If you discover a conflict:

- update the canonical doc first
- then update any derived docs in the same change
- do not silently implement "whatever seems right"

---

## 2. Locked decisions before coding

These are the canonical decisions to use unless the spec is changed explicitly:

- **Plan mode flag:** `--permission-mode plan`
- **Plan mode behavior:** `Read`, `Glob`, and `Grep` only; no `Bash`, `Write`, or `Edit` until `ExitPlanMode`
- **Budget flag:** `--max-budget-usd`
- **Config override env var:** `CLIDO_CONFIG`
- **User-facing log env var:** `CLIDO_LOG`
- **Doctor release boundary:** `clido doctor` exists in **V1** with basic checks; V1.5 expands it
- **Provider boundary:** `--profile`, `--model`, and `--provider` are part of the V1 CLI surface, but V1 only implements the single initial provider path; unsupported providers must fail fast with a helpful config/usage error

Reference docs:

- [`devdocs/plans/cli-interface-specification.md`](../plans/cli-interface-specification.md)
- [`devdocs/plans/releases/v1.md`](../plans/releases/v1.md)
- [`devdocs/schemas/config.md`](../schemas/config.md)

---

## 3. What to build first

Build in this order. Do not jump to V2+ features early.

1. **Workspace and shared types**
   - Create the Rust workspace and crate skeletons.
   - Implement `clido-core` types and errors first.

2. **Single provider path**
   - Implement one working provider end-to-end.
   - Ensure unsupported providers fail early and clearly.

3. **Six core tools**
   - `Read`, `Write`, `Edit`, `Glob`, `Grep`, `Bash`
   - Match the documented input/output and error formats exactly.

4. **Minimal agent loop**
   - User message → model → tool calls → tool results → repeat
   - No planner, memory, subagents, or MCP in V1

5. **CLI surface**
   - Implement the V1 command/flag surface from the CLI spec
   - Get exit codes, `--print`, sessions list/show, and non-interactive behavior right

6. **Session storage and resume**
   - JSONL session files
   - Resume flow and stale-file detection

7. **Context + permissions**
   - Context assembly and compaction
   - Permission modes, serialized `AskUser`, and `ExitPlanMode`

8. **V1 hardening**
   - Error handling
   - Graceful shutdown
   - Integration coverage
   - Basic `clido doctor`

---

## 4. What not to build yet

These are explicitly **not** part of the initial build target:

- Multi-provider support beyond the initial provider path
- Prompt caching
- Sandbox hardening
- Audit log and stats
- MCP
- Memory
- Subagents
- Repo indexing
- Planner / task graph

Those belong to later releases. V1 credibility comes from getting the core loop, permissions, sessions, and CLI behavior right.

Reference: [`devdocs/plans/releases/v1.md`](../plans/releases/v1.md)

---

## 5. Minimum docs to keep open while coding

For day-to-day implementation, keep these open:

- [`devdocs/plans/cli-interface-specification.md`](../plans/cli-interface-specification.md)
- [`devdocs/plans/releases/v1.md`](../plans/releases/v1.md)
- [`devdocs/plans/development-plan.md`](../plans/development-plan.md)
- [`devdocs/guides/contributor-test-matrix.md`](contributor-test-matrix.md)
- [`devdocs/schemas/config.md`](../schemas/config.md)
- [`devdocs/schemas/output-and-session.md`](../schemas/output-and-session.md)
- [`devdocs/guides/security-model.md`](security-model.md)

---

## 6. First local commands

Once the workspace exists, the default contributor loop should be:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```

If `cargo-nextest` is not installed:

```bash
cargo test --workspace
```

For implementation against fixtures and local models, use:

- [`devdocs/guides/local-development-testing.md`](local-development-testing.md)
- [`devdocs/guides/contributor-test-matrix.md`](contributor-test-matrix.md)

---

## 7. Definition of "ready to start coding"

You are ready to start coding when:

- the V1 release boundary is clear
- the conflicting CLI/config names are resolved
- the contributor knows which doc wins on conflict
- the first crate/milestone order is understood
- the validation path is known before code is written

If any of those are still unclear, fix the docs first.
