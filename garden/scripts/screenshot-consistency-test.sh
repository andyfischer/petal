#!/usr/bin/env bash
#
# Integration test for the debug server's screenshot/frame consistency contract
# (docs/debug-server.md "Frame consistency"): a `/screenshot` taken immediately
# after injected input must capture a complete, steady frame that reflects that
# input — no sleeps, no retries — and must expose the captured frame number in
# an `X-Garden-Frame` header. `GET /frame` reports the same counter instantly.
#
# The fixture is a panel script that counts `k` presses but displays the count
# through a two-frame `state` chain (shown <- lag <- count). Without the
# settle-then-capture contract, a screenshot right after POST /key renders the
# panel's cached commands mid-propagation and shows a stale value; with it,
# panels are ticked to a fixed point before the scene is built. The test drives
# input -> screenshot back-to-back 10x and asserts the captured scene reflects
# every press, the same layered strategy as scripts/diff-review-integration-test.sh.
#
# Usage:  scripts/screenshot-consistency-test.sh
# Exit:   0 if every assertion passes, 1 otherwise.

set -uo pipefail

GARDEN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$GARDEN_DIR/target/debug/garden"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/garden-shot-it.XXXXXX")"
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
checkge() { # description  actual  threshold  (actual >= threshold)
  if [ "$2" -ge "$3" ] 2>/dev/null; then
    printf '  ok   %s\n' "$1"
    pass=$((pass + 1))
  else
    printf '  FAIL %s\n        got [%s] want [>=%s]\n' "$1" "$2" "$3"
    fail=$((fail + 1))
  fi
}

# --- fixture: a counter panel whose display lags its state by two frames ------
cat > "$WORK/counter.ptl" <<'EOF'
// Counts `k` presses. The displayed value propagates through a two-frame
// state chain (shown <- lag <- count), so a capture taken before panel
// frames settle shows a stale `shown` — exactly what the screenshot
// consistency contract must prevent. Every frame of the chain draws
// distinct content (count updates immediately), so the settle loop's
// fixed-point detection sees each propagation step.
state count = 0
state lag = 0
state shown = 0

if key_pressed("k") then
  count = count + 1
end

clear(16, 18, 26)
draw_text("count: " ++ str(count), 10, 10, 14, 230, 230, 230)
draw_text("lag: " ++ str(lag), 10, 30, 14, 200, 200, 200)
draw_text("shown: " ++ str(shown), 10, 50, 14, 200, 200, 200)
// A solid swatch whose red channel encodes `shown` (10 + shown*20), so the
// test can decode the captured PNG and assert on the *pixels* of the shot.
draw_rect(20, 80, 260, 160, 10 + shown * 20, 40, 90)

shown = lag
lag = count
EOF
cat > "$WORK/init.ptl" <<EOF
layout(panel("$WORK/counter.ptl"))
EOF

# Minimal PNG reader (RGBA8, non-interlaced — what the debug server emits):
# unfilters scanlines up to the requested row and prints "r g b" of one pixel.
cat > "$WORK/pixel.py" <<'EOF'
import struct, sys, zlib
path, sx, sy = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
data = open(path, 'rb').read()
assert data[:8] == b'\x89PNG\r\n\x1a\n', 'not a PNG'
pos, idat, w, h = 8, b'', None, None
while pos < len(data):
    ln, typ = struct.unpack('>I4s', data[pos:pos + 8]); pos += 8
    chunk = data[pos:pos + ln]; pos += ln + 4
    if typ == b'IHDR':
        w, h, depth, color, _, _, inter = struct.unpack('>IIBBBBB', chunk)
        assert depth == 8 and color == 6 and inter == 0, 'unexpected PNG format'
    elif typ == b'IDAT':
        idat += chunk
raw = zlib.decompress(idat)
stride = w * 4
prev = bytearray(stride)
i = 0
for y in range(h):
    f = raw[i]; i += 1
    line = bytearray(raw[i:i + stride]); i += stride
    if f == 1:
        for x in range(4, stride): line[x] = (line[x] + line[x - 4]) & 255
    elif f == 2:
        for x in range(stride): line[x] = (line[x] + prev[x]) & 255
    elif f == 3:
        for x in range(stride):
            a = line[x - 4] if x >= 4 else 0
            line[x] = (line[x] + ((a + prev[x]) >> 1)) & 255
    elif f == 4:
        for x in range(stride):
            a = line[x - 4] if x >= 4 else 0
            b = prev[x]
            c = prev[x - 4] if x >= 4 else 0
            p = a + b - c
            pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
            line[x] = (line[x] + pr) & 255
    if y == sy:
        o = sx * 4
        print(line[o], line[o + 1], line[o + 2])
        break
    prev = line
EOF

# --- launch --------------------------------------------------------------------
echo "building..."
( cd "$GARDEN_DIR" && cargo build -q -p garden-app ) || { echo "build failed"; exit 1; }

echo "launching headless garden with counter panel..."
"$BIN" --headless --debug-port 0 --init "$WORK/init.ptl" >"$LOG" 2>&1 &
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

