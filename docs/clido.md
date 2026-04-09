# clido — Project conventions

## CI checks (must pass before merging)

```
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

Run `cargo fmt --all` to fix formatting, then re-check.

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
- Always run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` before committing
