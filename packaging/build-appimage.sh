#!/usr/bin/env bash
#
# Build the AppImage for an already-built release binary.
#
#   packaging/build-appimage.sh <version> [binary] [output-dir]
#
# <version>   the release tag or plain version ("v5.8.1" or "5.8.1")
# [binary]    defaults to target/release/pulse
# [out-dir]   defaults to the repository root
#
# The tools (linuxdeploy, appimagetool) are official AppImages themselves and
# are cached under target/appimage-tools. They run with
# APPIMAGE_EXTRACT_AND_RUN=1 so the build works in a container without FUSE.
#
# What gets bundled: the shared libraries `ldd` reports, minus the graphics
# stack, which has to match the host driver. Wayland and fontconfig are loaded
# at runtime rather than linked, so they come from the host too; the archive
# README says so.
set -euo pipefail

VERSION="${1:?usage: build-appimage.sh <version> [binary] [output-dir]}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="${2:-$ROOT/target/release/pulse}"
OUT="${3:-$ROOT}"
TOOLS="${PULSE_APPIMAGE_TOOLS:-$ROOT/target/appimage-tools}"

[ -x "$BINARY" ] || {
    echo "error: no release binary at $BINARY (run: cargo build --release -p pulse)" >&2
    exit 1
}

mkdir -p "$TOOLS" "$OUT"
cd "$ROOT"

fetch() {
    local url="$1" dest="$2"
    if [ -x "$dest" ]; then
        return
    fi
    echo "==> fetching $(basename "$dest")"
    curl --fail --location --silent --show-error --retry 3 --output "$dest.part" "$url"
    mv "$dest.part" "$dest"
    chmod +x "$dest"
}

LINUXDEPLOY="$TOOLS/linuxdeploy-x86_64.AppImage"
APPIMAGETOOL="$TOOLS/appimagetool-x86_64.AppImage"
fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage" "$LINUXDEPLOY"
fetch "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" "$APPIMAGETOOL"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
APPDIR="$STAGE/Pulse.AppDir"
# linuxdeploy names the deployed icon after the file it is given, and the
# desktop entry asks for "pulse". Give it a file with that name.
ICON="$STAGE/pulse.png"
install -m 644 "$ROOT/crates/ui_resources/assets/app-icon/256x256.png" "$ICON"

echo "==> collecting $BINARY and its libraries"
APPIMAGE_EXTRACT_AND_RUN=1 "$LINUXDEPLOY" \
    --appdir "$APPDIR" \
    --executable "$BINARY" \
    --desktop-file "$ROOT/packaging/pulse.desktop" \
    --icon-file "$ICON"

# linuxdeploy generates an AppRun for Qt; ours is the whole entry point, so it
# goes over the top.
install -m 755 "$ROOT/packaging/AppRun" "$APPDIR/AppRun"

NAME="Pulse-$VERSION-x86_64.AppImage"
echo "==> packing $NAME"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" \
    --no-appstream \
    "$APPDIR" "$OUT/$NAME"

echo "    $(du -h "$OUT/$NAME" | cut -f1)  $OUT/$NAME"
