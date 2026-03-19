#!/usr/bin/env bash
# Run Clido verification and optional CLI in an isolated test environment.
# Does not touch ~/.config/clido or your real repos.
#
# Usage: ./scripts/run-in-test-env.sh [init|verify|all]
#   verify — cargo build, cargo test, scripts/verify-dod.sh
#   init   — create test dir, run interactive clido init (config -> $CLIDO_CONFIG)
#   all    — verify first, then init (default)
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
  echo "  Clido will ask 2 questions: provider (1 or 2), then API key (Y/n)."
  echo "  Type your answer after each question and press Enter."
  cargo run -p clido-cli -q -- init
}

case "${1:-all}" in
  verify) run_verify ;;
  init)   run_init ;;
  all)    run_verify; run_init ;;
  *)      echo "Usage: $0 [init|verify|all]"; exit 1 ;;
esac
echo "Done."
