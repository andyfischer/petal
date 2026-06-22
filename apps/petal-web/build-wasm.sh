#!/bin/bash
set -e
cd "$(dirname "$0")/../rust"
wasm-pack build --target web --features wasm
# Copy pkg to petal-web
rm -rf ../petal-web/pkg
cp -r pkg ../petal-web/pkg
echo "WASM build complete → petal-web/pkg/"
