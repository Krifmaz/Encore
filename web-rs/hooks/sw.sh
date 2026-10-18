#!/bin/sh
# Trunk post_build hook (see Trunk.toml): writes the service worker from static/sw.js.
# Trunk names the .js and .wasm after their content hash, so the shell list cannot be fixed in the template;
# this script lists them (and the icons) from the staging dir and makes the cache name from that hash, so a
# new build always gets a new cache name: the new worker saves the new shell next to the old one, waits until
# the page asks it to take over ("A new version of Encore is ready" → Reload), and then deletes the old one.
# Prints the cache name and every file of the shell list.
set -eu
src="${TRUNK_SOURCE_DIR:-.}"
out="${TRUNK_STAGING_DIR:-${TRUNK_DIST_DIR:-dist}}"
files="/ /index.html /css/app.css /favicon.svg /manifest.webmanifest /share.html"
# the icons dir as it is (png and svg; the image's pre-compressed .br/.gz copies are made later and not listed)
for f in "$out"/icons/*; do
  [ -f "$f" ] || continue
  name=$(basename "$f")
  case "$name" in
    *.br|*.gz) continue ;;
  esac
  # the name goes into a JavaScript string and a sed replacement: plain names only
  if printf '%s' "$name" | grep -q '[^A-Za-z0-9._-]'; then
    echo "sw.sh: skipping icons/$name (only letters, digits, dot, dash and underscore in shell file names)" >&2
    continue
  fi
  files="$files /icons/$name"
done
hash=""
for f in "$out"/encore-web-*.js "$out"/encore-web-*.wasm; do
  [ -f "$f" ] || continue
  name=$(basename "$f")
  files="$files /$name"
  if [ -z "$hash" ]; then
    hash=$(printf '%s' "$name" | sed -n 's/^encore-web-\([0-9a-f]*\).*$/\1/p')
  fi
done
[ -n "$hash" ] || { echo "sw.sh: no encore-web-<hash>.js in $out" >&2; exit 1; }
# every file of the shell must be in the build: one missing file would make the worker's install fail
shell=""
n=0
for p in $files; do
  case "$p" in
    /) ;;
    *) [ -f "$out$p" ] || { echo "sw.sh: $p is in the shell list but not in $out" >&2; exit 1; } ;;
  esac
  shell="${shell:+$shell, }'$p'"
  n=$((n + 1))
done
sed -e "s|__CACHE__|encore-shell-$hash|" -e "s|__SHELL__|$shell|" "$src/static/sw.js" > "$out/sw.js"
echo "sw.js: cache encore-shell-$hash, $n shell files:"
for p in $files; do echo "  $p"; done
