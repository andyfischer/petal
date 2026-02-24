#!/usr/bin/env bash
# Run all .ptl examples and show first few lines of output from each.
# Usage: ./bin/test-examples.sh [--full]

set -e

cd "$(dirname "$0")/.."

# Build first
cargo build --quiet --manifest-path rust/Cargo.toml

PETAL="rust/target/debug/petal"
FULL=false
if [ "$1" = "--full" ]; then
    FULL=true
fi

PASS=0
FAIL=0

for f in examples/*.ptl; do
    name=$(basename "$f")
    echo "=== $name ==="
    if output=$("$PETAL" "$f" 2>&1); then
        if [ "$FULL" = true ]; then
            echo "$output"
        else
            echo "$output" | head -8
            lines=$(echo "$output" | wc -l)
            if [ "$lines" -gt 8 ]; then
                echo "  ... ($lines lines total)"
            fi
        fi
        PASS=$((PASS + 1))
    else
        echo "FAILED: $output" | head -5
        FAIL=$((FAIL + 1))
    fi
    echo
done

echo "Results: $PASS passed, $FAIL failed"
