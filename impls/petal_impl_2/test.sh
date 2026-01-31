#!/bin/bash
# Test script for evaluating Petal code

PETAL="./target/release/petal"

if [ ! -f "$PETAL" ]; then
    echo "Building petal..."
    cargo build --release
fi

if [ $# -eq 0 ]; then
    # No arguments - run REPL
    exec $PETAL repl
elif [ -f "$1" ]; then
    # Argument is a file - run it
    exec $PETAL "$@"
else
    # Argument is code - create temp file and run it
    TMPFILE=$(mktemp /tmp/petal_test_XXXXXX.ptl)
    echo "$1" > "$TMPFILE"
    $PETAL "$TMPFILE"
    EXIT_CODE=$?
    rm "$TMPFILE"
    exit $EXIT_CODE
fi
