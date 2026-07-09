#!/bin/bash
set -e
cd "$(dirname "$0")/rust"
wasm-pack build --target web
rm -rf ../pkg
cp -r pkg ../pkg
echo "WASM build complete → petal-web-canvas/pkg/"
