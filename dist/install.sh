#!/bin/sh
# Petal installer.
#
#   curl -fsSL https://petal-lang.org/install.sh | sh
#
# Downloads a prebuilt `petal` binary for your platform from the latest GitHub
# release and installs it to ~/.petal/bin, adding that directory to your PATH.
#
# Environment overrides:
#   PETAL_INSTALL        install prefix (default: $HOME/.petal)
#                        the binary is placed at $PETAL_INSTALL/bin/petal
#   PETAL_VERSION        version tag to install, e.g. v0.1.0 (default: latest)
#   PETAL_RELEASE_BASE   releases base URL (default: GitHub releases)
#   PETAL_NO_MODIFY_PATH set to 1 to skip editing your shell rc file
#
# Uninstall at any time with:
#   curl -fsSL https://petal-lang.org/uninstall.sh | sh
#   # or: ~/.petal/uninstall.sh

set -eu

REPO="andyfischer/petal"
RELEASE_BASE="${PETAL_RELEASE_BASE:-https://github.com/${REPO}/releases}"
INSTALL_DIR="${PETAL_INSTALL:-$HOME/.petal}"
BIN_DIR="$INSTALL_DIR/bin"

# --- pretty output ----------------------------------------------------------
if [ -t 1 ]; then
  B="$(printf '\033[1;35m')"; Y="$(printf '\033[1;33m')"
  R="$(printf '\033[1;31m')"; X="$(printf '\033[0m')"
else
  B=""; Y=""; R=""; X=""
fi
info() { printf '%spetal%s %s\n' "$B" "$X" "$1"; }
warn() { printf '%spetal%s %s\n' "$Y" "$X" "$1" >&2; }
err()  { printf '%spetal error:%s %s\n' "$R" "$X" "$1" >&2; exit 1; }

# --- pick a downloader ------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  dl() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  dl() { wget -qO "$2" "$1"; }
else
  err "need curl or wget installed to download Petal."
fi

# --- detect platform --------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin)
    case "$arch" in
      arm64 | aarch64) target="aarch64-apple-darwin" ;;
      x86_64)          target="x86_64-apple-darwin" ;;
      *) err "unsupported macOS architecture: $arch" ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64 | amd64)  target="x86_64-unknown-linux-musl" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-musl" ;;
      *) err "unsupported Linux architecture: $arch" ;;
    esac ;;
  *)
    err "unsupported OS: $os. Petal ships prebuilt binaries for macOS and Linux; build from source at https://github.com/${REPO}" ;;
esac

asset="petal-${target}.tar.gz"
if [ -n "${PETAL_VERSION:-}" ]; then
  url="${RELEASE_BASE}/download/${PETAL_VERSION}/${asset}"
  ver_label="$PETAL_VERSION"
else
  url="${RELEASE_BASE}/latest/download/${asset}"
  ver_label="latest"
fi

info "installing petal ($target, $ver_label)"

# --- download into a temp dir -----------------------------------------------
tmp="$(mktemp -d "${TMPDIR:-/tmp}/petal-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

info "downloading $url"
dl "$url" "$tmp/$asset" || err "download failed: $url"

# --- verify checksum --------------------------------------------------------
if dl "${url}.sha256" "$tmp/$asset.sha256" 2>/dev/null; then
  ( cd "$tmp"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum -c "$asset.sha256" >/dev/null 2>&1 || err "checksum verification failed"
    elif command -v shasum >/dev/null 2>&1; then
      shasum -a 256 -c "$asset.sha256" >/dev/null 2>&1 || err "checksum verification failed"
    else
      warn "no sha256 tool found; skipping checksum verification"
    fi )
  info "checksum verified"
else
  warn "no checksum published for this asset; skipping verification"
fi

# --- unpack -----------------------------------------------------------------
tar -xzf "$tmp/$asset" -C "$tmp" || err "failed to extract $asset"
if [ -f "$tmp/petal-${target}/petal" ]; then
  src_bin="$tmp/petal-${target}/petal"
elif [ -f "$tmp/petal" ]; then
  src_bin="$tmp/petal"
else
  src_bin="$(find "$tmp" -type f -name petal 2>/dev/null | head -1)"
fi
[ -n "${src_bin:-}" ] && [ -f "$src_bin" ] || err "petal binary not found in archive"

# --- install ----------------------------------------------------------------
mkdir -p "$BIN_DIR"
if ! install -m 755 "$src_bin" "$BIN_DIR/petal" 2>/dev/null; then
  cp "$src_bin" "$BIN_DIR/petal"
  chmod 755 "$BIN_DIR/petal"
fi
info "installed petal -> $BIN_DIR/petal"

# --- drop a matching uninstaller --------------------------------------------
# Kept in sync with dist/uninstall.sh (also hosted at petal-lang.org/uninstall.sh).
cat > "$INSTALL_DIR/uninstall.sh" <<'UNINSTALL_EOF'
#!/bin/sh
# Petal uninstaller. Removes the binary, install dir, and PATH edits.
set -eu
INSTALL_DIR="${PETAL_INSTALL:-$HOME/.petal}"
BIN_DIR="$INSTALL_DIR/bin"
info() { printf 'petal %s\n' "$1"; }
if [ -e "$BIN_DIR/petal" ]; then rm -f "$BIN_DIR/petal"; info "removed $BIN_DIR/petal"; fi
rm -f "$INSTALL_DIR/uninstall.sh" 2>/dev/null || true
rmdir "$BIN_DIR" 2>/dev/null || true
rmdir "$INSTALL_DIR" 2>/dev/null || true
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
UNINSTALL_EOF
chmod 755 "$INSTALL_DIR/uninstall.sh" 2>/dev/null || true

# --- ensure BIN_DIR is on PATH ----------------------------------------------
case ":${PATH}:" in
  *":$BIN_DIR:"*) on_path=1 ;;
  *) on_path=0 ;;
esac

ensure_path_in() {
  rc="$1"; line="$2"
  if [ -f "$rc" ] && grep -Fqs "$BIN_DIR" "$rc"; then
    return 0
  fi
  mkdir -p "$(dirname "$rc")"
  printf '\n# petal (added by install.sh)\n%s\n' "$line" >> "$rc"
  info "added petal to PATH in $rc"
}

if [ "$on_path" = "1" ]; then
  info "$BIN_DIR is already on your PATH"
elif [ "${PETAL_NO_MODIFY_PATH:-0}" = "1" ]; then
  warn "skipped PATH edit (PETAL_NO_MODIFY_PATH=1); add this to your shell rc:"
  warn "  export PATH=\"$BIN_DIR:\$PATH\""
else
  posix_line="export PATH=\"$BIN_DIR:\$PATH\""
  case "$(basename "${SHELL:-sh}")" in
    zsh)  ensure_path_in "$HOME/.zshrc" "$posix_line" ;;
    bash) ensure_path_in "$HOME/.bashrc" "$posix_line" ;;
    fish) ensure_path_in "$HOME/.config/fish/config.fish" "set -gx PATH \"$BIN_DIR\" \$PATH" ;;
    *)    ensure_path_in "$HOME/.profile" "$posix_line" ;;
  esac
  warn "restart your shell, or run:  export PATH=\"$BIN_DIR:\$PATH\""
fi

# --- confirm ----------------------------------------------------------------
info "done. try:  petal --help"
if [ -x "$BIN_DIR/petal" ]; then
  "$BIN_DIR/petal" --version 2>/dev/null || true
fi
