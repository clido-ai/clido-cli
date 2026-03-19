# Local Development and Testing

This guide explains how to develop and test clido locally without running the agent against repositories you care about.

**Config and env:** Config is loaded from `CLIDO_CONFIG` (if set), then `~/.config/clido/config.toml`, then `.clido/config.toml` (walk upward). Use `--max-budget-usd` to cap spend. Use `CLIDO_LOG=debug` (or `-v`) to increase log verbosity. Profile selection (`--profile`) uses the loaded config; without a config file, the default profile is still applied from built-in defaults.

## The core problem

clido reads and modifies files. During development, you will run incomplete or experimental builds that may behave unexpectedly — wrong edits, runaway loops, missing permission prompts. Running these against your actual repositories is risky.

The solution is isolation: use temporary fixture repositories, constrained run modes, and local model providers wherever possible.

### Quick test (script)

From the repo root, run verification and optional interactive init in an isolated dir (no change to `~/.config/clido`):

```sh
./scripts/run-in-test-env.sh        # verify + init
./scripts/run-in-test-env.sh verify  # build, test, DoD only
./scripts/run-in-test-env.sh init    # interactive init into $CLIDO_TEST_DIR (default /tmp/clido-test-env)
```

Override the test dir: `CLIDO_TEST_DIR=/path/to/dir ./scripts/run-in-test-env.sh init`.

---

## Setting up a development environment

### Use a local model provider for development

For most development work, you should not be hitting the Anthropic or OpenAI APIs. Every test run costs money and depends on network availability.

