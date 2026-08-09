#!/usr/bin/env bash
#
# Functional integration test for the `garden-diff` review client — the one
# diff/review tool behind `:Diff`, `:Review*`, `:PR`, `garden diff`, `garden pr`.
#
# `garden diff <base>` projects `git diff <base>` (base branch → working tree)
# into a panel with three views: an editable unified stream (the default — a real
# vim `edit_view` where deleting a `+` line drops that addition and deleting a `-`
# line reverts that deletion), an editable before/after split (the right column is
# the working tree, and a projection in its own right — `^S` folds its edits back
# into the files), and a per-file stat diagram. This test drives that whole loop over the debug server (see
# docs/debug-server.md), asserting on the panel's observed bindings
# (`panes[].panel.values` — every value the drawer's frame named, in its real
# type) and — the real proof — the underlying file on disk.
#
# It builds a throwaway git repo (a base commit on `main`, a working-tree change
# on a feature branch), opens `garden diff main` headless, then checks: the diff
# loads, the header pills switch views, deleting a `-` line in the unified view
# and saving with `^S` restores the base line, an edit typed into the after column
# and saved reaches the file, and the reloaded diff reflects it. It then covers
# the structural edits the projection makes possible: `dd` on a hunk header
# reverts that hunk in the file, and `dd` on the view's own title is refused.
#
# Usage:  scripts/diff-review-integration-test.sh [--window]
# Exit:   0 if every assertion passes, 1 otherwise.

set -uo pipefail

MODE="--headless"
[ "${1:-}" = "--window" ] && MODE=""

GARDEN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$GARDEN_DIR/target/debug/garden"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/garden-diff-it.XXXXXX")"
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

# --- fixture: a base commit on main, a working-tree change on a branch --------
mkdir -p "$REPO"
git -C "$REPO" init -q -b main
GC() { git -C "$REPO" -c user.email=t@t -c user.name=t "$@"; }
printf 'one\ntwo\nthree\nfour\n' > "$REPO/a.txt"
GC add -A && GC commit -qm base
GC checkout -q -b feature
printf 'one\nTWO\nfour\nfive\n' > "$REPO/a.txt"   # change one line, drop one, add one

# --- launch (cwd = fixture, so `garden diff` resolves that repo) -------------
echo "building..."
( cd "$GARDEN_DIR" && cargo build -q -p garden-app -p garden-diff ) \
  || { echo "build failed"; exit 1; }

echo "launching garden diff with debug server..."
( cd "$REPO" && "$BIN" diff main $MODE --debug-port 0 ) >"$LOG" 2>&1 &
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
key() { curl -s -X POST "$BASE/key" -d "{\"key\":\"$1\"}" >/dev/null; }
keymod() { curl -s -X POST "$BASE/key" -d "{\"key\":\"$1\",\"mods\":[\"$2\"]}" >/dev/null; }
click() { curl -s -X POST "$BASE/mouse" -d "{\"op\":\"click\",\"x\":$1,\"y\":$2}" >/dev/null; }
# The context gesture — `button: 1` in the debug protocol, right in `petal-ui`'s
# numbering. Panels are the only thing that sees it.
rclick() { curl -s -X POST "$BASE/mouse" -d "{\"op\":\"click\",\"x\":$1,\"y\":$2,\"button\":1}" >/dev/null; }
# One value the drawer's last frame bound, by name (see garden_diff.ptl), or ""
# when that binding has not run yet — a name whose term never executed is absent
# from the map, and an empty read is the sentinel every wait loop below tests
# against. (An empty read fed to shell arithmetic fails silently and looks like a
# UI bug — hence the sentinel, and hence the string comparisons.)
#
# Values keep their real types, so the printer has to as well: a bare Python
# `print()` of a JSON bool renders `True`/`False` and would silently fail every
# comparison. Strings print bare (so `mode` reads as `unified`, not `"unified"`);
# everything else prints as compact JSON, so bools are `true`/`false` and ints
# are `1`.
dstate() {
  curl -s "$BASE/state" | python3 -c "
import sys, json
panel = json.load(sys.stdin)['panes'][0].get('panel') or {}
v = (panel.get('values') or {}).get('$1', '')
print(v if isinstance(v, str) else json.dumps(v, separators=(',', ':')))"
}
# The review's scope is the `doc` query argument — "", "commit:<sha>" or
# "since:<sha>" — and the sha is fixture-dependent, so assert on the kind.
scope_kind() {
  case "$(dstate scope)" in
    commit:*) echo commit ;;
    since:*)  echo since ;;
    "")       echo whole ;;
    *)        echo "?" ;;
  esac
}
pkind() { curl -s "$BASE/state" | python3 -c "import sys,json;print(json.load(sys.stdin)['panes'][0]['kind'])"; }
cell_h() { curl -s "$BASE/state" | python3 -c "import sys,json;print(json.load(sys.stdin)['cell']['height'])"; }
# The drawer's coordinates are pane-local; /mouse takes window coordinates, so
# every hit target is offset by the pane's origin.
rect_x() { curl -s "$BASE/state" | python3 -c "import sys,json;print(json.load(sys.stdin)['panes'][0]['rect']['x'])"; }
rect_y() { curl -s "$BASE/state" | python3 -c "import sys,json;print(json.load(sys.stdin)['panes'][0]['rect']['y'])"; }
clickpanel() { # panel-local x y → a window click
  click "$(python3 -c "print(int($1 + $PANE_X))")" "$(python3 -c "print(int($2 + $PANE_Y))")"
}
rclickpanel() { # panel-local x y → a window right-click
  rclick "$(python3 -c "print(int($1 + $PANE_X))")" "$(python3 -c "print(int($2 + $PANE_Y))")"
}
# Wait for the drawer's `ready` flag — every scope change re-enters the loader.
wait_ready() {
  for _ in $(seq 1 40); do
    [ "$(dstate ready)" = "true" ] && break
    sleep 0.25
  done
}
# The status line's error slot — where a projection's refusal surfaces.
status_error() {
  curl -s "$BASE/state" | python3 -c 'import sys, json; print(json.load(sys.stdin).get("status_error") or "")'
}
refused() { # needle → yes/no
  case "$(status_error)" in
    *"$1"*) echo yes ;;
    *) echo no ;;
  esac
}

