# Reverse-Engineering: CLI Coding Agents

This folder contains the output of a technical reverse-engineering investigation of modern AI CLI coding agents.

## Deliverable

- **[REPORT.md](./REPORT.md)** — Single consolidated report:
  - **Part A:** Discovery and binaries (Claude CLI, Cursor agent; paths, runtime, key flags).
  - **Part B:** Evidence-only reconstruction: execution traces, model request structure, tool schemas (Claude + Cursor), context assembly, repository navigation, code edit strategy, decision/planning logic, error recovery, prompt discovery, Cursor architecture (where the "brain" lives), implementation checklist.
  - **Part C:** Comparison and design takeaways (why Claude/Cursor are stronger; engineering principles for a new CLI agent).
  - All claims tied to traces, code, or docs; **UNCERTAIN** where evidence is missing.

## Purpose

To understand how the strongest CLI coding agents (Claude CLI, Cursor agent) work and to extract enough concrete technical detail that an engineer could implement a comparable CLI agent (coding tasks, repository auditing, file I/O, safe edits, shell execution, context management, session/resume).

## Methods

Local binary discovery, CLI inspection, session trace extraction (~/.claude/projects), Anthropic Agent SDK source, Cursor bundle analysis (agent-session, protos, local-exec, shell-exec), and documentation. Conclusions are evidence-based; uncertainties are marked explicitly.
