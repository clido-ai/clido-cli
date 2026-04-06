# Building & Testing

This page covers how to build clido from source, run the test suite, check code quality, and produce a release build.

## Prerequisites

### Rust toolchain

The required toolchain version is pinned in `rust-toolchain.toml` at the repository root. [rustup](https://rustup.rs/) will pick this up automatically:

```bash
# Install rustup if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# rustup reads rust-toolchain.toml on first use
rustc --version
```

### System dependencies

| Platform | Required | Purpose |
|----------|---------|---------|
| Linux | `pkg-config`, `libssl-dev` | TLS for HTTP clients |
| macOS | Xcode Command Line Tools | Linker and system headers |
| Both | `sqlite3` (usually pre-installed) | Memory and index storage |

On Ubuntu/Debian:

```bash
sudo apt install pkg-config libssl-dev
```

On macOS:

```bash
xcode-select --install
```

## Building

### Development build

```bash
cargo build --workspace
```

The `clido` binary is written to `target/debug/clido`.

### Release build

```bash
cargo build --workspace --release
# or
make release
```

The `clido` binary is written to `target/release/clido`. Release builds have LTO enabled and produce a smaller, faster binary.

### Installing locally

```bash
cargo install --path crates/clido-cli
```

This compiles in release mode and copies `clido` to `~/.cargo/bin/`.

## Running tests

### Full test suite

```bash
cargo test --workspace
```

### Single crate

```bash
cargo test -p clido-core
cargo test -p clido-agent
```

### Single test

```bash
cargo test -p clido-core -- config_file_exists_true_when_project_config_present
```

### Integration tests

Integration tests live in `tests/` at the workspace root:

```bash
cargo test --test '*'
```

Some integration tests require an API key:

```bash
ANTHROPIC_API_KEY=sk-ant-... cargo test --test integration
```

Integration tests that need a live API are marked `#[ignore]` and must be explicitly included:

```bash
ANTHROPIC_API_KEY=sk-ant-... cargo test --test integration -- --include-ignored
```

## Code quality

### Clippy

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

The CI pipeline uses `-D warnings` so all lint warnings are treated as errors. Fix warnings before opening a PR.

### Format

```bash
cargo fmt --all
```

Check only (no changes):

```bash
cargo fmt --all -- --check
```

### Both (pre-commit check)

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
```

## Test coverage

Coverage reports use [cargo-tarpaulin](https://github.com/xd009642/tarpaulin) with repository settings in `tarpaulin.toml` (excluded paths and **`fail-under = 70.0`** — the command exits with an error if line coverage drops below 70%).

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --workspace --config tarpaulin.toml --out Html --output-dir target/coverage
open target/coverage/tarpaulin-report.html
```

Quick summary to the terminal:

```bash
cargo tarpaulin --workspace --config tarpaulin.toml --out Stdout
```

::: tip
If tarpaulin fails on your platform, try [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) for a local HTML report (CI still uses tarpaulin with `tarpaulin.toml`).
:::

## Benchmarks

Performance-critical code in `clido-context` and `clido-index` has Criterion benchmarks:

```bash
cargo bench --workspace
```

Results are written to `target/criterion/`.

## CI pipeline

GitHub Actions runs the following on every pull request (see `.github/workflows/ci.yml`):

| Job | Command | Notes |
|-----|---------|--------|
| format | `cargo fmt --check` | Workspace root |
| clippy | `cargo clippy --workspace -- -D warnings` | |
| test | `cargo nextest run --workspace` | Parallel test runner |
| build (release) | `cargo build --workspace --release` | |
| coverage | `cargo tarpaulin --workspace --config tarpaulin.toml --out Xml` | Min **70%** line coverage via `tarpaulin.toml`; uploads to Codecov |

Some integration tests that need live APIs stay `#[ignore]` unless you opt in with `--include-ignored` and secrets.

## Makefile targets

The `Makefile` provides convenience targets:

```bash
make build        # cargo build --workspace
make release      # cargo build --workspace --release
make test         # cargo test --workspace
make lint         # fmt check + clippy
make fmt          # cargo fmt --all
make clean        # cargo clean
make install      # cargo install --path crates/clido-cli --force
```

## Feature flags

clido does not currently use Cargo feature flags. All functionality is compiled into the binary.

## Cross-compilation

Cross-compilation is not officially supported but is possible with `cross`:

```bash
cargo install cross
cross build --target aarch64-unknown-linux-gnu --release
```

::: warning
Cross-compiled binaries have not been tested in CI. If you need a specific target, please open an issue.
:::
