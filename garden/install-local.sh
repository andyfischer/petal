#!/usr/bin/env bash
#
# install-local.sh — build Garden and install it for the current user.
#
# Installs `garden` plus the GPP clients it spawns (`directory-browser`,
# `git-log`, `garden-diff`) into cargo's bin dir via `cargo install`, then seeds
# a personal config directory at ~/.garden with a default init.ptl. garden
# resolves the GPP clients next to its own executable, so installing them all
# through cargo keeps them together. After this, typing `garden` anywhere opens
# the GUI.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"

# garden resolves the GPP client binaries next to its own executable, so all of
# them go into the same (cargo) bin dir. `git-viewers` produces `git-log`
# (`:Git`); `garden-diff` is the one diff/review client (`:Diff`, `:Review*`,
# `:PR`, `garden diff`, `garden pr`).
echo "==> Installing garden + GPP clients (release) → $CARGO_BIN"
for crate_dir in garden-app gpp-apps/directory-browser gpp-apps/git-viewers gpp-apps/garden-diff; do
    cargo install --path "$REPO_DIR/$crate_dir" --force
done

# The `pr-browser` and `git-diff` clients were replaced by `garden-diff`; drop
# any copies a previous install left behind so nothing stale sits on $PATH.
for retired in pr-browser git-diff; do
    if [ -e "$CARGO_BIN/$retired" ]; then
        echo "==> Removing retired client $CARGO_BIN/$retired (replaced by garden-diff)"
        rm -f "$CARGO_BIN/$retired"
    fi
done

echo "==> Setting up config dir → $HOME/.garden"
"$CARGO_BIN/garden" setup initialize-config-if-missing

echo
echo "Done. Run 'garden' to open the GUI."
