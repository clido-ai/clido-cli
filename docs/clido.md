# clido — Project conventions

## ⚠️ CRITICAL: CI MUST ALWAYS PASS

**The CI workflow is the gatekeeper. No exceptions. Failing CI blocks merging.**

### CI Requirements (MUST PASS 100%)

```bash
# 1. Formatting - ZERO TOLERANCE
cargo fmt --check
# Fix: cargo fmt --all

# 2. Clippy - ZERO WARNINGS
cargo clippy --workspace -- -D warnings
# Fix: Address every warning. No #[allow()] without justification.

# 3. Tests - ALL MUST PASS
cargo nextest run --workspace
# Fix: Fix the code, not the tests. Never modify tests to make them pass.

# 4. Release build - MUST COMPILE
cargo build --workspace --release
# Fix: Fix compilation errors.

# 5. Coverage - MIN 80% (configured in tarpaulin.toml)
cargo tarpaulin --workspace --config tarpaulin.toml --out Xml
# Fix: Add tests for uncovered code.
```

### Pre-Commit Checklist (MANDATORY)

Before every commit, run:

```bash
#!/bin/bash
set -e

echo "=== Formatting ==="
cargo fmt --all
cargo fmt --check

echo "=== Clippy ==="
cargo clippy --workspace -- -D warnings

echo "=== Tests ==="
cargo nextest run --workspace

echo "=== Release Build ==="
cargo build --workspace --release

echo "=== All checks passed ==="
```

**If any check fails, DO NOT COMMIT. Fix it first.**

## Project Rules for Agent

The agent automatically loads project rules from:
1. `.clido/rules.md` (checked first)
2. `CLIDO.md` (checked second)
3. `~/.config/clido/rules.md` (global fallback)

See `.clido/rules.md` for agent-specific instructions.

## Workspace

13 crates under `crates/`:

| Crate | Purpose |
|-------|---------|
| `clido-cli` | TUI, setup wizard, command registry, slash commands |
| `clido-agent` | Agent loop, planning, streaming, tool execution |
| `clido-tools` | Built-in tools (Bash, Read, Write, Edit, Glob, Grep, etc.) |
| `clido-providers` | LLM provider implementations + `PROVIDER_REGISTRY` |
| `clido-core` | Shared types, config loading, pricing |
| `clido-storage` | Session persistence, SQLite-backed memory |
| `clido-memory` | FTS5 semantic memory store |
| `clido-index` | Repo indexing |
| `clido-workflows` | YAML workflow engine (executor, loader, template) |
| `clido-planner` | Task planner |
| `clido-checkpoint` | Checkpoint/rollback |
| `clido-context` | Context window management, token estimation |
| `clido-harness` | Test harness utilities |

## Key conventions

- **`ModelProvider::list_models`** returns `Result<Vec<ModelEntry>, String>` — all test mocks must match this signature.
- **Provider registry** (`clido_providers::registry::PROVIDER_REGISTRY`) is the single source of truth for providers. Adding a new OpenAI-compatible provider only requires adding a `ProviderDef` entry.
- **Local/Ollama provider** is the last entry in the registry. Its index changes when new providers are added. Tests that reference it by index must be updated.
- **Credentials** are stored in a separate credentials file, NOT in `config.toml`. API keys must never appear in config.toml.
- **Paths**: use canonicalized paths. PathGuard blocks access outside workspace unless explicitly allowed via `/allow-path`.
- **Tests**: do not modify existing tests to make them pass — fix the code instead.

## Git conventions

- Branch naming: `feat/`, `fix/`, `docs/`, `chore/`, `refactor/`
- Release commits: `chore(release): v0.1.0-beta.N`
- **ALWAYS run the full pre-commit checklist before pushing**
- **NEVER push directly to master** — use PRs with passing CI

## Common CI Failures & Solutions

### `cargo fmt --check` fails
```bash
cargo fmt --all
git add -A && git commit --amend
```

### `cargo clippy --workspace -- -D warnings` fails
- Read every warning
- Fix the root cause
- No `#[allow()]` without team approval

### `cargo nextest run --workspace` fails
- Fix the code, not the test
- If test is genuinely wrong, discuss with team first

### `cargo build --workspace --release` fails
- Fix compilation errors
- Check for missing imports or type mismatches

### Coverage < 80% fails
- Add tests for uncovered code
- Check tarpaulin.toml exclusions are appropriate
- No bypassing coverage requirements

## Emergency Procedures

If CI fails on master:
1. **STOP** — do not push more commits
2. **Revert** the failing commit if needed
3. **Fix** the issue locally
4. **Verify** with full pre-commit checklist
5. **Push** the fix
6. **Verify** CI passes

**CI failures on master are P0 incidents. Treat them as such.**