# The panel loads its diff asynchronously (the client shells `git`), so wait for
# the drawer's own ready flag before asserting anything.
for _ in $(seq 1 40); do
  [ "$(dstate ready)" = "true" ] && break
  sleep 0.25
done

# --- assertions -------------------------------------------------------------
echo "running assertions..."

check "the pane is the garden-diff panel"  "$(pkind)"           "panel"
check "the diff loaded"                    "$(dstate ready)"    "true"
check "no load error"                      "$(dstate has_error)" "false"
check "one changed file"                   "$(dstate files)"    "1"
check "opens in the unified view"          "$(dstate mode)"     "unified"
check "a local diff has no PR block"       "$(dstate has_pr)"   "false"

# --- the header pills switch views -------------------------------------------
PANE_X="$(rect_x)"
PANE_Y="$(rect_y)"
PILL_Y="$(dstate pill_y)"
clickpanel "$(dstate stat_x)" "$PILL_Y"; sleep 0.3
check "the stat pill switches view"        "$(dstate mode)"     "stat"
clickpanel "$(dstate split_x)" "$PILL_Y"; sleep 0.3
check "the split pill switches view"       "$(dstate mode)"     "split"
clickpanel "$(dstate unified_x)" "$PILL_Y"; sleep 0.3
check "the unified pill switches back"     "$(dstate mode)"     "unified"

# --- the wrap toggle (unified only) ------------------------------------------
# Long diff lines soft-wrap to the column by default; the pill turns that off
# for the frames where the exact columns matter.
check "the unified view wraps by default"  "$(dstate uni_wrap)" "true"
clickpanel "$(dstate wrap_x)" "$PILL_Y"; sleep 0.3
check "the wrap pill turns wrapping off"   "$(dstate uni_wrap)" "false"
clickpanel "$(dstate wrap_x)" "$PILL_Y"; sleep 0.3
check "the wrap pill turns it back on"     "$(dstate uni_wrap)" "true"

# --- editing the after column and saving with ^S -----------------------------
clickpanel "$(dstate split_x)" "$PILL_Y"; sleep 0.5
check "back in the split view"             "$(dstate mode)"     "split"
# The after column's projected lines are:
#   0 review: … / 1 @@@ file: a.txt / 2 @@@ hunk: … / 3 one / 4 TWO / 5 four / 6 five
# Click line 4 ("TWO") to focus the editable region with the cursor on its first
# char, then delete that char (`x`) and write the files back (`^S`).
CELL_H="$(cell_h)"
LINE_Y="$(python3 -c "print(int($(dstate body_top) + 4.5 * $CELL_H))")"
clickpanel "$(dstate after_body_x)" "$LINE_Y"; sleep 0.3
key "x"; sleep 0.3
keymod "s" "ctrl"; sleep 1.5

