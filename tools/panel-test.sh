#!/usr/bin/env bash
# Shell helpers for driving a headless Garden panel through its debug server:
# the click / key / tick / obs / shot loop every panel-app session needs.
#
# Two ways to use it, same commands either way:
#
#   source tools/panel-test.sh          # then: click 80 30; tick 60; obs
#   tools/panel-test.sh click 80 30     # one command per call (agent harnesses,
#                                       # where every tool call is a new shell)
#
#   panel_start <slug> [garden args]    launch tools/run-example.ts headless on
#                                       a free port, wait until it answers
#   panel_stop                          kill the Garden holding the port
#   panel_port                          print the debug port in use
#
#   click  X Y [JSON]                   mouse ops, in PANE-LOCAL pixels: the
#   rclick X Y                          coordinates the script reads as
#   dclick X Y                          mouse_x()/mouse_y() and draws with
#   move   X Y
#   drag   X1 Y1 X2 Y2 [JSON]
#   scroll X Y LINES
#   key     NAME [MOD...]               key cmd-chord: key s cmd
#   keydown NAME / keyup NAME           hold a key across ticks
#   typetext STRING
#
#   tick [N] [DT]                       N frames of exactly DT s (1, 1/60)
#   panel_reset [SEED]                  restart the panel, dropping `state`
#
#   obs [NAMES|PREFIX_]                 panel values as JSON: `obs` is every
#                                       obs_* value, `obs score,lives` those
#                                       names, `obs ui_` that prefix, `obs all`
#                                       everything. Exits 1 and prints the
#                                       error if the panel has one
#   err                                 status_error (empty when clean)
#   shot [FILE] [window]                PNG of the pane (or the whole window)
#   find_text STRING                    pane-local rect + center of text runs
#   state [JQ FILTER]                   raw /state
#
# [JSON] is merged into the /mouse body: click 80 30 '{"hover_first":true}'
#
# Environment:
#   PORT       the debug port. Unset: read from $PANEL_LOG
#   PANEL_LOG  the log panel_start writes (default ./log.txt)
#   PANE       which pane mouse ops, obs and shot address (default 0)
#
# Mouse ops send {"pane": $PANE}, so Garden adds the pane's offset itself. On
# a Garden without that option (no feature `debug.mouse-pane` in /version) the
# offset is read from /state and added here instead.
#
# Needs curl and jq. Works sourced from bash or zsh.

_panel_root() {
  if [ -n "${PANEL_TEST_ROOT:-}" ]; then
    printf '%s\n' "$PANEL_TEST_ROOT"
  else
    git rev-parse --show-toplevel 2>/dev/null || pwd
  fi
}

panel_port() {
  if [ -n "${PORT:-}" ]; then
    printf '%s\n' "$PORT"
    return 0
  fi
  local log="${PANEL_LOG:-log.txt}" port=""
  if [ -f "$log" ]; then
    port=$(grep -o '127\.0\.0\.1:[0-9][0-9]*' "$log" | head -n 1 | cut -d: -f2)
  fi
  if [ -z "$port" ]; then
    echo "panel-test: no debug port: set PORT, or run panel_start (looked in $log)" >&2
    return 1
  fi
  printf '%s\n' "$port"
}

# _panel_get PATH [curl args] / _panel_post PATH JSON
_panel_get() {
  # Not `path`: zsh ties that name to $PATH.
  local port route="$1"
  port=$(panel_port) || return 1
  shift
  curl -sS "$@" "127.0.0.1:$port$route"
}

_panel_post() {
  local port
  port=$(panel_port) || return 1
  curl -sS -X POST "127.0.0.1:$port$1" -d "$2"
}

# Print a reply; a reply that is not {"ok": true, …} goes to stderr and fails.
_panel_ack() {
  local reply
  reply=$(cat)
  if printf '%s' "$reply" | jq -e '.ok == true' >/dev/null 2>&1; then
    printf '%s\n' "$reply" | jq -c "${1:-.}"
  else
    printf 'panel-test: %s\n' "$reply" >&2
    return 1
  fi
}