Set up [Ollama](https://ollama.com) and pull a code-capable model:

```sh
ollama pull codellama
# or a smaller model if you just need the loop to work:
ollama pull qwen2.5-coder:1.5b
```

Configure a local profile in `~/.config/clido/config.toml`:

```toml
default_profile = "local"

[profile.local]
provider = "local"
model = "codellama"
base_url = "http://localhost:11434/v1"

[profile.real]
provider = "anthropic"
model = "claude-sonnet-4-5"
api_key_env = "ANTHROPIC_API_KEY"
```

During development, `clido` uses the local profile by default. Switch to the real one only when you need to validate actual agent quality:

```sh
clido --profile real "explain this module"
```

Note: local models typically do not follow tool call schemas as reliably as cloud models. This is expected. Test the _tool execution and agent loop_ with local models, and test _agent quality_ with real providers.

---

## Fixture repositories

Never run the agent against a repository you are actively working on.

### Create a disposable fixture repository

```sh
mkdir -p /tmp/clido-fixture && cd /tmp/clido-fixture
git init
git commit --allow-empty -m "init"

# Add some realistic content
mkdir -p src tests
cat > src/main.rs << 'EOF'
fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

fn main() {
    println!("{}", greet("world"));
}
EOF

cat > tests/test_main.rs << 'EOF'
// intentional bug: missing test body
#[test]
fn test_greet() {}
EOF

git add . && git commit -m "add fixture content"
```

Now run clido from inside this directory. Even if the agent writes or overwrites files, nothing of value is lost:

```sh
cd /tmp/clido-fixture
clido "find and fix the failing test"
```

Recreate the fixture whenever you want a clean state:

```sh
rm -rf /tmp/clido-fixture && # repeat setup above
```

### Use `tests/fixtures/` for repeatable scenarios

For specific behaviors you want to test repeatedly, commit fixture repositories under `tests/fixtures/`. Each fixture should be small and have a clear purpose stated in a `README.md` inside the fixture:

```
tests/fixtures/
├── sample-project/         # minimal valid Rust project for happy-path runs
├── broken-project/         # intentional compile error for error-recovery tests
├── large-project/          # symlinked or generated repo for performance tests
└── session-fixtures/       # pre-recorded session JSONL files for resume tests
```

To run the agent against a fixture from the workspace root:

```sh
cd tests/fixtures/sample-project
clido "list all public functions"
```

---

## Constraining the agent during development

### Read-only mode (`--permission-mode plan`)

Use plan mode when you want to observe the agent's behavior without allowing any writes or shell commands:

```sh
clido --permission-mode plan "refactor the error handling in src/main.rs"
```

In plan mode, `Write`, `Edit`, and `Bash` calls are blocked. The agent can still read, glob, and grep. This is useful when you are iterating on context assembly or tool routing and don't want file modifications.

### Non-interactive print mode (`--print` / `-p`)

Use `--print` to run the agent without interactive permission prompts. All state-changing tools that would normally prompt the user are denied:

```sh
clido -p "what does this module do"
```

This is safe for exploratory runs and scripting. It is also the mode integration tests use.

### Max turn limit

Prevent runaway loops during development by setting a low turn limit:

```sh
clido --max-turns 5 "do something"
```

If the agent exceeds the limit, it stops and reports a clean result rather than spinning. The default is 50 turns; use a lower value when you are testing a specific interaction and do not need full task completion.

### Max budget

Prevent unexpected API spend during live-provider testing:

```sh
clido --profile real --max-budget-usd 0.05 "fix this test"
```

The agent will stop and report when the cumulative cost reaches the limit. The session is saved and can be resumed.

---

## Inspecting what the agent is doing

### Session files

Every run writes a session JSONL file to the session directory (`~/.local/share/clido/sessions/` on Linux/macOS by default, or the path set by `CLIDO_SESSION_DIR`).

Each line is a structured event: user messages, assistant responses, tool calls, tool results, system events. You can inspect a session with:

```sh
clido sessions list
clido sessions show <session-id>
```

Or read the raw JSONL if you are working on the storage crate:

```sh
cat ~/.local/share/clido/sessions/<session-id>.jsonl | jq .
```

This lets you verify that context, tool results, and session state are being recorded correctly without relying on the rendered output.

### JSON output mode

Use `--output-format json` to get a machine-readable final result instead of streamed text. Useful for scripting test assertions:

```sh
clido -p --output-format json "list the files in src/" | jq .result
```

### Log levels

Set `CLIDO_LOG=debug` for verbose output including context assembly, provider request details, and tool execution steps:

```sh
CLIDO_LOG=debug clido "fix the test"
```

Use `trace` for full request/response bodies (includes API payloads — do not use in shared environments):

```sh
CLIDO_LOG=trace clido -p "what is in src/"
```

---

## Running the automated test suite

### Fast lane (no API keys required)

```sh
cargo test --workspace
```

Or, preferably with `cargo-nextest`:

```sh
cargo nextest run --workspace
```

This runs all unit tests and deterministic integration tests. No network access, no API keys needed.

### Integration tests with mocked providers

Some integration tests use a mock HTTP server (wiremock-rs). These are included in the standard `cargo test` run. They exercise the full provider→agent→tool path without a real API.

### Live-provider tests (explicit opt-in)

Tests that call real provider APIs are gated behind a feature flag and excluded from the standard run:

```sh
ANTHROPIC_API_KEY=... cargo test --workspace --features integration
```

Run these selectively, not on every change. They consume API credits and depend on network availability.

### Running a single crate

When working on a specific crate, test only that crate to keep the feedback loop fast:

```sh
cargo test -p clido-tools
cargo test -p clido-agent
```

---

## Verifying session recovery

To test that a session can be resumed after interruption:

1. Start a run in one terminal and interrupt it mid-turn with `Ctrl-C`:
   ```sh
   clido --max-turns 20 "do a long task"
   # Ctrl-C after a few turns
   ```

2. List sessions to get the ID:
   ```sh
   clido sessions list
   ```

3. Resume:
   ```sh
   clido --resume <session-id> "continue"
   ```

Verify that the resumed session has the correct history, that no turns are duplicated, and that the task continues sensibly.

---

## Verifying permission prompts

To exercise the permission prompt interactively, use a fixture repository and run without `--print`:

```sh
cd /tmp/clido-fixture
clido "add a comment to src/main.rs"
```

The agent will request an `Edit` or `Write` call. The prompt should appear with the full input before execution. Test the responses: `y` (allow once), `a` (always allow for this session), `d` (deny), and `N` / enter (deny).

To verify that non-interactive mode denies without prompting:

```sh
clido -p "add a comment to src/main.rs"
```

The tool result should come back as `is_error: true` with a denial message. The agent should handle this gracefully and either rephrase or stop.

---

## Common development pitfalls

**Running against your active working directory.** Always `cd` into a fixture or temporary directory before running the agent. The agent will read from and potentially write to the current working directory.

**Forgetting to set a turn limit.** A buggy agent loop can run many turns and cost money if you are using a cloud provider. Set `--max-turns 5` or `--max-budget-usd 0.05` during development.

**Using a cloud provider for iteration.** Use a local model (Ollama) for the development loop. Switch to a cloud provider only to validate agent quality or provider-specific behavior.

**Reading session files from a previous run.** If you are working on session storage and resume behavior, make sure you are resuming from a session produced by the current build, not a stale file from a previous version. Check `clido sessions list` timestamps.

**Not checking CLIDO_LOG output.** If a tool result is wrong or the agent behaves unexpectedly, `CLIDO_LOG=debug` will usually show exactly what was sent to the model and what came back. Look there before digging into the source.
