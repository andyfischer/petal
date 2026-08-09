#!/usr/bin/env bash
#
# Functional integration test for Garden.
#
# Boots the real app with the debug server on a free port, drives it over HTTP
# the way a user would (vim keystrokes, the command line), and asserts on the
# observable state (/state JSON and /buffer text) and on files written to disk.
#
# This is the top layer of the testing strategy (see docs/testing.md): it
# exercises the whole stack — frontend loop, key routing, vim state machine,
# command line, and file I/O — that the pure unit tests cannot reach. It runs
# the headless frontend by default (no window, no GPU needed); pass --window
# to run the same checks through the real winit/wgpu frontend instead.
#
# Usage:  scripts/integration-test.sh [--window]
# Exit:   0 if every assertion passes, 1 otherwise.

set -uo pipefail

MODE="--headless"
[ "${1:-}" = "--window" ] && MODE=""

GARDEN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/garden-it.XXXXXX")"
SCRATCH="$WORK/scratch.txt"
OTHER="$WORK/other.txt"
SCRIPT="$WORK/init.ptl"
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
printf 'alpha\nbravo\ncharlie\n' > "$SCRATCH"
printf 'OTHER ONE\nOTHER TWO\n' > "$OTHER"
printf 'layout(editor("%s"))\n' "$SCRATCH" > "$SCRIPT"

# --- launch -----------------------------------------------------------------
echo "building..."
( cd "$GARDEN_DIR" && cargo build -p garden-app ) || { echo "build failed"; exit 1; }

echo "launching app with debug server..."
( cd "$GARDEN_DIR" && cargo run -q -p garden-app -- $MODE --debug-port 0 --init "$SCRIPT" ) >"$LOG" 2>&1 &
APP_PID=$!
disown "$APP_PID" 2>/dev/null  # suppress the job-control "Terminated" line on cleanup

# Discover the chosen port from the startup line and wait for it to answer.
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
field() { curl -s "$BASE/state" | python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }
buf0()  { curl -s "$BASE/buffer/0"; }
key()   { curl -s -X POST "$BASE/key" -d "{\"key\":\"$1\",\"mods\":${2:-[]}}" >/dev/null; }
text()  { curl -s -X POST "$BASE/text" -d "{\"text\":\"$1\"}" >/dev/null; }

# --- assertions -------------------------------------------------------------
echo "running checks..."

check "starts in Normal mode"            "$(field "d['panes'][0]['mode']")" "NORMAL"
check "opened the scratch file"          "$(field "d['panes'][0]['file']")" "$SCRATCH"

# Insert mode round-trip.
key i
check "i enters Insert mode"             "$(field "d['panes'][0]['mode']")" "INSERT"
text "X"
key escape
check "Escape returns to Normal"         "$(field "d['panes'][0]['mode']")" "NORMAL"
check "typed text was inserted"          "$(buf0 | head -1)"                "Xalpha"

# Delete a line.
key d; key d
check "dd deletes the current line"      "$(buf0 | head -1)"                "bravo"

# Yank + paste duplicates a line.
key y; key y; key p
check "yy then p duplicates a line"      "$(buf0 | sed -n '1,2p' | paste -sd'|' -)" "bravo|bravo"

# Command line: write the buffer, then confirm it hit disk.
key ":"; key w; key enter
check "buffer no longer dirty after :w"  "$(field "d['panes'][0]['dirty']")" "False"
check ":w persisted to disk"             "$(head -1 "$SCRATCH")"            "bravo"

# External file refresh: the reload-poll watches each open file's mtime/size.
# A clean buffer is reloaded silently when the file changes underneath it.
contains() { case "$2" in *"$1"*) echo ok;; *) echo "$2";; esac; }  # substr -> ok
printf 'DISK ONE\nDISK TWO\n' > "$SCRATCH"
sleep 0.5  # let the 200ms reload-poll pick up the external change
check "clean buffer reloads on external change" "$(buf0 | head -1)" "DISK ONE"
check "a reload note shows in the status bar"    "$(contains 'reloaded from disk' "$(field "d['status_note']")")" "ok"

