# First Run

This page walks through the first-run experience in detail: choosing a provider, entering your API key, understanding where configuration is stored, and verifying everything works.

## The first-run wizard

If no config file exists when you run `clido`, the wizard starts automatically:

```bash
clido
```

```
No config file found. Running first-time setup.
(You can re-run this any time with: clido init)

? Select a provider:
  > Anthropic (Claude)       — Best quality, prompt caching support
    OpenRouter               — Access many models with one key
    Local (Ollama)           — No API key required
```

You can also trigger the wizard explicitly at any time:

```bash
clido init
```

## Provider selection

### Anthropic (Claude)

The default and recommended option. Uses the Claude model family directly.

```
? Select a provider: Anthropic (Claude)
? Enter your ANTHROPIC_API_KEY (or set env var ANTHROPIC_API_KEY):
  sk-ant-api03-...
? Default model: (claude-sonnet-4-5)
```

The key is stored in your config file. To avoid storing it in plain text, press Enter to skip and set the `ANTHROPIC_API_KEY` environment variable in your shell profile instead.

### OpenRouter

OpenRouter gives you access to Claude, GPT, Mistral, and many other models through a single API key.

```
? Select a provider: OpenRouter
? Enter your OPENROUTER_API_KEY:
  sk-or-v1-...
? Default model: (anthropic/claude-3-5-sonnet)
```

### Local (Ollama)

No API key is required. Ollama must be running locally on port 11434.

```
? Select a provider: Local (Ollama)
? Ollama base URL: (http://localhost:11434)
? Default model: (llama3.2)
```

::: tip Starting Ollama
Install Ollama from [ollama.ai](https://ollama.ai), then pull a model: `ollama pull llama3.2`
:::

## Setup wizard keyboard (full-screen TUI)

In a normal terminal (TTY), the wizard uses a full-screen UI instead of the prompts shown above.

- **Esc** on the first step cancels without saving—returning to the shell, or to the chat TUI if you started from `/profile new` or `/init`.
- **Ctrl+Q** / **Ctrl+C** also cancel without saving (no failed exit code for cancellation).
- After entering a **new profile name**, **Esc** on the provider step goes back to edit the name.
- **←** / **→**, **Home**, **End** move the cursor in the profile name, API key, and similar fields.
- When **creating a profile** (`/profile new` or `clido profile create`) and another profile already has a credentials file entry, env var, or inline API key for the same provider, the wizard lists it: **↑↓** to choose, **Enter** to reuse, **n** to type a new key.

Non-TTY / piped runs use simple text prompts only; these keys do not apply there.

## Config file location

The wizard writes `~/.config/clido/config.toml` (or `%APPDATA%\clido\config.toml` on Windows, though Windows is not currently supported).

You can override the path with the `CLIDO_CONFIG` environment variable:

```bash
export CLIDO_CONFIG=/path/to/my/config.toml
clido init
```

### Project-level config

clido also looks for `.clido/config.toml` in the current directory and its parents (stopping at `$HOME`). Project config is merged on top of the global config, so you can override the model or provider per repository without changing your global settings.

## The generated config

After `clido init`, your `~/.config/clido/config.toml` looks like:

```toml
default_profile = "default"

[profile.default]
provider = "anthropic"
model    = "claude-sonnet-4-5"
api_key_env = "ANTHROPIC_API_KEY"

[agent]
max-turns      = 200
max-budget-usd = 5.0
```

See the [Configuration reference](/docs/reference/config) for all available keys.

## Verifying your setup

Run the doctor command to check every component:

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

Common failures and their fixes:

| Check | Failure | Fix |
|-------|---------|-----|
| API key | `ANTHROPIC_API_KEY is not set` | Export the variable in your shell profile |
| Config | `No config file found` | Run `clido init` |
| Session dir | `Cannot create session directory` | Check filesystem permissions |
| Bash | `bash not found` | Ensure `/bin/bash` exists or set `PATH` |

## Changing config after first run

Use `clido config set` to update individual values:

```bash
# Change the default model
clido config set model claude-3-opus-20240229

# Change the provider
clido config set provider openrouter

# Update the API key
clido config set api-key sk-ant-...
```

View the current config:

```bash
clido config show
```

Or edit the file directly — it is plain TOML:

```bash
$EDITOR ~/.config/clido/config.toml
```

## Environment variable overrides

All config values can be overridden at runtime:

```bash
CLIDO_MODEL=claude-haiku-4-5 clido "quick question"
CLIDO_PROVIDER=openrouter clido run "summarise this file" < README.md
```

See [Environment Variables](/docs/reference/env-vars) for the full list.

## Next steps

- [Quick Start](/docs/guide/quick-start) — run your first prompt
- [Configuration](/docs/guide/configuration) — full guide to config.toml and profiles
- [Providers & Models](/docs/guide/providers) — provider-specific options
