#!/bin/bash
set -e
cd "$(dirname "$0")/../../core"
REPO_ROOT="$(git rev-parse --show-toplevel)"
PKG=../integrations/petal-web-html/pkg
wasm-pack build --target web --features wasm
# Copy pkg to petal-web-html
rm -rf ../integrations/petal-web-html/pkg
cp -r pkg ../integrations/petal-web-html/pkg
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
echo "WASM build complete → integrations/petal-web-html/pkg/"
