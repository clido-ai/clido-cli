# Project Rules for clido

## Critical Rules

### CI MUST ALWAYS PASS

Before any code change, ensure:
- `cargo fmt --check` passes
- `cargo clippy --workspace -- -D warnings` passes  
- `cargo test --workspace` passes
- `cargo build --workspace --release` compiles

**Never commit if CI fails.**

### Code Quality

- **No warnings**: Clippy warnings are errors. Fix them, don't suppress them.
- **Formatting**: Run `cargo fmt` before committing.
- **Tests**: Add tests for new functionality. Don't modify existing tests to make them pass.

### Git Workflow

- **Branch naming**: Use `feat/`, `fix/`, `docs/`, `chore/`, `refactor/` prefixes
- **Commits**: Write clear commit messages explaining WHY, not just WHAT
- **No direct master pushes**: Use PRs with passing CI
- **Version bumps**: Use `chore(release): vX.Y.Z` format

### Common Mistakes to Avoid

1. **Don't forget to run `cargo fmt`** - CI will fail
2. **Don't ignore clippy warnings** - Use `-D warnings` locally
3. **Don't push broken tests** - Fix the code, not the test
4. **Don't hardcode API keys** - Use credentials file
5. **Don't bypass PathGuard** - Use `/allow-path` for external access

### Provider Registry

When adding new providers:
- Add to `crates/clido-providers/src/registry.rs`
- Update `PROVIDER_REGISTRY` with proper `ProviderDef`
- Ensure `list_models` returns correct signature
- Update tests that reference provider by index

### Testing

- Mock providers must match `ModelProvider` trait exactly
- Update provider index references when adding new providers
- Don't modify existing tests to make them pass - fix the code

## Architecture Notes

- **13 crates** in workspace under `crates/`
- **Credentials** stored separately from config (never in config.toml)
- **Paths** must be canonicalized before use
- **Sessions** stored in SQLite with FTS5 for semantic search
