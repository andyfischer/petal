#!/bin/bash
set -e
cd "$(dirname "$0")/../../rust"
wasm-pack build --target web --features wasm
# Copy pkg to petal-web
rm -rf ../apps/petal-web/pkg
cp -r pkg ../apps/petal-web/pkg
echo "WASM build complete → apps/petal-web/pkg/"
