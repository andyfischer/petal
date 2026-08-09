#!/usr/bin/env bash
#
# Build Garden as a macOS .app bundle with its Dock/Finder icon.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${GARDEN_PROFILE:-release}"

if [[ "$PROFILE" == "release" ]]; then
    CARGO_PROFILE_ARGS=(--release)
    TARGET_PROFILE_DIR="release"
else
    CARGO_PROFILE_ARGS=(--profile "$PROFILE")
    TARGET_PROFILE_DIR="$PROFILE"
fi

APP_DIR="$REPO_DIR/target/$TARGET_PROFILE_DIR/bundle/macos/Garden.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
ICON_SRC="$REPO_DIR/garden-app/assets/macos/Garden.icns"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: macOS app bundles can only be built on Darwin/macOS" >&2
    exit 1
fi

if [[ ! -f "$ICON_SRC" ]]; then
    echo "error: expected icon at $ICON_SRC" >&2
    exit 1
fi

echo "==> Building Garden binaries ($PROFILE)"
cargo build "${CARGO_PROFILE_ARGS[@]}" \
    -p garden-app \
    -p directory-browser \
    -p git-viewers \
    -p garden-diff \
    --manifest-path "$REPO_DIR/Cargo.toml"

# `git-viewers` produces `git-log` (`:Git`); `garden-diff` is the diff/review
# client behind `:Diff`, `:Review*`, `:PR`, `garden diff`, and `garden pr`.
for name in garden directory-browser git-log garden-diff; do
    built="$REPO_DIR/target/$TARGET_PROFILE_DIR/$name"
    if [[ ! -x "$built" ]]; then
        echo "error: expected built binary at $built" >&2
        exit 1
    fi
done

echo "==> Creating app bundle -> $APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"
for name in garden directory-browser git-log garden-diff; do
    install -m 0755 "$REPO_DIR/target/$TARGET_PROFILE_DIR/$name" "$MACOS_DIR/$name"
done
install -m 0644 "$ICON_SRC" "$RESOURCES_DIR/Garden.icns"

cat > "$CONTENTS_DIR/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Garden</string>
  <key>CFBundleExecutable</key>
  <string>garden</string>
  <key>CFBundleIconFile</key>
  <string>Garden</string>
  <key>CFBundleIdentifier</key>
  <string>com.andyfischer.garden</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Garden</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

printf 'APPL????' > "$CONTENTS_DIR/PkgInfo"

echo "Done. Open $APP_DIR"
