#!/usr/bin/env bash
#
# Functional integration test for the Git history browser (`:Git`).
#
# `garden git log` (the CLI twin of `:Git`) launches the `git-log` panel-mode GPP
# app (gpp-apps/git-viewers): it pushes a Petal-drawn drawer the host runs
# in-process — a commit list and per-commit file list on the left, the selected
# file's diff on the right — and answers its `query(kind, arg)` requests over the
# pipe by shelling out to `git`. The drawer bakes in NO data; it loads the log and
# each commit's diff at runtime through the async `query` native (on Petal's
# pending values). Because loads are async and cross a subprocess pipe, this test
# waits for the pending fetches to land (poll-until helpers) before asserting on
# the panel's observed bindings — every named value the drawer's frame bound,
# reported verbatim at /state → panes[0].panel.values (bools as bools, ints as
# ints), so the assertions below name the drawer's own variables. Same layered
# strategy as scripts/diff-review-integration-test.sh.
#
# It builds a throwaway git repo fixture (three commits + a dirty working tree),
# opens `garden git log` on it headless, and checks: the worktree row and commit
# rows select with keys and clicks (each selection fetching that commit's diff),
# Tab cycles the focus ring, and the wheel scrolls the hovered region without
# moving the selection.
#
# Usage:  scripts/git-panel-integration-test.sh [--window]
# Exit:   0 if every assertion passes, 1 otherwise.

set -uo pipefail

MODE="--headless"
[ "${1:-}" = "--window" ] && MODE=""

GARDEN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$GARDEN_DIR/target/debug/garden"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/garden-git-it.XXXXXX")"
REPO="$WORK/repo"
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
checkgt() { # description  actual  threshold  (actual > threshold)
  if [ "$2" -gt "$3" ] 2>/dev/null; then
    printf '  ok   %s\n' "$1"
    pass=$((pass + 1))
  else
    printf '  FAIL %s\n        got [%s] want [>%s]\n' "$1" "$2" "$3"
    fail=$((fail + 1))
  fi
}
# The `query`-backed data lands asynchronously, so a value that depends on a
# fresh fetch (the log, a newly-selected commit's diff) may take a few frames.
# These poll the reading until it settles (~5s cap) before the assertion, so the
# test exercises the real pending→ready path without racing it.
checke() {  # description  name  expected   (poll pstate name until == expected)
  local got=""
  for _ in $(seq 1 100); do
    got="$(pstate "$2")"; [ "$got" = "$3" ] && break; sleep 0.05
  done
  check "$1" "$got" "$3"
}
checkgte() { # description  name  threshold  (poll pstate name until > threshold)
  local got=""
  for _ in $(seq 1 100); do
    got="$(pstate "$2")"; [ "$got" -gt "$3" ] 2>/dev/null && break; sleep 0.05
  done
  checkgt "$1" "$got" "$3"
}

# --- fixture: three commits, then a dirty working tree ------------------------
mkdir -p "$REPO"
git -C "$REPO" init -q
GC() { git -C "$REPO" -c user.email=t@t -c user.name=t "$@"; }
printf 'a1\na2\n' > "$REPO/a.txt"
GC add -A && GC commit -qm "first: add a.txt"
seq 1 120 > "$REPO/big.txt"                     # a long, scrollable diff
GC add -A && GC commit -qm "second: add big.txt"
printf 'a1\nCHANGED\n' > "$REPO/a.txt"
printf 'c1\n' > "$REPO/c.txt"
GC add -A && GC commit -qm "third: touch a.txt and c.txt"
printf 'a1\nDIRTY\n' > "$REPO/a.txt"            # tracked edit → worktree row

# --- launch (cwd = fixture, so `garden git log` resolves that repo) ------------
echo "building..."
( cd "$GARDEN_DIR" && cargo build -q -p garden-app ) || { echo "build failed"; exit 1; }

