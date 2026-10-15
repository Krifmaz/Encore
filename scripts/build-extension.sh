#!/usr/bin/env bash
# Pack the Encore YouTube sign-in extension: Encore-youtube-extension-<version>.zip with a chrome/ folder
# (Chrome, Edge, Brave: Load unpacked) and a firefox/ folder (about:debugging → Load Temporary Add-on).
# Usage: scripts/build-extension.sh <version> [out dir]
set -euo pipefail
V="${1:?version (MAJOR.MINOR.PATCH)}"
OUT="${2:-.}"
SRC="$(cd "$(dirname "$0")/../extension/encore-youtube-cookies" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
for b in chrome firefox; do
  mkdir -p "$TMP/$b"
  cp "$SRC/background.js" "$SRC/options.html" "$SRC/options.js" "$SRC/icon.png" "$SRC/README.md" "$TMP/$b/"
  m="$SRC/manifest.json"; [ "$b" = firefox ] && m="$SRC/manifest.firefox.json"
  sed "s/\"version\": \"0.0.0\"/\"version\": \"$V\"/" "$m" > "$TMP/$b/manifest.json"
done
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
rm -f "$OUT/Encore-youtube-extension-$V.zip"
(cd "$TMP" && zip -qr "$OUT/Encore-youtube-extension-$V.zip" chrome firefox)
echo "$OUT/Encore-youtube-extension-$V.zip"
