#!/bin/bash
set -e
# --size: opt-level "z" for a page that cares more about the download than
# frame time (a decorative background). On petal-web-canvas, 2026-10-23:
#   default  3.69 MB raw, 1.29 MB gzip
#   --size   2.26 MB raw, 0.85 MB gzip, frames ~1.5x slower
PROFILE=speed
if [ "$1" = "--size" ]; then
  PROFILE=size
  export CARGO_PROFILE_RELEASE_OPT_LEVEL=z
fi
cd "$(dirname "$0")/rust"
REPO_ROOT="$(git rev-parse --show-toplevel)"
PKG=../pkg
wasm-pack build --target web
rm -rf ../pkg
cp -r pkg ../pkg
# Stamp the pkg with the Petal it was built from, so a project that vendors
# it can tell which one it has.
{
  echo "commit=$(git rev-parse HEAD)"
  if [ -n "$(git status --porcelain -- "$REPO_ROOT/core" "$REPO_ROOT/core-libs" "$REPO_ROOT/integrations")" ]; then
    echo "dirty=true"
  else
    echo "dirty=false"
  fi
  echo "profile=$PROFILE"
  echo "built=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$PKG/BUILD_INFO"
echo "WASM build complete → petal-web-canvas/pkg/"
