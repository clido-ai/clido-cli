# Installation

## Quick install (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/0xkurt/clido/master/scripts/install.sh | sh
```

This detects your OS and architecture, downloads the latest release binary, and installs it to `~/.local/bin`.

**Options:**

```bash
# Install a specific version
CLIDO_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/0xkurt/clido/master/scripts/install.sh | sh

# Install to a custom directory
CLIDO_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/0xkurt/clido/master/scripts/install.sh | sh
```

**Supported platforms:** macOS (arm64, x86_64), Linux (x86_64, aarch64)

## Build from source

Requires Rust 1.94+ ([rustup.rs](https://rustup.rs/)).

```bash
git clone https://github.com/0xkurt/clido.git
cd clido
cargo install --path crates/clido-cli --locked
```

## Verify the installation

```bash
clido --version
```

Expected output:

```
clido 0.1.0
```

Check that all required tools and configuration are present:

```bash
clido doctor
```

```
✓ Binary: clido 0.1.0
✓ API key: ANTHROPIC_API_KEY is set
✓ Config: ~/.config/clido/config.toml
✓ Session dir: ~/.local/share/clido/sessions
✓ Bash: /bin/bash
✓ All checks passed.
```

If any check fails, `doctor` will explain what to do. See the [First Run](/guide/first-run) guide for full setup details.

## Platform notes

### macOS

Fully supported on macOS 12+. The `--sandbox` flag uses `sandbox-exec` when available.

### Linux

Fully supported. The `--sandbox` flag uses `bwrap` (bubblewrap) when available. Install bubblewrap for sandboxed Bash execution:

```bash
# Debian / Ubuntu
sudo apt install bubblewrap

# Fedora / RHEL
sudo dnf install bubblewrap
```

### Windows

Windows is not currently supported. WSL2 with Ubuntu is a viable workaround.

## Shell completions

Generate completion scripts for your shell:

```bash
# Bash
clido completions bash >> ~/.bash_completion

# Zsh (add to ~/.zshrc)
clido completions zsh > "${fpath[1]}/_clido"

# Fish
clido completions fish > ~/.config/fish/completions/clido.fish
```

## Man page

Generate and install the man page:

```bash
clido man > /usr/local/share/man/man1/clido.1
man clido
```

## Updating

Pull the latest changes and reinstall:

```bash
cd clido
git pull
cargo install --path crates/clido-cli --force
```

## Next steps

- [Quick Start](/guide/quick-start) — run your first prompt
- [First Run](/guide/first-run) — configure a provider and API key
- [Configuration](/guide/configuration) — full config reference
