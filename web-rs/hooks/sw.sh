#!/bin/sh
# Trunk post_build hook (see Trunk.toml): writes the service worker from static/sw.js.
# Trunk names the .js and .wasm after their content hash, so the shell list cannot be fixed in the template;
# this script lists them from the staging dir and makes the cache name from that hash, so a new build always
# gets a new cache name and the activate step throws the previous shell away.
set -eu
src="${TRUNK_SOURCE_DIR:-.}"
out="${TRUNK_STAGING_DIR:-${TRUNK_DIST_DIR:-dist}}"
shell="'/', '/index.html', '/css/app.css', '/favicon.svg', '/manifest.webmanifest', '/icons/icon-192.png'"
hash=""
for f in "$out"/encore-web-*.js "$out"/encore-web-*.wasm; do
  [ -f "$f" ] || continue
  name=$(basename "$f")
  shell="$shell, '/$name'"
  if [ -z "$hash" ]; then
    hash=$(printf '%s' "$name" | sed -n 's/^encore-web-\([0-9a-f]*\).*$/\1/p')
  fi
done
[ -n "$hash" ] || { echo "sw.sh: no encore-web-<hash>.js in $out" >&2; exit 1; }
sed -e "s|__CACHE__|encore-shell-$hash|" -e "s|__SHELL__|$shell|" "$src/static/sw.js" > "$out/sw.js"
echo "sw.js: cache encore-shell-$hash, $(printf '%s,' "$shell" | tr -cd ',' | wc -c | tr -d ' ') shell files"
