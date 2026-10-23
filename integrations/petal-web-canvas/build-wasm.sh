#!/bin/bash
set -e
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
  echo "built=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$PKG/BUILD_INFO"
echo "WASM build complete → petal-web-canvas/pkg/"
