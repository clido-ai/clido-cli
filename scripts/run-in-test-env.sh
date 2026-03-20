#!/usr/bin/env bash
# Run Clido verification and optional CLI in an isolated test environment.
# Does not touch ~/.config/clido or your real repos.
#
# Usage: ./scripts/run-in-test-env.sh [init|verify|all|tui|<clido-args...>]
#   verify        — cargo build, cargo test, scripts/verify-dod.sh
#   init          — create test dir, run interactive clido init (config -> $CLIDO_CONFIG)
#   all           — verify first, then init (default)
#   tui           — open the interactive TUI (no prompt)
#   <anything>    — pass directly to clido (e.g. doctor, sessions list, run "fix bug")
#
# Override test dir: CLIDO_TEST_DIR=/path/to/dir ./scripts/run-in-test-env.sh

set -e
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

TEST_DIR="${CLIDO_TEST_DIR:-/tmp/clido-test-env}"
export CLIDO_CONFIG="$TEST_DIR/config.toml"

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
  echo "=== Interactive init (config -> $CLIDO_CONFIG) ==="
  # ux-requirements §3.2: intro before clido init so user knows questions follow
  echo "  Clido will ask 3 questions: provider, model, then API key (or base URL for local)."
  echo "  Use arrow keys to select, or type and press Enter. (Config → $CLIDO_CONFIG)"
  cargo run -p clido-cli -q -- init
}

run_tui() {
  mkdir -p "$TEST_DIR"
  cargo run -p clido-cli -q
}

case "${1:-all}" in
  verify) run_verify ;;
  init)   run_init ;;
  all)    run_verify; run_init ;;
  tui)    run_tui ;;
  help)   cargo run -p clido-cli -q -- help ;;
  *)
    # Pass all arguments directly to clido in the test environment.
    mkdir -p "$TEST_DIR"
    cargo run -p clido-cli -q -- "$@"
    ;;
esac
echo "Done."
