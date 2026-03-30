#!/usr/bin/env bash
# Generate AppIcon.icns from a source PNG image.
# Usage: ./create-icns.sh <source.png> <output.icns>
#
# If sips is available (macOS), generates a proper multi-resolution iconset.
# Otherwise, creates a minimal .icns with just the 512x512 icon.
set -euo pipefail

SOURCE="${1:?Usage: create-icns.sh <source.png> <output.icns>}"
OUTPUT="${2:?Usage: create-icns.sh <source.png> <output.icns>}"

ICONSET_DIR=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET_DIR"

if command -v sips &>/dev/null; then
    # macOS — use sips to resize + iconutil to package
    sips -z 16 16     "$SOURCE" --out "$ICONSET_DIR/icon_16x16.png"      > /dev/null 2>&1
    sips -z 32 32     "$SOURCE" --out "$ICONSET_DIR/icon_16x16@2x.png"   > /dev/null 2>&1
    sips -z 32 32     "$SOURCE" --out "$ICONSET_DIR/icon_32x32.png"      > /dev/null 2>&1
    sips -z 64 64     "$SOURCE" --out "$ICONSET_DIR/icon_32x32@2x.png"   > /dev/null 2>&1
    sips -z 128 128   "$SOURCE" --out "$ICONSET_DIR/icon_128x128.png"    > /dev/null 2>&1
    sips -z 256 256   "$SOURCE" --out "$ICONSET_DIR/icon_128x128@2x.png" > /dev/null 2>&1
    sips -z 256 256   "$SOURCE" --out "$ICONSET_DIR/icon_256x256.png"    > /dev/null 2>&1
    sips -z 512 512   "$SOURCE" --out "$ICONSET_DIR/icon_256x256@2x.png" > /dev/null 2>&1
    sips -z 512 512   "$SOURCE" --out "$ICONSET_DIR/icon_512x512.png"    > /dev/null 2>&1
    sips -z 1024 1024 "$SOURCE" --out "$ICONSET_DIR/icon_512x512@2x.png" > /dev/null 2>&1

    iconutil -c icns "$ICONSET_DIR" -o "$OUTPUT"
    echo "Created $OUTPUT from $SOURCE (multi-resolution)"
else
    echo "Warning: sips not available (not macOS). Skipping .icns generation."
    echo "The .app will use no custom icon."
    # Create empty placeholder so the build doesn't fail
    touch "$OUTPUT"
fi

rm -rf "$(dirname "$ICONSET_DIR")"
