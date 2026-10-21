#!/usr/bin/env bash
#
# Generate the application icons from web/public/pulse.svg.
#
# Outputs into crates/ui_resources/assets/app-icon/:
#   32x32.png 128x128.png 128x128@2x.png icon.png
#
# Rasterization prefers rsvg-convert or ImageMagick if present. Downscaling
# prefers ImageMagick too.
#
# Re-run this whenever assets/pulse.svg changes.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SVG="$ROOT/web/public/pulse.svg"
OUT="$ROOT/crates/ui_resources/assets/app-icon"
MASTER_SIZE=1024

mkdir -p "$OUT"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

master="$WORK/master.png"

# 1. Rasterize SVG -> master PNG at MASTER_SIZE.
if command -v rsvg-convert >/dev/null 2>&1; then
    rsvg-convert -w "$MASTER_SIZE" -h "$MASTER_SIZE" "$SVG" -o "$master"
elif command -v magick >/dev/null 2>&1; then
    magick -background none -density 384 "$SVG" -resize "${MASTER_SIZE}x${MASTER_SIZE}" "$master"
else
    echo "error: need rsvg-convert or magick to rasterize SVG" >&2
    exit 1
fi

# Downscale the master PNG to an arbitrary square size.
resize() { # <size> <dest>
    if command -v magick >/dev/null 2>&1; then
        magick "$master" -resize "${1}x${1}" "$2"
    elif command -v convert >/dev/null 2>&1; then
        convert "$master" -resize "${1}x${1}" "$2"
    else
        echo "error: need magick or convert to resize" >&2; exit 1
    fi
}

# 2. PNGs consumed directly by cargo-packager (pacman/deb).
resize 32   "$OUT/32x32.png"
resize 128  "$OUT/128x128.png"
resize 256  "$OUT/128x128@2x.png"
resize 256  "$OUT/256x256.png"
cp "$master" "$OUT/icon.png"

echo "Icons written to $OUT:"
ls -1 "$OUT"
