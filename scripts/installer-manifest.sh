#!/usr/bin/env bash
# Write the Omnistall manifest for one Encore version from clients/installer/encore.toml.in.
#   scripts/installer-manifest.sh <version> <out.toml> [SHA256SUMS]
# {version} is filled everywhere (app.version, the release tag, the asset names, the icon URL). With a SHA256SUMS
# file (the one a release publishes, or `sha256sum out/*` over freshly built payloads) each source's
# `checksum_asset = "SHA256SUMS"` line becomes `sha256 = "<hash of that asset>"`, so the installer carries the
# hashes itself; without one the line stays and Omnistall reads the release's SHA256SUMS when it installs.
# Used by the installer job in .github/workflows/apps.yml and by scripts/release-apps.sh --installer.
set -euo pipefail
V="${1:?version, e.g. 0.27.0}"
OUT="${2:?output file}"
SUMS="${3:-}"
V="${V#v}"
TPL="$(cd "$(dirname "$0")/.." && pwd)/clients/installer/encore.toml.in"
[ -f "$TPL" ] || { echo "$TPL is missing"; exit 1; }
if [ -n "$SUMS" ]; then
  [ -f "$SUMS" ] || { echo "$SUMS not found"; exit 1; }
  SUMS="$(cd "$(dirname "$SUMS")" && pwd)/$(basename "$SUMS")"
fi
case "$V" in */*|*'&'*|*'{'*|'') echo "bad version $V"; exit 1 ;; esac

sed "s/{version}/$V/g" "$TPL" | awk -v sums="$SUMS" '
  BEGIN {
    # GNU (`hash  name` / `hash *name`) lines; the name is the last field
    if (sums != "") while ((getline line < sums) > 0) { n = split(line, f, /[ *]+/); if (n >= 2 && length(f[1]) == 64) hash[f[n]] = f[1] }
  }
  /^asset = "/ { a = $0; sub(/^asset = "/, "", a); sub(/".*$/, "", a); asset = a }
  /^checksum_asset = / && sums != "" {
    if (asset in hash) { print "sha256 = \"" hash[asset] "\""; next }
    print "note: " asset " is not listed in " sums " - this source keeps checksum_asset" > "/dev/stderr"
  }
  { print }
' > "$OUT.tmp"
if grep -q '{version}' "$OUT.tmp"; then rm -f "$OUT.tmp"; echo "a {version} placeholder was left unfilled"; exit 1; fi
mv "$OUT.tmp" "$OUT"
echo "$OUT"
