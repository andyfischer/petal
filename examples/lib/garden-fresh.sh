# Sourced by every example's launch.sh: refuse to start a Garden binary that is
# behind this checkout.
#
# A garden/target/debug/garden left over from an earlier commit still launches,
# and then every language or host fix since looks unfixed. Garden prints a
# warning for this at startup, but a launch that goes to a log file
# (`./launch.sh --headless … > log.txt 2>&1 &`) hides it, so the launcher stops
# instead.
#
#   garden_require_fresh <garden binary>
#
# Returns 0 when the binary is current, or when it cannot be told (no git, a
# binary with no build stamp). Exits 3 when it is stale. GARDEN_ALLOW_STALE=1
# turns the stop into a banner and launches anyway.
#
# "Stale" is Garden's own rule (garden-app/src/version.rs, SOURCE_PATHSPECS —
# keep the list below in step with it): source files under garden/, petal-ui/
# or rust/ differ between the commit the binary was built from and HEAD.
# Uncommitted edits are not counted.

garden_require_fresh() {
    local garden="$1" root built head changed reason
    root="$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel 2>/dev/null)" || return 0
    # First line of --version: "garden 0.1.0 (87bbf1c 2026-10-07, built …)".
    built="$("$garden" --version 2>/dev/null | sed -n '1s/^[^(]*(\([0-9a-f]\{7,40\}\)[ ,)].*/\1/p')" || true
    [ -n "$built" ] || return 0
    head="$(git -C "$root" rev-parse --short HEAD 2>/dev/null)" || return 0

    if changed="$(git -C "$root" diff --name-only "$built" HEAD -- \
            ':(top)garden' ':(top)petal-ui' ':(top)rust' \
            ':(top,exclude,glob)**/*.md' ':(top,exclude)garden/tools' 2>/dev/null)"; then
        [ -n "$changed" ] || return 0
        reason="$(printf '%s\n' "$changed" | wc -l | tr -d ' ') source file(s) changed since"
    else
        reason="that commit is not in this checkout"
    fi

    {
        echo
        echo "################################################################"
        echo "##  STALE GARDEN BINARY"
        echo "##"
        echo "##  $garden"
        echo "##  was built from $built; the checkout is at $head"
        echo "##  ($reason)."
        echo "##"
        echo "##  Rebuild it:   (cd $root/garden && cargo build)"
        if [ "${GARDEN_ALLOW_STALE:-}" = 1 ]; then
            echo "##"
            echo "##  GARDEN_ALLOW_STALE=1: launching it anyway. You are testing old code."
        else
            echo "##  Or run it as it is:   GARDEN_ALLOW_STALE=1 ./launch.sh …"
            echo "##"
            echo "##  NOT LAUNCHED."
        fi
        echo "################################################################"
        echo
    } >&2
    [ "${GARDEN_ALLOW_STALE:-}" = 1 ] || exit 3
}
