#!/usr/bin/env bash
# Test a single .ptl file or a snippet from a temp file.
# Usage: ./bin/test-snippet.sh <file.ptl>
#        ./bin/test-snippet.sh examples/closures.ptl

cd "$(dirname "$0")/.."

PETAL="rust-impl/target/debug/petal"
cargo build --quiet --manifest-path rust-impl/Cargo.toml 2>&1 | grep -v warning

"$PETAL" "$1" 2>&1
echo "Exit code: $?"
