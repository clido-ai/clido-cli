#!/usr/bin/env bash
# Run Clido verification and optional CLI in an isolated test environment.
# Does not touch your real config.
#
# Usage: ./scripts/run-in-test-env.sh [init|verify|all|tui|<clido-args...>]
#   verify  — cargo build, cargo test, scripts/verify-dod.sh
#   init    — run interactive clido init (config -> TEST_DIR/config.toml)
#   all     — verify first, then init (default)
#   tui     — open the interactive TUI
#   <any>   — pass directly to clido (e.g. doctor, sessions list, run "fix bug")
#
# Config selection (in priority order):
#   1. CLIDO_CONFIG=/path/to/config.toml  — explicit override
#   2. CLIDO_TEST_DIR=/path/to/dir        — uses <dir>/config.toml
#   3. (nothing)                          — defaults to ~/.config/clido-test-env/config.toml
#
# Override workdir:   CLIDO_WORKDIR=/your/project  ./scripts/run-in-test-env.sh tui

set -e
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

export CLIDO_WORKDIR="${CLIDO_WORKDIR:-$REPO_ROOT}"

TEST_DIR="${CLIDO_TEST_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/clido-test-env}"

# Always use the test config unless CLIDO_CONFIG is explicitly set.
if [ -z "${CLIDO_CONFIG:-}" ]; then
  export CLIDO_CONFIG="$TEST_DIR/config.toml"
fi

run_verify() {
  echo "=== Build ==="
  cargo build --all-targets
  echo "=== Tests ==="
  cargo test
  echo "=== DoD ==="
  scripts/verify-dod.sh
}

run_init() {
  local cfg="${CLIDO_CONFIG}"
  mkdir -p "$(dirname "$cfg")"
  echo "=== Interactive init (config -> $cfg) ==="
  cargo run -p clido-cli -q -- init
}

run_tui() {
  echo "=== TUI (config: $CLIDO_CONFIG) ==="
  cargo run -p clido-cli -q
}

case "${1:-all}" in
  verify) run_verify ;;
  init)   run_init ;;
  all)    run_verify; run_init ;;
  tui)    run_tui ;;
  help)   cargo run -p clido-cli -q -- help ;;
  *)
    mkdir -p "$(dirname "$CLIDO_CONFIG")"
    cargo run -p clido-cli -q -- "$@"
    ;;
esac
echo "Done."