check "the ^S save reached the file"  "$(cat "$REPO/a.txt")" "$(printf 'one\nWO\nfour\nfive\n')"

# The save invalidates the query, so the drawer reloads the (now different) diff.
for _ in $(seq 1 40); do
  [ "$(dstate ready)" = "true" ] && break
  sleep 0.25
done
check "the reloaded diff still has the file" "$(dstate files)"    "1"
check "the reload carried no error"          "$(dstate has_error)" "false"

# The after column is a projection too, but an undecorated one: it shows only the
# new file, so it holds nothing to revert a hunk *back* to. Its `@@@` markers are
# therefore locked chrome — `dd` on one is refused rather than half-reverting the
# hunk (dropping its additions while leaving its deletions in place).
HUNKMARK_Y="$(python3 -c "print(int($(dstate body_top) + 2.5 * $CELL_H))")"
clickpanel "$(dstate after_body_x)" "$HUNKMARK_Y"; sleep 0.3
key "d"; key "d"; sleep 0.5
check "dd on the after column's marker is refused" "$(refused "not the change")" "yes"
check "the refusal left the file alone" \
  "$(cat "$REPO/a.txt")" "$(printf 'one\nWO\nfour\nfive\n')"

# --- editing the unified diff and saving with ^S -----------------------------
# The file is now `one/WO/four/five` against a base of `one/two/three/four`, so
# the reloaded unified stream reads:
#   0 unified: … / 1 @@@ file: a.txt / 2 @@@ hunk: … / 3 " one" / 4 "-two"
#   5 "-three" / 6 "+WO" / 7 " four" / 8 "+five"
# Deleting line 5 (`dd` on "-three") reverts that deletion, so `three` returns to
# the file at the point the diff showed it — the gesture the split view can't do.
clickpanel "$(dstate unified_x)" "$PILL_Y"; sleep 0.5
check "back in the unified view"           "$(dstate mode)"     "unified"
UNI_Y="$(python3 -c "print(int($(dstate body_top) + 5.5 * $CELL_H))")"
clickpanel "$(dstate unified_body_x)" "$UNI_Y"; sleep 0.3
key "d"; key "d"; sleep 0.3
keymod "s" "ctrl"; sleep 1.5

check "deleting a '-' line reverts that deletion" \
  "$(cat "$REPO/a.txt")" "$(printf 'one\nthree\nWO\nfour\nfive\n')"

for _ in $(seq 1 40); do
  [ "$(dstate ready)" = "true" ] && break
  sleep 0.25
done
check "the unified reload carried no error"  "$(dstate has_error)" "false"

# --- structural edits: the projection's tier-2 intents ------------------------
# The file is now `one/three/WO/four/five` against a base of `one/two/three/four`,
# so the reloaded unified stream reads:
#   0 unified: … / 1 @@@ file: a.txt / 2 @@@ hunk: … / 3 " one" / 4 "-two"
#   5 "+three" / 6 "+WO" / 7 " four" / 8 "+five"
#
# `dd` on the *hunk header* is not a line deletion: the projection reads it as a
# request to revert the hunk, so the file goes back to exactly what the base
# holds. Nothing in the diff text says this — it works because the host knows
# each line's origin.
HUNK_Y="$(python3 -c "print(int($(dstate body_top) + 2.5 * $CELL_H))")"
clickpanel "$(dstate unified_body_x)" "$HUNK_Y"; sleep 0.3
key "d"; key "d"; sleep 0.3
keymod "s" "ctrl"; sleep 1.5

check "dd on the hunk header reverts the hunk" \
  "$(cat "$REPO/a.txt")" "$(printf 'one\ntwo\nthree\nfour\n')"

for _ in $(seq 1 40); do
  [ "$(dstate ready)" = "true" ] && break
  sleep 0.25
done
# Reverting every hunk leaves the working tree identical to the base, so the
# reloaded diff is empty — proof the revert reached the file, not just the view.
check "the reverted file leaves nothing to diff" "$(dstate files)" "0"
check "the revert reload carried no error"       "$(dstate has_error)" "false"

