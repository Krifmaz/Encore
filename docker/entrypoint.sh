#!/bin/sh
set -e

# yt-dlp, streamrip and Encore's other helper programs are plugins (Settings → Plugins): they install into
# /config/plugins/<id> as the Encore user, survive a new image and are kept fresh by the "Update plugins" task
# (a /config/tools/venv from before 0.32 is adopted where it is). YTDLP_AUTO_UPDATE=false still turns off
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
  # The download clients' own folders (qBittorrent, SABnzbd, NZBGet, Transmission, Deluge, rTorrent, and the
  # blackhole clients' completed folders) are the client's, which may run as another user: one that exists
  # is left as it is, so the client can still write there; only one this script creates is handed to the
  # Encore user. An existing blackhole watch folder (Encore drops the .torrent/.nzb files there, the client
  # takes them away) is handed over only when the Encore user can't write to it yet.
  old_ifs=$IFS
  IFS='
'
  for line in "ENCORE_STAGING_PATH=${ENCORE_STAGING_PATH:-/downloads/encore}" "SLSKD_DOWNLOAD_PATH=${SLSKD_DOWNLOAD_PATH:-/downloads/slskd}" \
              $(env | grep -E '^[A-Z0-9_]+_(DOWNLOAD_PATH|BLACKHOLE_WATCH|BLACKHOLE_COMPLETED)=' || true); do
    name=${line%%=*}
    d=${line#*=}
    [ -n "$d" ] || continue
    case "$name" in
      QBIT_DOWNLOAD_PATH|SAB_DOWNLOAD_PATH|NZBGET_DOWNLOAD_PATH|TRANSMISSION_DOWNLOAD_PATH|DELUGE_DOWNLOAD_PATH|RTORRENT_DOWNLOAD_PATH|\
      TORRENT_BLACKHOLE_COMPLETED|NZB_BLACKHOLE_COMPLETED)
        [ -d "$d" ] && continue ;;
      *_BLACKHOLE_WATCH)
        [ -d "$d" ] && setpriv --reuid="${PUID}" --regid="${PGID}" --init-groups /bin/sh -c '[ -w "$1" ]' sh "$d" 2>/dev/null && continue ;;
    esac
    if mkdir -p "$d" 2>/dev/null; then
      chown "${PUID}:${PGID}" "$d" 2>/dev/null || true
    fi
  done
  IFS=$old_ifs
  # "Update now" (Settings → Diagnostics) talks to Docker through its socket when the owner mounts it: the
  # Encore user joins the socket's group so it may write to it (never root's group 0: then PUID=0 or a PGID
  # of the socket's group is the owner's choice). ENCORE_SELF_UPDATE=off keeps it out of that group: every
  # program Encore starts (slskd, yt-dlp, streamrip, Cadenza, the translator) would get the same access.
  case "$(printf '%s' "${ENCORE_SELF_UPDATE:-}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')" in
    off|false|no|0) ;;
    *)
      if [ -S /var/run/docker.sock ]; then
        sock_gid=$(stat -c %g /var/run/docker.sock 2>/dev/null || echo 0)
        if [ "$sock_gid" != "0" ] && [ "$sock_gid" != "${PGID}" ]; then
          getent group "$sock_gid" >/dev/null 2>&1 || groupadd -g "$sock_gid" dockersock 2>/dev/null || true
          sock_group=$(getent group "$sock_gid" | cut -d: -f1)
          # setpriv --init-groups takes the groups of the first user with uid PUID: "encore", or the image's own
          # user when PUID was already taken (33 www-data and the like; useradd -o adds "encore" after it)
          sock_user=$(getent passwd "${PUID}" | cut -d: -f1)
          [ -n "$sock_group" ] && usermod -aG "$sock_group" "${sock_user:-encore}" 2>/dev/null || true
        fi
      fi
      ;;
  esac
  export HOME=/config
  # the rest of this script (the loop below) runs again as the Encore user
  exec setpriv --reuid="${PUID}" --regid="${PGID}" --init-groups /bin/sh "$0" "$@"
fi

# Run Encore. "Restart server" (Settings → Diagnostics) ends it with exit code 75: start it again at once.
# Any other code ends the container with that code, so `restart: unless-stopped` and a plain `docker run`
# behave as before. `docker stop` (SIGTERM, or SIGINT) is passed on to Encore and ends the loop.
set +e
child=
stopping=
trap 'stopping=1; [ -n "$child" ] && kill -TERM "$child" 2>/dev/null' TERM INT
while :; do
  # `docker stop` between two runs: don't start Encore again
  [ -n "$stopping" ] && exit 143
  "$@" &
  child=$!
  # or a stop that came while it was starting, before the trap knew the new process
  [ -n "$stopping" ] && kill -TERM "$child" 2>/dev/null
  wait "$child"
  code=$?
  # a signal interrupts the wait: wait again for Encore's own exit code
  while kill -0 "$child" 2>/dev/null; do
    wait "$child"
    code=$?
  done
  child=
  if [ "$code" -ne 75 ] || [ -n "$stopping" ]; then
    exit "$code"
  fi
  echo "Encore is restarting (exit code 75)" >&2
done
