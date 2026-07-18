#!/usr/bin/env bash
# Soak test: run the real, compiled Tauri app for an extended period under
# a synthetically-growing Claude Code transcript, sampling RSS and CPU time
# for its process tree at regular intervals, to catch a memory leak or a
# busy-loop that a short manual run wouldn't show.
#
# Why synthetic transcript growth instead of real `claude` CLI calls: each
# real call is a real, billed API request (confirmed ~$0.05-0.20/call in
# this repo's own commit history) -- fine for a couple of e2e sanity
# checks (see tools/e2e-status-check.sh), not for driving a 15+ minute
# soak loop. The suspected leak surface here is the polling
# tail/parse/emit loop in spawn_transcript_watcher, which synthetic lines
# exercise identically to real ones -- the state machine doesn't know or
# care where a line came from.
#
# Usage:
#   tools/soak-test.sh [duration_seconds] [line_interval_ms]
#
# Defaults: 900s (15 min) duration, a new synthetic transcript line every
# 200ms (~4500 lines total at the default duration -- a heavier sustained
# rate than any real interactive session would produce).
#
# Requires: a release build (cargo build --release, from the repo root --
# this is a Cargo workspace so the binary lands in ./target/release/, not
# src-tauri/target/release/), and Xvfb on PATH (no real display needed).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

DURATION="${1:-900}"
LINE_INTERVAL_MS="${2:-200}"

BIN="$(pwd)/target/release/desktop-companion"
if [[ ! -x "$BIN" ]]; then
  echo "Release binary not found at $BIN -- build it first:" >&2
  echo "  cargo build --release" >&2
  exit 1
fi

