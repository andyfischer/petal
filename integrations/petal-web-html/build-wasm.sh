#!/bin/bash
set -e
cd "$(dirname "$0")/../../rust"
wasm-pack build --target web --features wasm
# Copy pkg to petal-web-html
rm -rf ../integrations/petal-web-html/pkg
cp -r pkg ../integrations/petal-web-html/pkg
echo "WASM build complete → integrations/petal-web-html/pkg/"
