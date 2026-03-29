#!/usr/bin/env bash
# Run Clido verification and optional CLI in an isolated test environment.
# Does not touch ~/.config/clido or your real repos.
#
# Usage: ./scripts/run-in-test-env.sh [init|verify|all|tui|<clido-args...>]
#   verify        — cargo build, cargo test, scripts/verify-dod.sh
#   init          — create test dir, run interactive clido init (config -> TEST_DIR/config.toml)
#   all           — verify first, then init (default)
#   tui           — open the interactive TUI using your real ~/.config/clido config
#   <anything>    — pass directly to clido (e.g. doctor, sessions list, run "fix bug")
#
# Override test dir:  CLIDO_TEST_DIR=/path/to/dir ./scripts/run-in-test-env.sh
# Override workdir:   CLIDO_WORKDIR=/your/project  ./scripts/run-in-test-env.sh tui

set -e
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Workdir for clido: honour CLIDO_WORKDIR env var, otherwise default to REPO_ROOT.
export CLIDO_WORKDIR="${CLIDO_WORKDIR:-$REPO_ROOT}"

TEST_DIR="${CLIDO_TEST_DIR:-/tmp/clido-test-env}"
# NOTE: CLIDO_CONFIG is NOT exported globally — it is only set for commands that write
# to the isolated test dir (init, all). Commands like "tui" use the real config so the
# user is not forced through first-run setup every time.

run_verify() {
  echo "=== Build ==="
  cargo build --all-targets
  echo "=== Tests ==="
  cargo test
  echo "=== DoD ==="
  scripts/verify-dod.sh
}

run_init() {
  mkdir -p "$TEST_DIR"
  local cfg="$TEST_DIR/config.toml"
  echo "=== Interactive init (config -> $cfg) ==="
  echo "  Clido will ask 3 questions: provider, model, then API key (or base URL for local)."
  echo "  Use arrow keys to select, or type and press Enter. (Config → $cfg)"
  CLIDO_CONFIG="$cfg" cargo run -p clido-cli -q -- init
}

run_tui() {
  # Use the real config so the user is not prompted for first-run setup.
  cargo run -p clido-cli -q
}

case "${1:-all}" in
  verify) run_verify ;;
  init)   run_init ;;
  all)    run_verify; run_init ;;
  tui)    run_tui ;;
  help)   cargo run -p clido-cli -q -- help ;;
  *)
    # Pass all arguments directly to clido using the real config.
    mkdir -p "$TEST_DIR"
    cargo run -p clido-cli -q -- "$@"
    ;;
esac
echo "Done."
