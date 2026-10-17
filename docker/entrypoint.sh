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
  # Every folder the environment names is created and handed to the Encore user before root is dropped:
  # the staging and Soulseek folders (their defaults too), every *_DOWNLOAD_PATH and the blackhole folders.
  # Only the folder itself is chowned, never its contents (-R): the files in a /downloads mount are the
  # owner's. Folders saved in Settings instead of the environment are checked by the server itself.
  old_ifs=$IFS
  IFS='
'
  for d in "${ENCORE_STAGING_PATH:-/downloads/encore}" "${SLSKD_DOWNLOAD_PATH:-/downloads/slskd}" \
           $(env | sed -n -e 's/^[A-Z0-9_]*_DOWNLOAD_PATH=//p' -e 's/^[A-Z0-9_]*_BLACKHOLE_WATCH=//p' -e 's/^[A-Z0-9_]*_BLACKHOLE_COMPLETED=//p'); do
    [ -n "$d" ] || continue
    if mkdir -p "$d" 2>/dev/null; then
      chown "${PUID}:${PGID}" "$d" 2>/dev/null || true
    fi
  done
  IFS=$old_ifs
  export HOME=/config
  exec setpriv --reuid="${PUID}" --regid="${PGID}" --init-groups "$@"
fi

exec "$@"