echo "launching git history browser with debug server..."
( cd "$REPO" && "$BIN" git log $MODE --debug-port 0 ) >"$LOG" 2>&1 &
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
# One of the panel's observed bindings by name (or "None" if the binding never
# ran this frame). Values keep their real JSON types, so print them the way this
# test compares them: a string bare, everything else as compact JSON — a bare
# python print() of a bool would render True/False and never match "true".
pstate() { curl -s "$BASE/state" | python3 -c "
import sys, json
missing = object()
vals = ((json.load(sys.stdin)['panes'][0]['panel'] or {}).get('values') or {})
v = vals.get('$1', missing)
print('None' if v is missing else v if isinstance(v, str) else json.dumps(v))"; }
kind()   { curl -s "$BASE/state" | python3 -c "import sys,json;print(json.load(sys.stdin)['panes'][0]['kind'])"; }
key()    { curl -s -X POST "$BASE/key" -d "{\"key\":\"$1\"}" >/dev/null; }
# On-screen text runs containing "error" (a panel runtime error renders as text).
errcount() { curl -s "$BASE/scene" | python3 -c "import sys,json;d=json.load(sys.stdin);print(sum(1 for p in d['primitives'] if p.get('type')=='text' and 'error' in p['text'].lower()))"; }
# Window coords for panel-local geometry (drawer constants: header 38, PAD 12 →
# commit rows start at panel y=74, 40px each; the diff region sits right of the
# left column, which is clamp(36% of width, 280..480) + 12 wide).
panelpt() { # local-dx  local-dy  → "x y" window coords
  curl -s "$BASE/state" | python3 -c "
import sys, json
r = json.load(sys.stdin)['panes'][0]['rect']
print(int(r['x']) + $1, int(r['y']) + $2)"
}
click_at() { # local-x local-y
  panelpt "$1" "$2" | { read -r x y; curl -s -X POST "$BASE/mouse" -d "{\"op\":\"click\",\"x\":$x,\"y\":$y}" >/dev/null; }
}
scroll_at() { # local-x local-y lines
  panelpt "$1" "$2" | { read -r x y; curl -s -X POST "$BASE/mouse" -d "{\"op\":\"scroll\",\"x\":$x,\"y\":$y,\"lines\":$3}" >/dev/null; }
}
mouse_at() { # op local-x local-y   (op = down|move|up, for dragging)
  panelpt "$2" "$3" | { read -r x y; curl -s -X POST "$BASE/mouse" -d "{\"op\":\"$1\",\"x\":$x,\"y\":$y}" >/dev/null; }
}
# Panel-local x inside the diff region: just right of the (draggable) left
# column, whose current width the drawer binds (and the host observes) as `left_w`.
# Wait for the asynchronous `query("log", …)` to resolve before asserting on it.
echo "waiting for the git history to load (pending → ready)..."
for _ in $(seq 1 200); do
  [ "$(pstate log_ready)" = "true" ] && break
  sleep 0.05
done
DIFF_X="$(( $(pstate left_w) + 40 ))"

# --- assertions -------------------------------------------------------------
echo "running assertions..."
check "pane is a panel"                 "$(kind)"                  "panel"
check "the log loaded (pending→ready)"  "$(pstate log_ready)"      "true"
check "panel has no runtime error"      "$(errcount)"              "0"
checke "3 commits + the worktree row"   total_rows                 "4"
check "starts on the worktree row"      "$(pstate commit_selected)" "0"
checke "worktree diff has one file"     file_count                 "1"
checke "no data errors"                 has_error                  "false"

# j walks into the history; each selection fetches that commit's file list
# through `query("commit", hash)` — a Pending until the background git lands.
key j
check "j selects the newest commit"     "$(pstate commit_selected)" "1"
checke "third commit changed two files" file_count                 "2"
key j
check "j again → second commit"         "$(pstate commit_selected)" "2"
checke "second commit changed one file" file_count                 "1"
checkgte "big.txt diff is long"         diff_lines                 "100"

# The wheel over the diff region scrolls it; the selection stays put. The diff
# body is a `text_view` region whose content overflows its rect, so the region
# itself consumes the wheel (native editor scroll, independent of the script's
# diff_scroll — the script only sees scroll_y() when the region has nothing to
# scroll). Observe the region's topmost visible text run changing in /scene.
diff_first() { # topmost text run inside the diff region
  panelpt "$DIFF_X" 130 | { read -r dx dy; curl -s "$BASE/scene" | python3 -c "
import sys, json
d = json.load(sys.stdin)
ts = [p for p in d['primitives']
      if p.get('type') == 'text' and p['pos'][0] >= $dx - 30 and p['pos'][1] >= $dy - 20]
ts.sort(key=lambda p: p['pos'][1])
print(ts[0]['text'] if ts else '')"; }
}
top0="$(diff_first)"
scroll_at "$DIFF_X" 200 8
top1="$top0"
for _ in $(seq 1 40); do
  top1="$(diff_first)"; [ "$top1" != "$top0" ] && break; sleep 0.05
done
if [ -n "$top0" ] && [ "$top1" != "$top0" ]; then
  printf '  ok   %s\n' "wheel scrolls the diff (native region scroll)"; pass=$((pass + 1))
else
  printf '  FAIL %s\n        top diff line stayed [%s]\n' "wheel scrolls the diff (native region scroll)" "$top1"; fail=$((fail + 1))
fi
check "wheel does not move selection"   "$(pstate commit_selected)" "2"

# Tab cycles focus: commits → files → diff; keys follow the focused region.
check "commit list focused initially"   "$(pstate focus)"          "0"
key tab
check "Tab focuses the file list"       "$(pstate focus)"          "1"
key tab
check "Tab focuses the diff"            "$(pstate focus)"          "2"
before="$(pstate diff_scroll)"
key pageup
check "PageUp scrolls the focused diff" "$(pstate diff_scroll)"    "0"
key tab
check "Tab wraps back to commits"       "$(pstate focus)"          "0"

# Clicking a commit row selects it (row 0 = worktree, rows are 40px from y=74)
# and resets the diff scroll for the new selection.
click_at 100 $((74 + 20))
check "click selects the worktree row"  "$(pstate commit_selected)" "0"
check "selection change resets scroll"  "$(pstate diff_scroll)"    "0"
checke "worktree file list is back"     file_count                 "1"

# Clicking a hunk header (the first diff row, at panel y≈114) uncollapses the
# diff to full context, and clicking it again collapses it. On the second commit
# (big.txt) the diff is long either way, so we assert the toggle + no shrink.
key j; key j
check "on the big.txt commit"           "$(pstate commit_selected)" "2"
check "diff starts collapsed"           "$(pstate diff_expanded)"  "false"
click_at "$DIFF_X" 114
check "hunk click expands to full ctx"  "$(pstate diff_expanded)"  "true"
checkgte "expanded diff stays full"     diff_lines                 "100"
click_at "$DIFF_X" 114
check "hunk click collapses again"      "$(pstate diff_expanded)"  "false"

# Dragging the vertical divider (at panel x = 12 + left_w) widens the left
# column; the new width holds after the button is released.
lw0="$(pstate left_w)"
divx=$((12 + lw0))
mouse_at down "$divx" 300
mouse_at move $((divx + 140)) 300
lw1="$(pstate left_w)"
checkgt "divider drag widens left column" "$lw1"                   "$lw0"
mouse_at up $((divx + 140)) 300
check "widened column holds after drag"   "$(pstate left_w)"       "$lw1"

# Dragging the horizontal divider (at panel y = header 50 + commits_area + 2)
# resizes the commit list vs the file list.
ca0="$(pstate commits_area)"
hy=$((50 + ca0 + 2))
mouse_at down 100 "$hy"
mouse_at move 100 $((hy - 90))
ca1="$(pstate commits_area)"
mouse_at up 100 $((hy - 90))
if [ "$ca1" -lt "$ca0" ] 2>/dev/null; then
  printf '  ok   %s\n' "horizontal drag shrinks commit list"; pass=$((pass + 1))
else
  printf '  FAIL %s\n        got [%s] want [<%s]\n' "horizontal drag shrinks commit list" "$ca1" "$ca0"; fail=$((fail + 1))
fi

# The ⟳ Refresh button (top-right) re-runs git: after a new tracked change lands
# in the repo, clicking it reloads the working-tree diff and the new file appears.
# This proves Refresh calls git again rather than serving the cached diff.
click_at 100 $((74 + 20))                       # select the worktree row
checke "back on the worktree row"       file_count                 "1"
printf 'brand new\n' > "$REPO/d.txt"; GC add d.txt   # a new staged file vs HEAD
PW="$(curl -s "$BASE/state" | python3 -c "import sys,json;print(int(json.load(sys.stdin)['panes'][0]['rect']['w']))")"
click_at $((PW - 61)) 22                         # click ⟳ Refresh
checke "Refresh re-runs git: new file"  file_count                 "2"

# --- report -----------------------------------------------------------------
echo
echo "passed: $pass   failed: $fail"
[ "$fail" -eq 0 ]
