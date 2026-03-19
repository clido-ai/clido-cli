# Clido Documentation Index

This folder contains the design, planning, schema, testing, and research documentation for Clido.

## Start Here

- **[guides/implementation-bootstrap.md](./guides/implementation-bootstrap.md)** — Best entry point before writing code. Explains doc precedence, locked decisions, and build order.
- **[plans/cli-interface-specification.md](./plans/cli-interface-specification.md)** — Canonical user-facing CLI behavior.
- **[plans/releases/v1.md](./plans/releases/v1.md)** — Current implementation target and V1 release boundary.
- **[plans/development-plan.md](./plans/development-plan.md)** — Architecture and milestone roadmap.

## Guides

- **[guides/local-development-testing.md](./guides/local-development-testing.md)** — Safe local development workflow with fixtures, local models, and development flags.
- **[guides/testing-strategy-and-master-test-plan.md](./guides/testing-strategy-and-master-test-plan.md)** — Full testing strategy across unit, integration, e2e, performance, resilience, and security.
- **[guides/contributor-test-matrix.md](./guides/contributor-test-matrix.md)** — Contributor-facing test commands, required tools, and fast/slow lanes.
- **[guides/security-model.md](./guides/security-model.md)** — Security boundaries, permissions, path handling, secret redaction, and sandbox rules.
- **[guides/platform-support.md](./guides/platform-support.md)** — Platform support matrix and packaging expectations by release.
- **[guides/ci-and-release.md](./guides/ci-and-release.md)** — CI lanes, release validation, and packaging flow.
- **[guides/pricing-and-offline.md](./guides/pricing-and-offline.md)** — Pricing metadata, offline mode, and update behavior.
- **[guides/software-development-best-practices.md](./guides/software-development-best-practices.md)** — Project-wide engineering rules and documentation expectations.

## Plans

- **[plans/ux-requirements.md](./plans/ux-requirements.md)** — UX and copy standards: interactive prompts, script intros, visual design (functional and "hübsch"); first-run/init, permission, empty states; color use in setup flow.
- **[plans/releases/README.md](./plans/releases/README.md)** — Release overview from V1 to V4.
- **[plans/releases/v1.md](./plans/releases/v1.md)** — V1 scope and exit criteria.
- **[plans/releases/v1-5.md](./plans/releases/v1-5.md)** — V1.5 operator-quality scope.
- **[plans/releases/v2.md](./plans/releases/v2.md)** — V2 productization scope.
- **[plans/releases/v3.md](./plans/releases/v3.md)** — V3 advanced capability scope.
- **[plans/releases/v4.md](./plans/releases/v4.md)** — V4 planner scope.

## Schemas and References

- **[schemas/config.md](./schemas/config.md)** — `config.toml`, `.clido/config.toml`, and `pricing.toml`.
- **[schemas/session.md](./schemas/session.md)** — Session JSONL schema.
- **[schemas/output-and-session.md](./schemas/output-and-session.md)** — Output contracts, audit schemas, and versioning notes.
- **[schemas/types.md](./schemas/types.md)** — Shared type-level reference.

## Research Basis

- **[REPORT.md](./REPORT.md)** — Consolidated reverse-engineering report of Claude CLI and Cursor agent.
- **[ARTIFACTS.md](./ARTIFACTS.md)** — Extracted traces, tool data, and binary/bundle artifacts.

## Ideas

These are exploratory and **not binding** unless promoted into the roadmap/spec:

- **[ideas/multi-model-subagent-orchestration.md](./ideas/multi-model-subagent-orchestration.md)**
- **[ideas/self-improvement-loops.md](./ideas/self-improvement-loops.md)**
- **[ideas/skills-workflows-marketplace-and-agent-payments.md](./ideas/skills-workflows-marketplace-and-agent-payments.md)**
