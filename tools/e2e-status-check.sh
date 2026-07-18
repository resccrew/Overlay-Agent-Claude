#!/usr/bin/env bash
# e2e check: does the companion's derived state actually reflect a real
# Claude Code session, not just hand-written fixtures?
#
# Runs two *independently written* implementations of the transcript ->
# state mapping over the exact same real transcript file:
#   - tools/transcript-watcher/derive-state.mjs (the original Node prototype)
#   - crates/companion-state/examples/replay.rs (the Rust port main.rs ships)
# and diffs their "Final state" / "Transition counts by state" output. If
# the two disagree, either the Rust port drifted from the prototype, or one
# of them is misreading a real transcript shape the unit tests' synthetic
# fixtures didn't cover -- both are exactly the kind of bug that would make
# the companion show the wrong status for a real user.
#
# Usage:
#   tools/e2e-status-check.sh                # auto-picks the most recently
#                                             # modified transcript under
#                                             # ~/.claude/projects
#   tools/e2e-status-check.sh /path/to.jsonl  # check a specific transcript
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

TRANSCRIPT="${1:-}"
if [[ -z "$TRANSCRIPT" ]]; then
  PROJECTS_ROOT="${COMPANION_TRANSCRIPT_ROOT:-$HOME/.claude/projects}"
  TRANSCRIPT=$(find "$PROJECTS_ROOT" -maxdepth 2 -name '*.jsonl' -printf '%T@ %p\n' 2>/dev/null \
    | sort -rn | head -1 | cut -d' ' -f2-)
fi

if [[ -z "$TRANSCRIPT" || ! -f "$TRANSCRIPT" ]]; then
  echo "No real transcript found (looked under ${COMPANION_TRANSCRIPT_ROOT:-$HOME/.claude/projects})." >&2
  echo "Pass a path explicitly, or run this from a machine with a live Claude Code session." >&2
  exit 2
fi

echo "Transcript under test: $TRANSCRIPT"
echo

echo "--- Node reference implementation ---"
NODE_OUT=$(node tools/transcript-watcher/derive-state.mjs "$TRANSCRIPT")
echo "$NODE_OUT" | sed -n '/^Replayed/,/^\(First\|Transitions\)/p' | sed '$d'

echo
echo "--- Rust implementation (companion-state, same code main.rs uses) ---"
cargo build --quiet --example replay -p companion-state
RUST_OUT=$(./target/debug/examples/replay "$TRANSCRIPT")
echo "$RUST_OUT"

extract() {
  # Normalizes both outputs to "final:<state>" + sorted "count:<state>:<n>" lines
  # so ordering differences between the two scripts don't cause false mismatches.
  local text="$1"
  local final_state
  final_state=$(grep -oP '^Final state: \K.*' <<<"$text")
  echo "final:$final_state"
  grep -oP '^  \K[a-zA-Z]+: \d+' <<<"$text" | sed 's/: /:/' | sort | sed 's/^/count:/'
}

NODE_NORM=$(extract "$NODE_OUT")
RUST_NORM=$(extract "$RUST_OUT")

echo
if [[ "$NODE_NORM" == "$RUST_NORM" ]]; then
  echo "PASS: Rust and Node implementations agree on this real transcript."
  exit 0
else
  echo "FAIL: Rust and Node implementations disagree on this real transcript." >&2
  echo "--- Node (normalized) ---" >&2
  echo "$NODE_NORM" >&2
  echo "--- Rust (normalized) ---" >&2
  echo "$RUST_NORM" >&2
  exit 1
fi