# --- helpers --------------------------------------------------------------------
key() { curl -s -X POST "$BASE/key" -d "{\"key\":\"$1\"}" >/dev/null; }
kind() { curl -s "$BASE/state" | python3 -c "import sys,json;print(json.load(sys.stdin)['panes'][0]['kind'])"; }
# A value the panel bound this frame, off /state -> panes[].panel.values. Those
# values keep their real JSON types, so print strings bare and everything else as
# compact JSON: a plain print() of a JSON bool renders True/False and would
# silently fail every comparison against true/false. Absent keys print "None".
pstate() { curl -s "$BASE/state" | python3 -c "
import sys, json
missing = object()
v = (json.load(sys.stdin)['panes'][0]['panel'] or {}).get('values', {}).get('$1', missing)
print('None' if v is missing else v if isinstance(v, str) else json.dumps(v, separators=(',', ':')))"; }
state_frame() { curl -s "$BASE/state" | python3 -c "import sys,json;print(json.load(sys.stdin).get('frame'))"; }
frame_now() { curl -s "$BASE/frame" | python3 -c "import sys,json;print(json.load(sys.stdin).get('frame'))"; }
# Number of on-screen text runs whose text is exactly $1.
scene_count() { curl -s "$BASE/scene" | python3 -c "import sys,json;d=json.load(sys.stdin);print(sum(1 for p in d['primitives'] if p.get('type')=='text' and p['text']=='$1'))"; }
errcount() { curl -s "$BASE/scene" | python3 -c "import sys,json;d=json.load(sys.stdin);print(sum(1 for p in d['primitives'] if p.get('type')=='text' and 'error' in p['text'].lower()))"; }
# GET /screenshot, saving the PNG body and the X-Garden-Frame header value.
shot() { # png-path -> prints header frame (or "missing")
  local hdr="$WORK/headers.txt"
  curl -s -D "$hdr" "$BASE/screenshot" -o "$1"
  tr -d '\r' < "$hdr" | awk -F': ' 'tolower($1)=="x-garden-frame"{print $2; found=1} END{if(!found)print "missing"}'
}
png_magic() { head -c 4 "$1" | od -An -tx1 | tr -d ' \n'; }
# Decode the swatch pixel of a shot and map its red channel back to `shown`
# (drawn as 10 + shown*20; sRGB round-trip error is a couple of counts, so
# snap to the nearest step and reject anything further than 5 off).
png_shown() { # png-path -> the `shown` value the captured pixels encode
  python3 "$WORK/pixel.py" "$1" "$SWATCH_X" "$SWATCH_Y" | awk \
    '{step = int(($1 - 10) / 20.0 + 0.5); off = $1 - (10 + step * 20);
      if (off < -5 || off > 5) print "bad-red-" $1; else print step}'
}

# --- sanity ----------------------------------------------------------------------
echo "running assertions..."
check "pane is a panel"            "$(kind)"      "panel"
check "panel has no runtime error" "$(errcount)"  "0"
check "counter starts at 0"        "$(pstate count)" "0"

# Physical-pixel sample point inside the swatch (panel-local 20,80 + 260x160),
# from the pane rect and window scale reported by /state.
read -r SWATCH_X SWATCH_Y <<<"$(curl -s "$BASE/state" | python3 -c "
import sys, json
d = json.load(sys.stdin)
r, s = d['panes'][0]['rect'], d['window']['scale']
print(int((r['x'] + 20 + 130) * s), int((r['y'] + 80 + 80) * s))")"
echo "swatch sample point: $SWATCH_X,$SWATCH_Y"

# --- the contract: input then immediate screenshot, 10x, no sleeps ---------------
last_frame=0
for i in $(seq 1 10); do
  key k
  frame="$(shot "$WORK/shot-$i.png")"

  # The capture carries its frame number, monotonically increasing.
  checkge "shot $i: X-Garden-Frame present and increasing (frame=$frame)" "$frame" "$((last_frame + 1))"
  [ "$frame" != "missing" ] && last_frame="$frame"

  # The body is a real PNG.
  check "shot $i: body is a PNG" "$(png_magic "$WORK/shot-$i.png")" "89504e47"

  # The captured *pixels* reflect the press, including the swatch color that
  # needs two extra panel frames to propagate — the settle contract itself.
  check "shot $i: PNG pixels show settled shown: $i" "$(png_shown "$WORK/shot-$i.png")" "$i"

  # The captured (= settled) scene reflects the press, including the value that
  # needs two extra panel frames to propagate. /scene follows the same settle
  # contract, and no further input arrived, so it shows what the PNG shows.
  check "shot $i: scene shows count: $i" "$(scene_count "count: $i")" "1"
  check "shot $i: scene shows settled shown: $i" "$(scene_count "shown: $i")" "1"

  # /state agrees, and reports the global frame counter. At the settled fixed
  # point every link of the chain holds the same value, so the observed `shown`
  # binding matches the one that was drawn.
  check "shot $i: panel values settled" "$(pstate shown)" "$i"
  checkge "shot $i: /state frame >= capture frame" "$(state_frame)" "$frame"

  # /frame reports the current counter instantly (the poll target for clients).
  checkge "shot $i: /frame >= capture frame" "$(frame_now)" "$frame"
done

# --- report -----------------------------------------------------------------------
echo
echo "passed: $pass   failed: $fail"
[ "$fail" -eq 0 ]
