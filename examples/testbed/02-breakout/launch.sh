#!/usr/bin/env bash
# Launch 02 — Breakout.
# Runs from any working directory; extra arguments are passed to garden,
# e.g. ./launch.sh --headless --debug-port 0
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
garden="${GARDEN_BIN:-$here/../../../garden/target/debug/garden}"

if [ ! -x "$garden" ]; then
    echo "launch.sh: garden binary not found at $garden" >&2
    echo "launch.sh: build it, or point GARDEN_BIN at one" >&2
    exit 1
fi

export GARDEN_HEADLESS_SIZE="${GARDEN_HEADLESS_SIZE:-1100x780}"

cd -- "$here"
exec "$garden" --init layout.ptl "$@"
