#!/usr/bin/env bash
# Run Clido verification and optional CLI in an isolated test environment.
# Does not touch ~/.config/clido or your real repos.
#
# Usage: ./scripts/run-in-test-env.sh [init|verify|all|tui|<clido-args...>]
#   verify        — cargo build, cargo test, scripts/verify-dod.sh
#   init          — create test dir, run interactive clido init (config -> TEST_DIR/config.toml)
#   all           — verify first, then init (default)
#   tui           — open the interactive TUI
#   <anything>    — pass directly to clido (e.g. doctor, sessions list, run "fix bug")
#
# Config selection (in priority order):
#   1. CLIDO_CONFIG=/path/to/config.toml  — explicit config file path
#   2. CLIDO_TEST_DIR=/path/to/dir        — uses <dir>/config.toml as config
#   3. (nothing)                          — uses your real ~/.config/clido/config.toml
#
# Examples:
#   ./scripts/run-in-test-env.sh tui                          # real config
#   CLIDO_TEST_DIR=/tmp/mytest ./scripts/run-in-test-env.sh tui  # test config
#   CLIDO_CONFIG=/tmp/mytest/config.toml ./scripts/run-in-test-env.sh tui
#
# Override workdir:   CLIDO_WORKDIR=/your/project  ./scripts/run-in-test-env.sh tui

set -e
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Workdir for clido: honour CLIDO_WORKDIR env var, otherwise default to REPO_ROOT.
export CLIDO_WORKDIR="${CLIDO_WORKDIR:-$REPO_ROOT}"

TEST_DIR="${CLIDO_TEST_DIR:-/tmp/clido-test-env}"

# Resolve which config to use:
#   - CLIDO_CONFIG set explicitly → use as-is
#   - CLIDO_TEST_DIR set          → use TEST_DIR/config.toml
#   - neither                     → leave unset (clido uses ~/.config/clido/config.toml)
if [ -z "${CLIDO_CONFIG:-}" ] && [ -n "${CLIDO_TEST_DIR:-}" ]; then
  export CLIDO_CONFIG="$TEST_DIR/config.toml"
fi
# If CLIDO_CONFIG is already set in the environment it is automatically inherited.

run_verify() {
  echo "=== Build ==="
  cargo build --all-targets
  echo "=== Tests ==="
  cargo test
  echo "=== DoD ==="
  scripts/verify-dod.sh
}

run_init() {
  local cfg="${CLIDO_CONFIG:-$TEST_DIR/config.toml}"
  mkdir -p "$(dirname "$cfg")"
  echo "=== Interactive init (config -> $cfg) ==="
  echo "  Clido will ask 3 questions: provider, model, then API key (or base URL for local)."
  echo "  Use arrow keys to select, or type and press Enter."
  CLIDO_CONFIG="$cfg" cargo run -p clido-cli -q -- init
}

run_tui() {
  if [ -n "${CLIDO_CONFIG:-}" ]; then
    echo "=== TUI (config: $CLIDO_CONFIG) ==="
  fi
  cargo run -p clido-cli -q
}

case "${1:-all}" in
  verify) run_verify ;;
  init)   run_init ;;
  all)    run_verify; run_init ;;
  tui)    run_tui ;;
  help)   cargo run -p clido-cli -q -- help ;;
  *)
    # Pass all arguments directly to clido.
    mkdir -p "$TEST_DIR"
    cargo run -p clido-cli -q -- "$@"
    ;;
esac
echo "Done."
