#!/bin/sh
set -e

# Keep yt-dlp fresh – sites change their players constantly.
if [ "${YTDLP_AUTO_UPDATE:-true}" = "true" ]; then
  echo "[encore] updating yt-dlp…"
  pip install -q -U yt-dlp 2>/dev/null || echo "[encore] yt-dlp update failed, using bundled version"
fi

# Run as PUID/PGID (like linuxserver.io images) unless started as a non-root user.
if [ "$(id -u)" = "0" ] && [ "${PUID}" != "0" ]; then
  groupadd -o -g "${PGID}" encore 2>/dev/null || true
  useradd -o -u "${PUID}" -g "${PGID}" -d /config -s /bin/sh encore 2>/dev/null || true
  chown -R "${PUID}:${PGID}" /config
  for d in "${ENCORE_STAGING_PATH:-/downloads/encore}"; do
    mkdir -p "$d" && chown "${PUID}:${PGID}" "$d" || true
  done
  export HOME=/config
  exec setpriv --reuid="${PUID}" --regid="${PGID}" --init-groups "$@"
fi

exec "$@"
