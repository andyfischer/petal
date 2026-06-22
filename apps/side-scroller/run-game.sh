#!/usr/bin/env bash
# Launch the side-scroller game.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/petal-sdl/target/debug/petal-sdl"
if [ ! -x "$BIN" ]; then
    echo "Building petal-sdl..."
    ( cd "$ROOT/petal-sdl" && cargo build )
fi
cd "$ROOT"
exec "$BIN" side-scroller/game.ptl --width 960 --height 600 --title "Petal Runner"
