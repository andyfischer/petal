#!/bin/bash
# Print project statistics

echo "=== Petal Implementation Statistics ==="
echo ""

echo "Source Code:"
wc -l src/*.rs | tail -1

echo ""
echo "Sample Programs:"
find samples -name "*.ptl" | wc -l

echo ""
echo "Binary Size:"
ls -lh target/release/petal | awk '{print $5}'

echo ""
echo "Total Documentation:"
wc -l *.md | tail -1

echo ""
echo "Recent Additions:"
echo "- 18 sample programs (was 12)"
echo "- Variable binding with let"
echo "- State management"
echo "- User-defined functions"
echo "- Recursion support"
