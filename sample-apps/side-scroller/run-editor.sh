#!/usr/bin/env bash
# Launch the level editor.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/../.." && pwd)"
SDL_DIR="$REPO/integrations/petal-desktop-sdl"
BIN="$SDL_DIR/target/debug/petal-sdl"
if [ ! -x "$BIN" ]; then
    echo "Building petal-desktop-sdl..."
    ( cd "$SDL_DIR" && cargo build )
fi
# cwd = sample-apps/ so the hard-coded "side-scroller/levels/..." paths resolve.
cd "$SCRIPT_DIR/.."
exec "$BIN" side-scroller/editor.ptl --width 960 --height 600 --title "Petal Level Editor"
