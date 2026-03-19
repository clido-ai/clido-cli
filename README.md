# cli;do

<p align="center">
  <img src="https://merbeth.io/files/clido.svg" width="420" height="140" alt="cli;do logo">
</p>

**cli;do** is a local-first, multi-provider CLI coding agent. Run it in your terminal, give it a task in plain language, and it uses AI (with tools like read, edit, search, and run) to get the job done—with permission prompts for anything that changes your files.

## Vision

- **CLI-first** — Built for the terminal; scripting and automation are first-class.
- **Multi-provider** — Use different AI backends (e.g. Anthropic, OpenAI) via profiles.
- **Safe by default** — Destructive or state-changing actions require your approval.
- **Session-aware** — Resume after interrupt; cost and usage visible when you care.

Planned capabilities include: core agent loop with tools, sessions, context and permissions (V1); JSON output and operator tooling (V1.5); multi-provider, sandboxing, packaging (V2); memory, MCP, semantic search, declarative workflows (V3); optional task-graph planner (V4).

## Build

**Prerequisites:** [Rust](https://rustup.rs) (install via `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`).

- `cargo build --release` — binary at `target/release/clido`
- `cargo run --release -- "your prompt"` — run with a one-off prompt
- `cargo run --release` — interactive (REPL when TTY)
- `cargo test --workspace` — run tests

See [Local development and testing](devdocs/guides/local-development-testing.md) for environment setup (API keys, config) and [Implementation bootstrap](devdocs/guides/implementation-bootstrap.md) for contributor workflow.

## Status

**V1 implementation:** Core agent loop, six tools, config with profiles, sessions with resume and stale-file detection, context compaction, permission modes and ExitPlanMode, `clido doctor` and `clido init`, interactive REPL (`clido` with no args at a TTY), first-run setup when no config exists, and hardening (retries, SIGINT, tests) are implemented. Scope and exit criteria: [V1 plan](devdocs/plans/releases/v1.md). Build and test: see **Build** above.

## Documentation

| Doc | Description |
| --- | --- |
| [Implementation bootstrap](devdocs/guides/implementation-bootstrap.md) | Where to start, canonical doc order, locked pre-build decisions |
| [Development plan](devdocs/plans/development-plan.md) | Architecture, Rust workspace, phased roadmap |
| [CLI interface spec](devdocs/plans/cli-interface-specification.md) | Canonical command surface and behavior |
| [UX requirements](devdocs/plans/ux-requirements.md) | Interactive copy, script intros, visual design (functional and clear) |
| [Releases](devdocs/plans/releases/README.md) | V1 → V4 scope and exit criteria |
| [Config reference](devdocs/schemas/config.md) | `config.toml`, `.clido/config.toml`, and `pricing.toml` schema |
| [Testing strategy](devdocs/guides/testing-strategy-and-master-test-plan.md) | Full validation strategy and test taxonomy |

## License

[MIT](LICENSE)
