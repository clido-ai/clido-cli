#!/usr/bin/env bash
# Verify Definition of Done for the active release.
# Reads devdocs/plans/releases/CURRENT and devdocs/plans/releases/<release>-dod.yaml,
# runs each verification (command / cli / coverage), and exits 0 only if all pass.
# Requires: yq v4+ (https://github.com/mikefarah/yq — e.g. brew install yq)

set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RELEASES_DIR="$REPO_ROOT/devdocs/plans/releases"
CURRENT_FILE="$RELEASES_DIR/CURRENT"

if ! command -v yq >/dev/null 2>&1; then
  echo "verify-dod.sh requires yq. Install: brew install yq (macOS) or see https://github.com/mikefarah/yq"
  exit 1
fi

if [[ ! -f "$CURRENT_FILE" ]]; then
  echo "Missing $CURRENT_FILE (active release). Create it with a single line, e.g. v1"
  exit 1
fi

RELEASE=$(grep -v '^#' "$CURRENT_FILE" | grep -v '^[[:space:]]*$' | head -1 | xargs)
if [[ -z "$RELEASE" ]]; then
  echo "CURRENT file is empty or only comments/whitespace"
  exit 1
fi

DOD_YAML="$RELEASES_DIR/$RELEASE-dod.yaml"
if [[ ! -f "$DOD_YAML" ]]; then
  echo "Missing DoD file: $DOD_YAML"
  exit 1
fi

# Count items (yq v4: .items | length)
N=$(yq '.items | length' "$DOD_YAML")
FAILED=0

for (( i=0; i<N; i++ )); do
  ID=$(yq ".items[$i].id" "$DOD_YAML")
  DESC=$(yq ".items[$i].description" "$DOD_YAML")
  TYPE=$(yq ".items[$i].verification.type" "$DOD_YAML")
  STATUS=$(yq ".items[$i].status" "$DOD_YAML")
  PASS=0

  case "$TYPE" in
    command)
      CMD=$(yq ".items[$i].verification.command" "$DOD_YAML")
      TMPCMD=$(mktemp)
      if (cd "$REPO_ROOT" && eval "$CMD") >"$TMPCMD" 2>&1; then
        PASS=1
      else
        echo "    (run: $CMD)"
        tail -5 "$TMPCMD" 2>/dev/null | sed 's/^/    /'
      fi
      rm -f "$TMPCMD"
      ;;
    cli)
      ARGS=()
      while IFS= read -r line; do
        [[ -n "$line" && "$line" != "null" ]] && ARGS+=("$line")
      done < <(yq ".items[$i].verification.argv[]" "$DOD_YAML" 2>/dev/null || true)
      EXIT_EXPECT=$(yq ".items[$i].verification.expect_exit" "$DOD_YAML")
      STDERR_FILTER=$(yq ".items[$i].verification.expect_stderr_contains" "$DOD_YAML")
      TMPOUT=$(mktemp)
      TMPERR=$(mktemp)
      trap "rm -f '$TMPOUT' '$TMPERR'" EXIT
      ACTUAL_EXIT=255
      if (cd "$REPO_ROOT" && cargo run -p clido-cli --quiet -- "${ARGS[@]}" 2>"$TMPERR") >"$TMPOUT"; then
        ACTUAL_EXIT=0
      else
        ACTUAL_EXIT=$?
      fi
      # expect_exit can be a number or a list [0, 1]
      if [[ "$EXIT_EXPECT" == *"["* ]]; then
        # list: e.g. [0, 1] — check if ACTUAL_EXIT is in the list
        EXIT_OK=0
        for e in $(yq ".items[$i].verification.expect_exit[]" "$DOD_YAML"); do
          if [[ "$ACTUAL_EXIT" == "$e" ]]; then EXIT_OK=1; break; fi
        done
        [[ $EXIT_OK -eq 1 ]] && PASS=1
      else
        [[ "$ACTUAL_EXIT" == "$EXIT_EXPECT" ]] && PASS=1
      fi
      if [[ -n "$STDERR_FILTER" && "$STDERR_FILTER" != "null" ]]; then
        if ! grep -q -- "$STDERR_FILTER" "$TMPERR" 2>/dev/null; then
          PASS=0
        fi
      fi
      STDOUT_FILTER=$(yq ".items[$i].verification.expect_stdout_contains" "$DOD_YAML")
      if [[ -n "$STDOUT_FILTER" && "$STDOUT_FILTER" != "null" ]]; then
        if ! grep -q -- "$STDOUT_FILTER" "$TMPOUT" 2>/dev/null; then
          PASS=0
        fi
      fi
      rm -f "$TMPOUT" "$TMPERR"
      trap - EXIT
      ;;
    coverage)
      MIN_PCT=$(yq ".items[$i].verification.min_percent" "$DOD_YAML")
      if ! cargo tarpaulin --help >/dev/null 2>&1; then
        PASS=0
      else
        TMPCOV=$(mktemp)
        trap "rm -f '$TMPCOV'" EXIT
        if (cd "$REPO_ROOT" && cargo tarpaulin --workspace --all-features --out Stdout 2>/dev/null) >"$TMPCOV" 2>&1; then
          COV_LINE=$(grep -oE '[0-9]+\.?[0-9]*%' "$TMPCOV" | tail -1)
          COV_NUM="${COV_LINE%\%}"
          if [[ -n "$COV_NUM" ]]; then
            if awk -v c="$COV_NUM" -v m="$MIN_PCT" 'BEGIN { exit !(c >= m) }'; then
              PASS=1
            fi
          fi
        fi
        rm -f "$TMPCOV"
        trap - EXIT
      fi
      ;;
    *)
      echo "  [??] $ID — unknown verification type: $TYPE"
      PASS=0
      ;;
  esac

  # Documented GAP: treat as pass so verifier can exit 0 when all failures are accepted GAPs.
  if [[ "$STATUS" == "GAP" ]]; then
    PASS=1
  fi

  if [[ $PASS -eq 1 ]]; then
    echo "  PASS: $ID — $DESC"
  else
    echo "  FAIL: $ID — $DESC"
    FAILED=$((FAILED + 1))
  fi
done

if [[ $FAILED -gt 0 ]]; then
  echo ""
  echo "DoD verification failed: $FAILED item(s). Fix or mark as GAP with reason in $DOD_YAML"
  exit 1
fi
echo ""
echo "All DoD items passed for release: $RELEASE"
exit 0
