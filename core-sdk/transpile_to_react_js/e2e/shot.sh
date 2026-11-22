#!/bin/bash
# shot.sh <entry.jsx> <export> <width> <height> <out.png>: render one export
# of a transpiled module to static markup and screenshot it in headless Chrome.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
html="$(mktemp -t petal-shot).html"
node "$here/render.mjs" "$1" --export "$2" --html ${PROPS} | node -e '
let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const r=JSON.parse(s);if(!r.ok){console.error(JSON.stringify(r));process.exit(1)}
process.stdout.write(`<!doctype html><meta charset=utf-8><style>body{margin:0}</style>${r.html}`)})' > "$html"
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu --hide-scrollbars \
  --window-size="$3,$4" --screenshot="$5" "file://$html" >/dev/null 2>&1
echo "$5"
