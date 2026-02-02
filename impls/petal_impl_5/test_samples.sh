#!/bin/bash
# Test all sample Petal scripts

set -e

BINARY="./target/release/petal"

if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found. Run 'cargo build --release' first."
    exit 1
fi

echo "Testing all Petal samples..."
echo ""

for sample in samples/*.ptl; do
    echo "=== Testing: $sample ==="
    "$BINARY" "$sample" 2>&1 || true
    echo ""
done

echo "All tests completed!"
