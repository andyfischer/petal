#!/bin/sh
# Petal uninstaller.
#
#   curl -fsSL https://petal-lang.org/uninstall.sh | sh
#   # or, if installed: ~/.petal/uninstall.sh
#
# Removes the petal binary, the install directory, and the PATH lines the
# installer added to your shell rc files.
#
# Environment overrides:
#   PETAL_INSTALL   install prefix to remove (default: $HOME/.petal)

set -eu

INSTALL_DIR="${PETAL_INSTALL:-$HOME/.petal}"
BIN_DIR="$INSTALL_DIR/bin"

if [ -t 1 ]; then
  B="$(printf '\033[1;35m')"; X="$(printf '\033[0m')"
else
  B=""; X=""
fi
info() { printf '%spetal%s %s\n' "$B" "$X" "$1"; }

if [ -e "$BIN_DIR/petal" ]; then
  rm -f "$BIN_DIR/petal"
  info "removed $BIN_DIR/petal"
else
  info "no petal binary found at $BIN_DIR/petal"
fi

# Remove the install tree if it only holds our own files.
rm -f "$INSTALL_DIR/uninstall.sh" 2>/dev/null || true
rmdir "$BIN_DIR" 2>/dev/null || true
rmdir "$INSTALL_DIR" 2>/dev/null || true

# Strip the PATH block the installer appended (the "# petal ..." comment line
# plus the following export/set line that mentions our bin dir).
for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" \
          "$HOME/.zprofile" "$HOME/.profile" \
          "$HOME/.config/fish/config.fish"; do
  [ -f "$rc" ] || continue
  if grep -Fqs "$BIN_DIR" "$rc"; then
    tmp="$(mktemp "${TMPDIR:-/tmp}/petal-rc.XXXXXX")"
    awk -v b="$BIN_DIR" '
      { line[NR] = $0 }
      END {
        for (i = 1; i <= NR; i++) {
          if (index(line[i], b) > 0) {
            drop[i] = 1
            if (i > 1 && index(line[i-1], "# petal") > 0) drop[i-1] = 1
          }
        }
        for (i = 1; i <= NR; i++) if (!(i in drop)) print line[i]
      }' "$rc" > "$tmp" && cat "$tmp" > "$rc" && rm -f "$tmp"
    info "removed PATH entry from $rc"
  fi
done

info "petal uninstalled. Open a new shell to refresh your PATH."
