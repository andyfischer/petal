#!/bin/bash
# Count project metrics

echo "=== Petal Implementation Metrics ==="
echo ""

echo "Sample Programs:"
find samples -name "*.ptl" | wc -l

echo ""
echo "Source Code Lines:"
wc -l src/*.rs | tail -1

echo ""
echo "Total Documentation Lines:"
wc -l *.md | tail -1

echo ""
echo "Build Command:"
echo "  cargo build --release"

echo ""
echo "Test Command:"
echo "  ./test_samples.sh"