panel_start() {
  if [ $# -lt 1 ]; then
    echo "usage: panel_start <slug> [garden args...]" >&2
    return 2
  fi
  local root log="${PANEL_LOG:-log.txt}" port="" i=0
  root=$(_panel_root)
  : > "$log"
  # The subshell + nohup is what lets Garden outlive the calling shell.
  (nohup "$root/tools/run-example.ts" "$@" --headless --debug-port 0 > "$log" 2>&1 < /dev/null &)
  while [ $i -lt 150 ]; do
    port=$(grep -o '127\.0\.0\.1:[0-9][0-9]*' "$log" | head -n 1 | cut -d: -f2)
    if [ -n "$port" ] && curl -s -o /dev/null "127.0.0.1:$port/frame"; then
      echo "PORT=$port   (log: $log)"
      PORT="$port" err
      return 0
    fi
    # run-example.ts exits 3 on a stale Garden binary, 1 on a bad slug.
    if grep -q 'run-example:\|STALE GARDEN BINARY\|shutting down' "$log"; then
      break
    fi
    sleep 0.1
    i=$((i + 1))
  done
  echo "panel-test: Garden did not come up; $log says:" >&2
  cat "$log" >&2
  return 1
}

panel_stop() {
  local port pid
  port=$(panel_port) || return 1
  # By port, never by name: other Gardens on this machine are not ours.
  pid=$(lsof -ti "tcp:$port" -sTCP:LISTEN 2>/dev/null)
  if [ -z "$pid" ]; then
    echo "panel-test: nothing is listening on $port" >&2
    return 1
  fi
  kill $pid && echo "stopped pid $pid (port $port)"
}

# _panel_mouse OP X Y [EXTRA_JSON]  — X/Y (and EXTRA's "to") are pane-local.
_panel_mouse() {
  local op="$1" x="$2" y="$3" extra="${4:-}" pane="${PANE:-0}" body
  [ -n "$extra" ] || extra='{}'
  body=$(jq -cn --arg op "$op" --argjson x "$x" --argjson y "$y" \
    --argjson pane "$pane" --argjson extra "$extra" \
    '{op: $op, x: $x, y: $y, pane: $pane} + $extra') || return 2
  if ! _panel_get /version | jq -e '.features | index("debug.mouse-pane")' >/dev/null 2>&1; then
    # An older Garden ignores "pane": add the pane's origin here.
    local origin
    origin=$(_panel_get /state | jq -c --argjson pane "$pane" '.panes[$pane].rect | {x, y}') || return 1
    if [ -z "$origin" ] || [ "$origin" = '{"x":null,"y":null}' ]; then
      echo "panel-test: no pane $pane" >&2
      return 1
    fi
    body=$(printf '%s' "$body" | jq -c --argjson o "$origin" \
      'del(.pane) | .x += $o.x | .y += $o.y
       | if .to then .to.x += $o.x | .to.y += $o.y else . end')
  fi
  _panel_post /mouse "$body" | _panel_ack '{ok}'
}

click()  { _panel_mouse click "$1" "$2" "${3:-}"; }
rclick() { _panel_mouse click "$1" "$2" '{"button":1}'; }
dclick() { _panel_mouse click "$1" "$2" '{"clicks":2}'; }
move()   { _panel_mouse move "$1" "$2"; }
scroll() { _panel_mouse scroll "$1" "$2" "{\"lines\":$3}"; }
drag() {
  local extra="${5:-}"
  [ -n "$extra" ] || extra='{}'
  _panel_mouse drag "$1" "$2" \
    "$(jq -cn --argjson x "$3" --argjson y "$4" --argjson e "$extra" '{to: {x: $x, y: $y}} + $e')"
}

_panel_key() {
  local op="$1" name="$2" body
  shift 2
  body=$(jq -cn --arg key "$name" --arg op "$op" '{key: $key, mods: $ARGS.positional}
    + (if $op == "tap" then {} else {op: $op} end)' --args "$@") || return 2
  _panel_post /key "$body" | _panel_ack '{ok}'
}

key()     { _panel_key tap "$@"; }
keydown() { _panel_key down "$@"; }
keyup()   { _panel_key up "$@"; }

typetext() {
  _panel_post /text "$(jq -cn --arg text "$1" '{text: $text}')" | _panel_ack '{ok}'
}

# tick [N] [DT]: N frames of exactly DT seconds. After the first tick the
# panel's time() and dt() are virtual: only ticks move them.
tick() {
  local n="${1:-1}" body
  if [ -n "${2:-}" ]; then
    body="{\"n\":$n,\"dt\":$2}"
  else
    body="{\"n\":$n}"
  fi
  _panel_post /tick "$body" | _panel_ack '{ok, panel_frames, clocks}'
}

panel_reset() {
  local body='{}'
  [ -z "${1:-}" ] || body="{\"seed\":$1}"
  _panel_post /panel/reset "$body" | _panel_ack
}

err() {
  local e state
  state=$(_panel_get '/state?values=none') || return 1
  e=$(printf '%s' "$state" | jq -r '.status_error // empty') || return 1
  if [ -n "$e" ]; then
    printf '%s\n' "$e" >&2
    return 1
  fi
}

obs() {
  local what="${1:-obs_}" query pane="${PANE:-0}" state
  case "$what" in
    all) query="" ;;
    *_)  query="?values_prefix=$what" ;;
    *)   query="?values=$what" ;;
  esac
  state=$(_panel_get "/state$query") || return 1
  printf '%s' "$state" | jq --argjson pane "$pane" '.panes[$pane].panel.values'
  if printf '%s' "$state" | jq -e '.status_error != null' >/dev/null; then
    printf '%s' "$state" | jq -r '"panel-test: status_error: \(.status_error)"' >&2
    return 1
  fi
  if printf '%s' "$state" | jq -e --argjson pane "$pane" '.panes[$pane].panel.values_stale == true' >/dev/null; then
    echo "panel-test: values are stale (the last frame raised)" >&2
    return 1
  fi
}

