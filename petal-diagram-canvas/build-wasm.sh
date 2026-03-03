#!/bin/bash
set -e
cd "$(dirname "$0")/../rust"
wasm-pack build --target web --features wasm
# Copy pkg to petal-diagram-canvas
rm -rf ../petal-diagram-canvas/pkg
cp -r pkg ../petal-diagram-canvas/pkg
echo "WASM build complete → petal-diagram-canvas/pkg/"
