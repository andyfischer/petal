#!/usr/bin/env bash
# Test each .ptl example individually with a timeout.
# Usage: ./bin/test-each.sh [timeout_seconds]

cd "$(dirname "$0")/.."

TIMEOUT=${1:-3}
PETAL="rust/target/debug/petal"

cargo build --quiet --manifest-path rust/Cargo.toml 2>&1 | grep -v warning

PASS=0
FAIL=0
HANG=0

for f in examples/*.ptl; do
    name=$(basename "$f")
    result=$(timeout "$TIMEOUT" "$PETAL" "$f" 2>&1)
    code=$?
    if [ $code -eq 0 ]; then
        echo "PASS: $name"
        PASS=$((PASS + 1))
    elif [ $code -eq 124 ]; then
        echo "HANG: $name"
        HANG=$((HANG + 1))
    else
        echo "FAIL: $name -- $(echo "$result" | tail -1)"
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "Results: $PASS passed, $FAIL failed, $HANG hung (timeout ${TIMEOUT}s)"