shot() {
  local file="${1:-shot.png}" query="?pane=${PANE:-0}"
  [ "${2:-}" != "window" ] || query=""
  _panel_get "/screenshot$query" -f -o "$file" && echo "$file"
}

find_text() {
  local port
  port=$(panel_port) || return 1
  curl -sS -G "127.0.0.1:$port/scene" --data-urlencode "pane=${PANE:-0}" \
    --data-urlencode "find=text~:$1" | jq -c '.primitives[]? | {text, rect, center}'
}

state() {
  _panel_get /state | jq "${1:-.}"
}

# Run as a command (`tools/panel-test.sh click 80 30`) rather than sourced.
_panel_sourced=0
if [ -n "${ZSH_VERSION:-}" ]; then
  case "${ZSH_EVAL_CONTEXT:-}" in *:file*) _panel_sourced=1 ;; esac
elif [ -n "${BASH_VERSION:-}" ] && [ "${BASH_SOURCE[0]}" != "$0" ]; then
  _panel_sourced=1
fi
if [ "$_panel_sourced" = 0 ]; then
  PANEL_TEST_ROOT="${PANEL_TEST_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
  case "${1:-}" in
    panel_start | panel_stop | panel_port | panel_reset | click | rclick | dclick | move | drag | \
      scroll | key | keydown | keyup | typetext | tick | err | obs | shot | find_text | state)
      "$@"
      ;;
    start | stop | port | reset)
      cmd="panel_$1"
      shift
      "$cmd" "$@"
      ;;
    *)
      sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
      [ -n "${1:-}" ] && [ "$1" != "help" ] && [ "$1" != "--help" ] && exit 2
      exit 0
      ;;
  esac
fi
