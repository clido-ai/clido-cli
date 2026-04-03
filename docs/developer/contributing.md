# Contributing

Thank you for your interest in contributing to clido. This guide covers everything you need to get started: development setup, coding conventions, the PR process, and how the release cycle works.

## Code of conduct

Be respectful and constructive. Assume good faith. Disagreements about design are resolved by discussion and evidence, not authority. Harassment of any kind will not be tolerated.

## Development setup

### 1. Fork and clone

```bash
git clone https://github.com/clido-ai/clido-cli.git
cd clido-cli
```

### 2. Install the toolchain

The required Rust version is pinned in `rust-toolchain.toml`. rustup handles this automatically:

```bash
rustup show   # downloads the pinned toolchain if needed
```

### 3. Build and test

```bash
cargo build --workspace
cargo test --workspace
```

### 4. Verify lint and format

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Fix any issues before starting work. CI will reject PRs with lint warnings or format differences.

### 5. Set up your API key (for integration tests)

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

Integration tests are marked `#[ignore]` and require a live key. Run them with:

```bash
cargo test --test '*' -- --include-ignored
```

## Branch naming

| Type | Pattern | Example |
|------|---------|---------|
| Feature | `feature/<description>` | `feature/add-gemini-provider` |
| Bug fix | `fix/<description>` | `fix/tui-scroll-crash` |
| Documentation | `docs/<description>` | `docs/add-mcp-guide` |
| Refactor | `refactor/<description>` | `refactor/agent-loop-cleanup` |
| Release | `release/v<N>` | `release/v2` |

Branch off `master` for all contributions.

## Commit style

Use the [Conventional Commits](https://www.conventionalcommits.org/) format:

```
<type>(<scope>): <summary>

<optional body>

<optional footer>
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, `ci`

Scopes match crate names: `cli`, `agent`, `tools`, `providers`, `core`, `storage`, `context`, `memory`, `index`, `workflows`, `planner`

Examples:

```
feat(tools): add FetchUrl tool for HTTP GET requests
fix(tui): prevent scroll crash when chat history is empty
docs(guide): add MCP server configuration examples
refactor(agent): extract permission check into separate function
test(providers): add unit tests for Anthropic message serialisation
```

Keep the summary line under 72 characters. Write in the imperative mood ("add", "fix", "update" — not "added", "fixed", "updated").

## Pull request checklist

Before opening a PR, verify:

- [ ] `cargo fmt --all -- --check` passes (no format changes needed)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes (no warnings)
- [ ] `cargo test --workspace` passes (all unit tests green)
- [ ] New public functions and types have doc comments (`///`)
- [ ] New tools are registered in `default_registry()`
- [ ] New config keys are documented in `docs/reference/config.md`
- [ ] New CLI commands or flags are documented in `docs/reference/cli.md` and `docs/reference/flags.md`
- [ ] Breaking changes are noted in the PR description
- [ ] The PR description explains the motivation, not just what changed

### For new tools

- [ ] `Tool::schema()` has accurate JSON Schema with description fields
- [ ] Input validation returns `ToolOutput { is_error: true }` (not `Err(...)`)
- [ ] Unit tests cover the happy path and error cases
- [ ] Long outputs are truncated to a reasonable limit

### For new providers

- [ ] `ModelProvider` trait is fully implemented
- [ ] Error types map to `ClidoError::Provider`
- [ ] Token usage is correctly populated in `ModelResponse::usage`
- [ ] The provider is registered in `make_provider()`
- [ ] Pricing data is added for the provider's models

## Definition of Done (DoD)

Each release milestone has a Definition of Done document in `devdocs/`. A PR for a feature that is part of a release milestone must satisfy all DoD items for that feature before it can be merged.

Current release milestones:

| Release | Status | DoD |
|---------|--------|-----|
| V1 | Active | `devdocs/v1/dod.md` |
| V2 | Planned | `devdocs/v2/dod.md` |

## Release process

1. All features for the release milestone are merged to `master`
2. All DoD items are checked off
3. `CHANGELOG.md` is updated with the release notes
4. A release branch `release/vN` is cut from `master`
5. CI runs the full test suite including integration tests
6. The release is tagged `vN.M.P` and GitHub Release is created
7. The `master` branch is updated to the next development version

## Where to find the roadmap

The project roadmap is maintained in:

- `devdocs/` — Definition of Done documents for each release
- `CHANGELOG.md` — History of completed releases
- GitHub Issues — Bug reports and feature requests
- GitHub Milestones — Tracking for each release

## Getting help

- Open a GitHub Issue for bugs or feature requests
- Start a GitHub Discussion for design questions
- Check existing issues and discussions before opening a new one

When reporting a bug, include:
- `clido --version` output
- Operating system and version
- Steps to reproduce
- Expected vs actual behaviour
- Relevant section of `CLIDO_LOG=debug clido ...` output
