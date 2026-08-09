#!/usr/bin/env bash
#
# Multi-window integration test for Garden.
#
# Opens the real windowed frontend, spawns a second OS window at runtime
# (:windownew), and asserts the two windows are independent: the debug server's
# per-window addressing (?window=<ordinal>, /windows) routes to the right one,
# an edit in one window never touches the other, closing the focused window
# leaves the process and the surviving window intact, and Cmd+Q quits.
#
# WINDOWED-ONLY: this test genuinely opens (and closes) real OS windows on the
# desktop for a few seconds — there is no headless path for it, because the
# whole point is the winit/wgpu window registry that headless does not have.
#
# HOME is redirected to a throwaway dir so spawned windows load a known
# init.ptl (an empty editor) and the state DB never touches the real ~/.garden.
#
# Usage:  scripts/multi-window-integration-test.sh
# Exit:   0 if every assertion passes, 1 otherwise.

set -uo pipefail

GARDEN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/garden-mwi.XXXXXX")"
HOME_DIR="$WORK/home"
SCRATCH="$WORK/w1.txt"
INIT="$WORK/init.ptl"
LOG="$WORK/app.log"
APP_PID=""

cleanup() {
  [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null
  rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail=0
check() { # description  actual  expected
  if [ "$2" = "$3" ]; then
    printf '  ok   %s\n' "$1"
    pass=$((pass + 1))
  else
    printf '  FAIL %s\n        got [%s] want [%s]\n' "$1" "$2" "$3"
    fail=$((fail + 1))
  fi
}

# --- fixtures ---------------------------------------------------------------
# A spawned window loads $HOME/.garden/init.ptl; make that a single empty
# editor so window 2's buffer is deterministically empty.
mkdir -p "$HOME_DIR/.garden"
printf 'layout(editor())\n' > "$HOME_DIR/.garden/init.ptl"
# Window 1 is launched with --init on a file with recognizable content.
printf 'W1LINE\n' > "$SCRATCH"
printf 'layout(editor("%s"))\n' "$SCRATCH" > "$INIT"

# --- launch -----------------------------------------------------------------
echo "building..."
( cd "$GARDEN_DIR" && cargo build -p garden-app ) || { echo "build failed"; exit 1; }
BIN="$GARDEN_DIR/target/debug/garden"

# Run the built binary directly (not `cargo run`): HOME is redirected for the
# app, and cargo itself needs the real $HOME for its registry/toolchain.
echo "launching windowed app (a real window will open)..."
HOME="$HOME_DIR" "$BIN" --debug-port 0 --init "$INIT" >"$LOG" 2>&1 &
APP_PID=$!
disown "$APP_PID" 2>/dev/null

BASE=""
for _ in $(seq 1 60); do
  port="$(grep -oE 'http://127\.0\.0\.1:[0-9]+' "$LOG" 2>/dev/null | grep -oE '[0-9]+$' | head -1)"
  if [ -n "$port" ] && curl -s "http://127.0.0.1:$port/state" >/dev/null 2>&1; then
    BASE="http://127.0.0.1:$port"
    break
  fi
  sleep 0.25
done
if [ -z "$BASE" ]; then
  echo "app did not start; log:"; cat "$LOG"; exit 1
fi
echo "debug server at $BASE"

# --- helpers ----------------------------------------------------------------
key()  { curl -s -X POST "$BASE/key" -d "{\"key\":\"$1\",\"mods\":${2:-[]}}" >/dev/null; }
text() { curl -s -X POST "$BASE/text" -d "{\"text\":\"$1\"}" >/dev/null; }
# Type an ex command char by char (command-line input must be per-key, not /text).
ex() { key ":"; local s="$1" i c; for ((i=0;i<${#s};i++)); do c="${s:i:1}"; [ "$c" = " " ] && c="space"; key "$c"; done; key enter; }
# Buffer text of pane 0 in a specific window ordinal.
bufw() { curl -s "$BASE/buffer/0?window=$1"; }
win_count() { curl -s "$BASE/windows" | python3 -c "import sys,json;print(len(json.load(sys.stdin)['windows']))"; }
win_focused() { # ordinal -> True / False / MISSING
  curl -s "$BASE/windows" | python3 -c "
import sys, json
d = json.load(sys.stdin)
w = [x for x in d['windows'] if x['window'] == $1]
print(w[0]['focused'] if w else 'MISSING')"
}
alive() { kill -0 "$APP_PID" 2>/dev/null && echo alive || echo dead; }

# --- assertions -------------------------------------------------------------
echo "running checks..."

# One window at startup, ordinal 1, focused, with the launch file's content.
check "starts with exactly one window"     "$(win_count)"        "1"
check "window 1 is focused"                "$(win_focused 1)"    "True"
check "window 1 shows its launch file"     "$(bufw 1 | head -1)" "W1LINE"

# Spawn a second window at runtime.
ex "windownew"
sleep 1.5

check "now there are two windows"          "$(win_count)"        "2"
check "the new window (2) took focus"      "$(win_focused 2)"    "True"
check "window 1 is no longer focused"      "$(win_focused 1)"    "False"

# Isolation: the fresh window's buffer is empty; window 1's is untouched.
check "window 2 opened an empty buffer"    "$(bufw 2 | head -1)" ""
check "window 1 still holds its content"   "$(bufw 1 | head -1)" "W1LINE"

# Edit the focused window (2); window 1 must not change at all.
key i; text "W2EDIT"; key escape
check "the edit landed in window 2"        "$(bufw 2 | head -1)" "W2EDIT"
check "editing window 2 left window 1 alone" "$(bufw 1 | head -1)" "W1LINE"

# Close the focused window (Cmd+W). The process and window 1 survive.
key w '["cmd"]'
sleep 1
check "the process is still running"       "$(alive)"            "alive"
check "one window remains"                 "$(win_count)"        "1"
# The ordinal is not renumbered when a window closes.
check "the survivor keeps ordinal 1"       "$(win_focused 1)"    "True"
check "no window 2 lingers"                "$(win_focused 2)"    "MISSING"
# The focused default (/buffer without ?window) now serves the survivor.
check "the survivor serves its own buffer" "$(curl -s "$BASE/buffer/0" | head -1)" "W1LINE"

# Cmd+Q quits the whole process.
key q '["cmd"]'
gone="dead"
for _ in $(seq 1 12); do
  [ "$(alive)" = "dead" ] && { gone="dead"; break; }
  gone="alive"; sleep 0.25
done
check "Cmd+Q exits the process"            "$gone"               "dead"
APP_PID=""

# --- summary ----------------------------------------------------------------
echo
echo "passed: $pass   failed: $fail"
[ "$fail" -eq 0 ] && { echo "PASS"; exit 0; } || { echo "FAIL"; exit 1; }
