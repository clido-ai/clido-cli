# Contributing to cli;do

Thank you for your interest in contributing. This document is the **implementation bootstrap**: it answers what to build first, which doc wins when they conflict, what V1 means operationally, which commands to run, and what is not implemented yet.

## First milestone

The first implementation milestone is **Phase 1.1 — Workspace Initialization** in [development-plan.md](devdocs/plans/development-plan.md):

1. Create the Rust workspace root `Cargo.toml` with members and `resolver = "2"`.
2. Create `.cargo/config.toml` (see [ci-and-release.md](devdocs/guides/ci-and-release.md) §4).
3. Add `rust-toolchain.toml` pinning a stable channel.
4. Create all crate skeletons under `crates/`.
5. Add `[workspace.dependencies]` with pinned versions for tokio, serde, etc.

Then proceed in order: Phase 1.2 (core types), 1.3 (tracing), Phase 2 (provider + Bash tool + PoC loop), and so on. Do not skip ahead; each phase has explicit dependencies.

## Which document wins on conflicts

- **User-facing behavior (CLI, flags, exit codes, output format):** The [CLI interface specification](devdocs/plans/cli-interface-specification.md) is the authority. If the development plan or any other doc disagrees with it, the CLI spec wins and the other doc should be updated.
- **Implementation sequence and milestones:** The [development plan](devdocs/plans/development-plan.md) is the authority. Implement in the order it specifies; each phase has clear dependencies.
- **If you find a contradiction:** Open an issue, fix the doc first (so the spec is consistent), then implement. Do not implement to one doc while another says something different.

## Where implementation starts

Start at **Phase 1** of the development plan: workspace init, core types, tracing. The first commands to run once the workspace exists:

```bash
cargo build --workspace
cargo test --workspace
cargo nextest run --workspace
```

Use the [local development testing](devdocs/guides/local-development-testing.md) guide to run and test the agent without risking your own repositories.

**Rust version:** This repo pins **Rust 1.94** in `rust-toolchain.toml`. Use **rustup only** (install from https://rustup.rs). Run `rustup update` so you have 1.94; then `cargo build` and `cargo clippy` in this repo use that version automatically. **Do not use Homebrew Rust** for this repo—it can mix with rustup and cause E0514. If you have both, never set `RUSTC` and put `~/.cargo/bin` first in `PATH`.


**Pre-commit hook (recommended):** To run the same checks as CI before each commit (`cargo fmt --check` and `cargo clippy --workspace -- -D warnings`):

```bash
git config core.hooksPath .githooks
```

The hook lives in `.githooks/pre-commit`. CI also runs tests; run `cargo test --workspace` (or `cargo nextest run --workspace`) before pushing.

**If you cannot commit** because the hook fails:
- **`cargo fmt --check` failed:** Run `cargo fmt --all`, then `git add -u` and try again.
- **`cargo clippy` failed with E0514 (incompatible rustc):** The repo pins Rust 1.94 (`rust-toolchain.toml`). Run `rustup update` so you have 1.94, then `export PATH="${HOME}/.cargo/bin:${PATH}"` and `unset RUSTC`, then `cargo clean` and `cargo clippy --workspace -- -D warnings`. Then commit again.

## What V1 means operationally

V1 is the **first shippable release** (version 0.1.0). Operationally it means:

- **One provider:** Only Anthropic is implemented. The CLI and config support `--profile`, `--provider`, `--model` and named profiles, but selecting a non-Anthropic profile (e.g. OpenRouter) returns a clear startup error, not a runtime panic.
- **Six tools:** Read, Write, Edit, Glob, Grep, Bash. No MCP, no SemanticSearch, no planner.
- **Doctor (basic):** `clido doctor` runs in V1 with checks for: config file parseable, API key set for active profile, session directory writable, `pricing.toml` present and not stale. Provider connectivity ping and MCP checks are V1.5.
- **Pricing:** A `pricing.toml` file exists (shipped default or user override); staleness warning if older than 90 days. Cumulative cost tracking in the session footer is V1.5.
- **No sandboxing, no audit log, no telemetry, no packaging:** Those are V2 or later.

The exact V1 boundary is defined in [releases/v1.md](devdocs/plans/releases/v1.md) and the roadmap table in [releases/README.md](devdocs/plans/releases/README.md). When in doubt, the CLI spec and release docs override the development plan for *what* ships in which release; the development plan is the authority for *how* to implement it.

**Definition of Done (DoD):** Each release has a machine-readable checklist in `devdocs/plans/releases/<release>-dod.yaml`. Run `scripts/verify-dod.sh` from the repo root to verify the active release (see `devdocs/plans/releases/CURRENT`). The script requires [yq](https://github.com/mikefarah/yq) (e.g. `brew install yq`). Regenerate human-readable `*-dod.md` with `scripts/generate-dod-md.sh [release]`.

## What is intentionally not implemented in V1

V1 ships a single working provider (Anthropic), the six core tools, sessions, config with named profiles, and basic doctor checks. The following are **not** in V1 and should not be assumed available:

- Multi-provider support (OpenAI, OpenRouter, Alibaba) — V2
- Subagents — V3
- Memory system — V3
- MCP support — V3
- Task graph / planner — V4
- Bash sandboxing — V2
- Telemetry and audit logging — V2
- Shell completion and man pages — V2
- Packaging and distribution — V2

See the [release plans](devdocs/plans/releases/README.md) for the full map of phases to releases.

## Key reference docs

- **UX and copy:** [devdocs/plans/ux-requirements.md](devdocs/plans/ux-requirements.md) — interactive prompts (first-run, init, permission), script intros, visual design and color; CLI must be functional and visually clear.
- **Config and pricing:** [devdocs/schemas/config.md](devdocs/schemas/config.md) — field-by-field reference for `config.toml`, `.clido/config.toml`, and `pricing.toml`.
- **Security:** [devdocs/guides/security-model.md](devdocs/guides/security-model.md) — workspace boundaries, path policy, secret redaction, permission model, sandbox behavior.
- **Session and output schemas:** [devdocs/schemas/output-and-session.md](devdocs/schemas/output-and-session.md) — session JSONL, `--output-format json` / `stream-json`, audit log, versioning.
- **Platform support:** [devdocs/guides/platform-support.md](devdocs/guides/platform-support.md) — supported platforms and packaging by release.
- **Running tests:** [devdocs/guides/contributor-test-matrix.md](devdocs/guides/contributor-test-matrix.md) — fast lane, optional tools, which commands run unit/integration/e2e/live-provider, feature flags and env vars.

## Resolving contradictions during implementation

If you discover that two docs conflict while implementing:

1. Prefer the CLI spec for any user-visible behavior.
2. Prefer the development plan for implementation order and internal design.
3. Update the losing doc in the same PR (or a follow-up) so the next contributor sees a single source of truth.
