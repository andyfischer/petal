#!/bin/bash
# Build garden-web for the browser into ./pkg (wasm-bindgen ES module + .wasm),
# stamped with the Petal commit it was built from. petal-lang.org vendors it
# with `pnpm build:sdk`.
set -e
cd "$(dirname "$0")"
REPO_ROOT="$(git rev-parse --show-toplevel)"
wasm-pack build --target web --out-dir pkg
rm -f pkg/.gitignore pkg/package.json pkg/README.md
{
  echo "commit=$(git rev-parse HEAD)"
  if [ -n "$(git status --porcelain -- "$REPO_ROOT/core" "$REPO_ROOT/core-libs" "$REPO_ROOT/garden/garden-script" "$REPO_ROOT/garden/garden-web")" ]; then
    echo "dirty=true"
  else
    echo "dirty=false"
  fi
  echo "built=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > pkg/BUILD_INFO
echo "WASM build complete → garden/garden-web/pkg/"
