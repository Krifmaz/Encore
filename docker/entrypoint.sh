#!/bin/sh
set -e

# yt-dlp and streamrip are kept fresh by Encore's "Update download tools" task: updates go to
# /config/tools/venv as the Encore user and survive a new image. YTDLP_AUTO_UPDATE=false still turns off
# yt-dlp's automatic updates (read by Encore).

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
