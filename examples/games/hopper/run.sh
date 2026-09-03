#!/usr/bin/env bash
# Launch Hopper in the petal-sdl host (builds the host on first use).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
sdl="$repo/integrations/petal-desktop-sdl"
bin="$sdl/target/debug/petal-sdl"
if [ ! -x "$bin" ]; then ( cd "$sdl" && cargo build ); fi
exec "$bin" "$here/game.ptl" --width 960 --height 600 --title "Hopper" "$@"
