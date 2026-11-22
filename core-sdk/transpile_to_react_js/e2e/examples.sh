#!/bin/bash
# examples.sh <petal-to-react> <out-dir> <design-or-file>...: transpile each
# (as a project) and render its default export; one line per example.
# Files in <out-dir> are overwritten, never deleted.
bin="$1"; out="$2"; shift 2
here="$(cd "$(dirname "$0")" && pwd)"
for f in "$@"; do
  n=$(basename "${f%/}" .ptl)
  mkdir -p "$out/$n"
  if ! "$bin" "$f" "$out/$n" >/dev/null 2>"$out/$n.log"; then echo "$n FAIL transpile"; continue; fi
  entry=$(grep -l "export default" "$out/$n"/*.jsx | grep -v petal-design | head -1)
  node "$here/render.mjs" "$entry" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{const r=JSON.parse(s);console.log(process.argv[1], r.ok?"ok":"FAIL", r.elements??"", r.keyWarnings?`(${r.keyWarnings} key warnings)`:"", (r.error??"").split("\n")[0].slice(0,200), (r.errors??[]).slice(0,1).join("").slice(0,300))}catch(e){console.log(process.argv[1],"FAIL",s.slice(0,300))}})' "$n ($(grep -c warning "$out/$n.log") warnings)"
done
