---
layout: home

hero:
  name: clido
  text: An AI coding agent that lives in your terminal.
  tagline: Powered by Claude and compatible models. Persistent sessions, full tool access, and a polished TUI — all from a single Rust binary.
  actions:
    - theme: brand
      text: Get Started
      link: /guide/introduction
    - theme: alt
      text: Quick Start
      link: /guide/quick-start
    - theme: alt
      text: GitHub
      link: https://github.com/clido-ai/clido-cli

features:
  - icon: 🖥️
    title: Interactive TUI
    details: A full terminal UI built with Ratatui. Real-time tool progress, session picker, slash commands, permission prompts, and a cost/token status strip.

  - icon: 🧠
    title: Session Memory
    details: Every session is persisted as JSONL. Resume any past conversation with --continue or --resume, and let long-term SQLite memory inject context automatically.

  - icon: 🔌
    title: Multi-Provider
    details: Supports Anthropic (Claude), OpenAI-compatible endpoints, OpenRouter, and local models via Ollama. Switch at runtime with --provider and --model.

  - icon: ⚙️
    title: Declarative Workflows
    details: Define multi-step agent pipelines in YAML. Dynamic parameters, parallel steps, retry policies, and pre-flight checks — no code required.

  - icon: 🛠️
    title: MCP Servers
    details: Connect any Model Context Protocol tool server over stdio. External tools appear natively to the agent alongside built-in Bash, Read, Write, Grep, and Glob.

  - icon: 📦
    title: Open Source
    details: Written in Rust. A clean multi-crate workspace designed to be extended. Add new tools, providers, or context strategies without touching the core loop.
---
