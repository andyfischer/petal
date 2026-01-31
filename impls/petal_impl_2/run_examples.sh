#!/usr/bin/env bash

# Script to run all Petal example files

set -e

echo "Building Petal..."
cargo build --quiet 2>&1 | grep -v "warning:" || true

echo ""
echo "Running all examples..."
echo "======================"
echo ""

for file in examples/*.ptl; do
    echo "=== $(basename "$file") ==="
    ./target/debug/petal "$file" 2>&1 | head -20
    echo ""
done

echo "======================"
echo "All examples completed!"
