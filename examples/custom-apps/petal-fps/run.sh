#!/usr/bin/env bash
# Launch petal-fps (the cyberpunk-city FPS demo).
#
# petal-fps is a Shape B sample app: it has its own binary that depends on the
# petal-sdl integration as a library. Building it builds petal-sdl transitively.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The SDL2 linker needs Homebrew's lib dir on macOS; harmless elsewhere.
if [ -d /opt/homebrew/lib ]; then
    export LIBRARY_PATH="${LIBRARY_PATH:-}:/opt/homebrew/lib"
fi

SCRIPT="${1:-examples/fps_game.ptl}"
cd "$SCRIPT_DIR"
cargo run --release -- "$SCRIPT" "${@:2}"
