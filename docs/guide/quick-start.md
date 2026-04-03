# Quick Start

Get up and running with clido in about five minutes.

## 1. Install

Build and install from source:

```bash
git clone https://github.com/clido-ai/clido-cli.git
cd clido-cli
cargo install --path crates/clido-cli
```

Verify:

```bash
clido --version
# clido 0.1.0
```

See the [Installation guide](/docs/guide/installation) for prerequisites and platform notes.

## 2. Run `clido init`

The first time you run clido (or at any time), run `clido init` to configure your provider and API key interactively:

```bash
clido init
```

```
Welcome to clido!

? Select a provider:
  > Anthropic (Claude)
    OpenRouter
    Local (Ollama)

? Enter your ANTHROPIC_API_KEY:
  sk-ant-...

? Default model: (claude-sonnet-4-5)

✓ Config written to ~/.config/clido/config.toml
```

::: tip Existing users
If you already have a config file, `clido init` will ask before overwriting it. You can also use `clido config set` to change individual values.
:::

## 3. Run your first prompt

Navigate to a project directory and give clido a task:

```bash
cd ~/projects/my-rust-app
clido "list all Rust source files in this directory and show the total line count"
```

clido will:
1. Identify it needs to find `.rs` files
2. Call the `Glob` tool to find them
3. Call `Bash` to count lines
4. Return a summary

```
[Turn 1] Searching for Rust files...
[Turn 2] Counting lines...

Found 23 Rust source files totalling 4,812 lines.

The largest files are:
- src/main.rs (412 lines)
- src/parser.rs (387 lines)
- src/codegen.rs (341 lines)

  Cost: $0.0008  Turns: 2  Time: 3.1s
```

## 4. Open the interactive TUI

Run `clido` with no arguments (from a TTY) to open the interactive TUI:

```bash
clido
```

```
╭─ clido ──────────────────────────── claude-3-5-sonnet ─╮
│                                                         │
│  No session active. Type a message to begin.            │
│                                                         │
╰─────────────────────────────────────────────────────────╯
[ No session ]  $0.00  0 tok                         q quit
> _
```

Type your message and press **Enter** to send. The agent will respond in the chat pane above the input field.

**Useful TUI shortcuts:**

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Ctrl+C` | Cancel running agent / quit |
| `Ctrl+J` | Insert newline in input |
| `/sessions` | Open session picker |
| `/help` | Show all slash commands |
| `q` | Quit (when agent is idle) |

See the [TUI guide](/docs/guide/tui) for the full reference.

## 5. Verify your setup with `clido doctor`

```bash
clido doctor
```

```
✓ Binary: clido 0.1.0
✓ API key: ANTHROPIC_API_KEY is set
✓ Config: ~/.config/clido/config.toml
✓ Default profile: default → anthropic / claude-sonnet-4-5
✓ Session dir: ~/.local/share/clido/sessions
✓ Bash: /bin/bash
✓ All checks passed.
```

If anything is red, follow the hint printed next to the failing check.

## What's next?

- [Running Prompts](/docs/guide/running-prompts) — non-TUI usage, flags, output formats
- [Sessions](/docs/guide/sessions) — resume and manage past conversations
- [Configuration](/docs/guide/configuration) — providers, profiles, and all config options
- [TUI Guide](/docs/guide/tui) — slash commands, key bindings, layout
- [Workflows](/docs/guide/workflows) — declarative multi-step pipelines