# A dirty buffer is never clobbered: the external change warns instead.
key g; key g; key 0; key i; text "Z"; key escape  # dirty: "ZDISK ONE..."
printf 'NEWER\n' > "$SCRATCH"
sleep 0.5
check "dirty buffer keeps the unsaved edit"      "$(buf0 | head -1)" "ZDISK ONE"
check "an external-change warning shows"          "$(contains 'changed on disk' "$(field "d['status_note']")")" "ok"
key u  # undo -> clean again; the next poll is free to reload the newest content
sleep 0.5
check "reload resumes once the buffer is clean"  "$(buf0 | head -1)" "NEWER"

# Command line: open another file in the pane.
key ":"; key e; key space
for ch in $(echo "$OTHER" | sed 's/./& /g'); do key "$ch"; done
key enter
check ":e opened the other file"         "$(field "d['panes'][0]['file']")" "$OTHER"
check ":e loaded its contents"           "$(buf0 | head -1)"                "OTHER ONE"

# Search: / jumps to the next match, n wraps, :noh clears the highlights.
# (Buffer is OTHER: "OTHER ONE" / "OTHER TWO" — two matches for "OTHER".)
hl_count() { # search-match quads in the scene (theme::SEARCH_MATCH)
  curl -s "$BASE/scene" | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(sum(1 for p in d['primitives']
          if p['type'] == 'quad'
          and abs(p['color'][0] - 0xd7/255) < 0.001 and abs(p['color'][3] - 0.30) < 0.001))"
}
key "/"
for ch in O T H E R; do key "$ch"; done
check "search prompt shows in /state"    "$(field "d['command_line']")"      "/OTHER"
key enter
check "/OTHER jumps to the next match"   "$(field "d['panes'][0]['cursor']['line']")" "1"
check "search matches are highlighted"   "$(hl_count)"                       "2"
key n
check "n wraps back to the first match"  "$(field "d['panes'][0]['cursor']['line']")" "0"
key ":"; key n; key o; key h; key enter
check ":noh clears the highlights"       "$(hl_count)"                       "0"