WORKDIR=$(mktemp -d)
cleanup() {
  # Kill in reverse-dependency order and swallow errors -- this runs on
  # every exit path (including a failed assertion), and a stray already-dead
  # PID here shouldn't mask the real exit status. Explicit PIDs, not a
  # process-group kill: xvfb-run/Xvfb/the app all inherit this script's
  # stdout, so leaving any of them alive is also what hangs a downstream
  # `| tail` reader (that's the actual bug this replaced -- see git log).
  kill "${GEN_PID:-0}" 2>/dev/null || true
  kill "${APP_PID:-0}" 2>/dev/null || true
  kill "${XVFB_PID:-0}" 2>/dev/null || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

PROJECTS_ROOT="$WORKDIR/claude-projects"
SESSION_DIR="$PROJECTS_ROOT/-soak-test-project"
mkdir -p "$SESSION_DIR"
TRANSCRIPT="$SESSION_DIR/soak-session.jsonl"
touch "$TRANSCRIPT"

SAMPLES="$WORKDIR/samples.tsv"
echo -e "elapsed_s\trss_kb\tcpu_time_s\tthread_count" > "$SAMPLES"

echo "Workdir: $WORKDIR"
echo "Transcript root: $PROJECTS_ROOT"
echo "Duration: ${DURATION}s, synthetic line every ${LINE_INTERVAL_MS}ms"
echo

# --- Line generator: appends a realistic, repeating turn cycle so the
# state machine keeps transitioning (thinking -> typing -> [error 1-in-N]
# -> waitingForInput -> thinking -> ...) instead of sitting idle, which is
# what would actually stress the tail/parse/emit path over time.
(
  i=0
  while true; do
    i=$((i + 1))
    is_error="false"
    if (( i % 17 == 0 )); then is_error="true"; fi
    {
      printf '{"type":"user","message":{"content":[{"type":"text","text":"turn %d"}]}}\n' "$i"
      printf '{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"..."}]}}\n'
      printf '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}\n'
      printf '{"type":"user","message":{"content":[{"type":"tool_result","is_error":%s,"content":"r"}]}}\n' "$is_error"
      printf '{"type":"assistant","message":{"content":[{"type":"text","text":"done %d"}]}}\n' "$i"
    } >> "$TRANSCRIPT"
    sleep "$(awk -v ms="$LINE_INTERVAL_MS" 'BEGIN{print ms/1000}')"
  done
) &
GEN_PID=$!

# Managing Xvfb directly (rather than via the xvfb-run wrapper) so this
# script holds the exact PIDs it started -- xvfb-run's own child (Xvfb) is
# otherwise untracked and outlives a `kill` of just the wrapper, which is
# what orphaned Xvfb + the app in an earlier version of this script and
# hung a `| tail` reader downstream (it inherits this script's stdout and
# keeps the pipe open even after this script exits).
DISPLAY_NUM=$((RANDOM % 5000 + 100))
Xvfb ":$DISPLAY_NUM" -screen 0 1280x1024x24 -nolisten tcp > "$WORKDIR/xvfb.log" 2>&1 &
XVFB_PID=$!
sleep 1

export DISPLAY=":$DISPLAY_NUM"
export COMPANION_TRANSCRIPT_ROOT="$PROJECTS_ROOT"
# Redirected to a log file, not inherited from this script -- inheriting
# would keep this script's stdout pipe open for as long as the app (or its
# webkit2gtk child processes) are alive, which is exactly what hung a
# downstream `| tail` reader in an earlier version of this script.
"$BIN" > "$WORKDIR/app.log" 2>&1 &
APP_PID=$!
sleep 2 # let it finish window/tray setup before we start sampling

if ! kill -0 "$APP_PID" 2>/dev/null; then
  echo "App failed to start -- see $WORKDIR/app.log:" >&2
  cat "$WORKDIR/app.log" >&2
  exit 1
fi
echo "App pid: $APP_PID (display :$DISPLAY_NUM, Xvfb pid $XVFB_PID)"

START=$(date +%s)
END=$((START + DURATION))
while [[ $(date +%s) -lt $END ]]; do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "App process $APP_PID died mid-soak -- treat as a failure." >&2
    exit 1
  fi
  ELAPSED=$(( $(date +%s) - START ))
  # RSS across the whole process tree (main + webview subprocesses), not
  # just the parent -- a leak in a child renderer process counts too.
  RSS_KB=$(ps --ppid "$APP_PID" -o rss= 2>/dev/null | awk -v p="$(ps -o rss= -p "$APP_PID")" '{s+=$1} END{print s+p+0}')
  CPU_TIME=$(ps -o cputimes= -p "$APP_PID" 2>/dev/null | tr -d ' ')
  THREADS=$(ps -o nlwp= -p "$APP_PID" 2>/dev/null | tr -d ' ')
  echo -e "${ELAPSED}\t${RSS_KB:-0}\t${CPU_TIME:-0}\t${THREADS:-0}" >> "$SAMPLES"
  sleep 10
done

kill "$GEN_PID" 2>/dev/null || true
kill "$APP_PID" 2>/dev/null || true

echo
echo "=== Samples ($SAMPLES) ==="
cat "$SAMPLES"

echo
python3 - "$SAMPLES" <<'PYEOF'
import sys
path = sys.argv[1]
rows = []
with open(path) as f:
    next(f)
    for line in f:
        parts = line.strip().split("\t")
        if len(parts) == 4:
            rows.append(tuple(float(x) for x in parts))

if len(rows) < 3:
    print("Not enough samples to analyze (soak ran too short).")
    sys.exit(0)

first_rss = rows[0][1]
last_rss = rows[-1][1]
max_rss = max(r[1] for r in rows)
growth = last_rss - first_rss
pct = (growth / first_rss * 100) if first_rss else 0

print(f"RSS: first={first_rss:.0f}KB last={last_rss:.0f}KB max={max_rss:.0f}KB growth={growth:+.0f}KB ({pct:+.1f}%)")

# Slope via simple linear regression on (elapsed_s, rss_kb).
n = len(rows)
xs = [r[0] for r in rows]
ys = [r[1] for r in rows]
mean_x = sum(xs) / n
mean_y = sum(ys) / n
num = sum((x - mean_x) * (y - mean_y) for x, y in zip(xs, ys))
den = sum((x - mean_x) ** 2 for x in xs)
slope = num / den if den else 0
print(f"RSS trend: {slope*60:+.1f} KB/min")

LEAK_THRESHOLD_KB_PER_MIN = 200  # generous; real leaks tend to be much steeper
if slope * 60 > LEAK_THRESHOLD_KB_PER_MIN:
    print(f"FAIL: RSS growing faster than {LEAK_THRESHOLD_KB_PER_MIN} KB/min -- looks like a leak.")
    sys.exit(1)
else:
    print("PASS: no sustained RSS growth trend detected.")
PYEOF