# --- a locked line refuses rather than corrupting the view --------------------
# The title line belongs to the view, not to the change. Deleting it is refused
# (with a status message) instead of silently removing a line from a file it has
# nothing to do with.
printf 'one\nTWO\nthree\nfour\n' > "$REPO/a.txt"
curl -s -X POST "$BASE/command" -d '{"command":"Diff main"}' >/dev/null 2>&1
for _ in $(seq 1 40); do
  [ "$(dstate ready)" = "true" ] && break
  sleep 0.25
done
TITLE_Y="$(python3 -c "print(int($(dstate body_top) + 0.5 * $CELL_H))")"
clickpanel "$(dstate unified_body_x)" "$TITLE_Y"; sleep 0.3
key "d"; key "d"; sleep 0.5
check "deleting the title is refused" "$(refused "not the change")" "yes"

# --- the commits view, the context menu, and scoping --------------------------
# The review is `main..feature`, so it has exactly one commit of its own once
# the working-tree change is committed. Committing it also empties the
# whole-review *working-tree* diff, which is what makes the scoping assertions
# below unambiguous: any file the diff shows after this is one the scope put
# there, not a leftover uncommitted edit.
printf 'one\nTWO\nthree\nfour\n' > "$REPO/a.txt"
GC commit -qam "shout the second line"
printf 'beta\n' > "$REPO/b.txt"
GC add -A && GC commit -qm "add b.txt"
curl -s -X POST "$BASE/command" -d '{"command":"Diff main"}' >/dev/null 2>&1
wait_ready

clickpanel "$(dstate commits_x)" "$PILL_Y"; sleep 1.0
check "the commits pill switches view"     "$(dstate mode)"         "commits"
check "the review's two commits are listed" "$(dstate commit_rows)" "2"

# Row 0 is the newest commit ("add b.txt"). A left click scopes the diff to it.
CROW0_Y="$(python3 -c "print(int($(dstate body_top) + 20))")"
clickpanel 300 "$CROW0_Y"; sleep 0.5
wait_ready
check "clicking a commit scopes the diff"   "$(scope_kind)"       "commit"
check "the scoped diff is read-only"        "$(dstate editable)"  "false"
check "it shows only that commit's file"    "$(dstate files)"     "1"
check "the scoped load carried no error"    "$(dstate has_error)" "false"

# Right-click opens the context menu on that row. Its rows are 24px tall from
# 6px below the menu's top edge, which is the pointer: item 0 spans +6..+30,
# item 1 +30..+54, the separator +54..+63, and item 3 ("Whole review") +63..+87.
clickpanel "$(dstate commits_x)" "$PILL_Y"; sleep 0.5
rclickpanel 300 "$CROW0_Y"; sleep 0.5
check "right-click opens the context menu"  "$(dstate menu_open)" "true"

# "Everything since this commit" still ends at the working tree, so unlike
# "only this commit" it stays editable.
clickpanel 330 "$(python3 -c "print(int($CROW0_Y + 42))")"; sleep 0.5
wait_ready
check "the menu scopes to 'since this commit'" "$(scope_kind)"      "since"
check "a 'since' scope stays editable"         "$(dstate editable)" "true"

# And back to the whole review, which the menu offers only while scoped.
clickpanel "$(dstate commits_x)" "$PILL_Y"; sleep 0.5
rclickpanel 300 "$CROW0_Y"; sleep 0.5
clickpanel 330 "$(python3 -c "print(int($CROW0_Y + 72))")"; sleep 0.5
wait_ready
check "the menu returns to the whole review" "$(scope_kind)"      "whole"
check "the whole review is editable again"   "$(dstate editable)" "true"

# --- `/` searches the unified diff -------------------------------------------
# The prompt is the host's, opened from inside the region; the pattern searches
# that region's buffer and the cursor lands on the match.
clickpanel "$(dstate unified_x)" "$PILL_Y"; sleep 0.5
BODY_Y="$(python3 -c "print(int($(dstate body_top) + 3.5 * $CELL_H))")"
clickpanel "$(dstate unified_body_x)" "$BODY_Y"; sleep 0.3
key "/"
CMDLINE="$(curl -s "$BASE/state" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("command_line") or "")')"
check "/ opens the search prompt in a region" "$CMDLINE" "/"
for c in b e t a; do key "$c"; done
key "return"; sleep 0.4
check "a search that hits reports no error"   "$(status_error)" ""
key "/"; for c in z z z; do key "$c"; done; key "return"; sleep 0.4
check "a search that misses says so"          "$(refused "pattern not found")" "yes"

# --- report -----------------------------------------------------------------
echo
echo "passed: $pass   failed: $fail"
[ "$fail" -eq 0 ]