# :s substitution — plain-text search/replace. Buffer is OTHER:
# "OTHER ONE" / "OTHER TWO". ex() opens ":" and types a command char by char.
ex() { key ":"; local s="$1" i c; for ((i=0;i<${#s};i++)); do c="${s:i:1}"; [ "$c" = " " ] && c="space"; key "$c"; done; key enter; }
two_lines() { buf0 | sed -n '1,2p' | paste -sd'|' -; }
ex "%s/OTHER/X/g"
check ":%s replaces across the whole buffer" "$(two_lines)" "X ONE|X TWO"
key u
check "the whole :%s undoes in one step"     "$(two_lines)" "OTHER ONE|OTHER TWO"
key g; key g  # back to the first line
ex "s/OTHER/Y/"
check ":s replaces only the current line"    "$(two_lines)" "Y ONE|OTHER TWO"
ex "s/Y/OTHER/"  # restore line 0 for the multi-click checks below
check ":s restores the line"                 "$(two_lines)" "OTHER ONE|OTHER TWO"

# Smartcase search: an all-lowercase pattern matches the uppercase text.
key g; key g  # to line 0
key "/"; for ch in o t h e r; do key "$ch"; done; key enter
check "lowercase search matches via smartcase" "$(field "d['panes'][0]['cursor']['line']")" "1"
check "smartcase highlights every match"        "$(hl_count)" "2"
key ":"; key n; key o; key h; key enter  # clear highlights again

# Multi-click selection: the /mouse "clicks" field drives double-click word
# and triple-click line selection. Click coordinates are computed from /state
# geometry (pane rect, cell size, gutter = max(3, digits(line_count)) + 2
# cells, pane padding 6px). Buffer is still OTHER: "OTHER ONE" / "OTHER TWO".
click_sel() { # clicks line col -> selected text as a JSON string (or null)
  curl -s "$BASE/state" | python3 -c "
import sys, json
d = json.load(sys.stdin)
p = d['panes'][0]; r = p['rect']
cw, ch = d['cell']['width'], d['cell']['height']
gutter = (max(3, len(str(p['line_count']))) + 2) * cw
print(r['x'] + 6 + gutter + $3 * cw, r['y'] + 6 + ($2 + 0.5) * ch)" | {
    read -r mx my
    curl -s -X POST "$BASE/mouse" -d "{\"op\":\"click\",\"x\":$mx,\"y\":$my,\"clicks\":$1}" \
      | python3 -c "import sys,json;print(json.dumps((json.load(sys.stdin).get('selection') or {}).get('text')))"
  }
}
check "double-click selects the word under it"  "$(click_sel 2 1 7)" '"TWO"'
check "triple-click selects the line + newline" "$(click_sel 3 0 2)" '"OTHER ONE\n"'

# % bracket matching: insert a bracket pair and jump across it.
key g; key g; key o   # open a new line below line 0, in Insert mode
text "(abc)"
key escape
key "0"; key "%"      # from the '(' jump to the matching ')'
check "% jumps to the matching bracket"  "$(field "d['panes'][0]['cursor']['col']")" "4"
key "%"               # and back from the ')' to the '('
check "% jumps back to the opening bracket" "$(field "d['panes'][0]['cursor']['col']")" "0"

# --- directory browser (GPP subprocess pane) --------------------------------
# `garden <dir>` opens the directory-browser GPP client in pane 0: a navigable
# listing whose text the subprocess pushes over the Garden Pane Protocol. The
# host forwards the subscribed navigation keys; selecting a file asks the host
# to swap the pane for a normal editor (openPath). This is a separate app
# instance because it is launched on a directory argument, not the .ptl script.
echo "running directory-browser checks..."
kill "$APP_PID" 2>/dev/null; APP_PID=""

DBROOT="$WORK/dbtree"
mkdir -p "$DBROOT/subdir"
printf 'hello world\n' > "$DBROOT/file_a.txt"
printf 'second\n'      > "$DBROOT/subdir/inner.txt"

( cd "$GARDEN_DIR" && cargo run -q -p garden-app -- $MODE --debug-port 0 "$DBROOT" ) >"$LOG" 2>&1 &
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
  echo "directory-browser app did not start; log:"; cat "$LOG"; exit 1
fi

# The pane is process-backed; the subprocess identifies itself as the browser.
check "pane 0 is a process pane"          "$(field "d['panes'][0]['kind']")"               "process"
check "the process is directory-browser"  "$(field "d['panes'][0]['process']['name']")"    "directory-browser"

# The initial listing shows ".." then the dir and file, with "> " on row 0.
check "listing marks the selected row"    "$(buf0 | sed -n '1p')"  "> ../"
check "listing shows the subdir"          "$(buf0 | grep -c 'subdir/')"    "1"
check "listing shows the file"            "$(buf0 | grep -c 'file_a.txt')" "1"

# j moves the selection marker down a row.
key j
check "j moves the selection down"        "$(buf0 | grep -n '^> ' | cut -d: -f1)" "2"

# Selection is now on "subdir/"; Enter descends into it.
key enter
check "Enter descends into the subdir"    "$(buf0 | grep -c 'inner.txt')" "1"

# Go back up (Enter on the "..") to the original directory.
key enter
check "Enter on .. returns to the parent" "$(buf0 | grep -c 'file_a.txt')" "1"

# Select file_a.txt (rows: ".." , "subdir/", "file_a.txt") and open it.
key j; key j
check "selection lands on the file"       "$(buf0 | sed -n '3p')" "> file_a.txt"
key enter
sleep 0.3  # the openPath swap drops the subprocess and loads the editor
check "opening a file swaps in an editor"  "$(field "d['panes'][0]['kind']")"  "editor"
check "the opened editor shows the file"    "$(buf0 | head -1)"                 "hello world"

# --- summary ----------------------------------------------------------------
echo
echo "passed: $pass   failed: $fail"
[ "$fail" -eq 0 ] && { echo "PASS"; exit 0; } || { echo "FAIL"; exit 1; }
