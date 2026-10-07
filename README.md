# Encore

A self-hosted music request and library app. Your users sign in, search the whole MusicBrainz catalogue, request artists, albums or songs, and play what is already in your library. All of it happens in one modern web UI. Behind the scenes, Encore runs Lidarr, slskd (Soulseek) and yt-dlp for you. Only Encore needs to be reachable from the internet.

## Features

- **Accounts**: login and sign-up with invite-only, open or closed modes. The admin panel manages users, roles, weekly request limits, auto-approve, password resets and session revocation. Invite links can have a use count and an expiry date.
- **Discover**: search artists, albums and songs on MusicBrainz. Artist pages show the full discography with *In library* and *Requested* badges. Any album or song can be requested with one click.
- **Request pipeline**: each request is tried against **Lidarr → Soulseek → yt-dlp** in the order you choose, and falls back to the next source automatically.
  - **Lidarr**: adds the artist and monitors and searches only the requested album.
  - **slskd**: ranks results by track count, FLAC/MP3 preference, bitrate, folder name, free upload slots and speed. Stalled or failing users are skipped.
  - **yt-dlp**: matches each MusicBrainz track on YouTube, preferring official "Topic" uploads with the right length.
- **Downloader**: paste any URL from YouTube, YouTube Music, SoundCloud, Bandcamp, Mixcloud or the roughly 1,800 other yt-dlp sites. A Spotify playlist or album link becomes one request per track.
- **Library**: browse albums, artists and songs, with a built-in web player and album zip downloads.
- **MusicBrainz tagging**: match any album against MusicBrainz and review an old → new preview before tags are written. Writes full tags (MBIDs, ISRC, label, barcode and more) and cover art, then renames files using your template. *Auto-tag untagged* handles the whole library in bulk. Manual tag editing is also built in.
- **Integrations**: refreshes Navidrome, Jellyfin or Plex after an import. Notifications go to Discord, ntfy or a generic webhook. An API key enables automation, with Swagger docs at `/api/docs`.
- Dark and light themes, a mobile-friendly layout, and installable as a PWA.

## Quick start

```bash
git clone https://github.com/krifmaz/test encore && cd encore
cp .env.example .env        # set paths, admin password, API keys
docker compose up -d --build
```

Open `http://your-server:8080` and sign in with `ENCORE_ADMIN_USER` / `ENCORE_ADMIN_PASSWORD`. If you didn't set a password, one is generated and printed in `docker logs encore`, and saved to `config/encore/initial_admin_password.txt`.

Then go to **Settings** to configure the services and use **Test connection**. Invite people from **Invites**.

### Prebuilt image

GitHub Actions publishes `ghcr.io/krifmaz/test:latest` (amd64 and arm64). Use `image:` in place of `build: .` if you prefer.

## Wiring up the services

Encore, Lidarr and slskd must see the **same files at the same paths**:

| Host folder | Encore | Lidarr | slskd |
|---|---|---|---|
| Music library | `/music` | `/music` (root folder) | `/music` (read-only share) |
| Downloads | `/downloads` | `/downloads` | `/downloads` |

**slskd** saves to `/downloads/slskd`, and Encore imports from there. Add an API key to `config/slskd/slskd.yml`:

```yaml
web:
  authentication:
    api_keys:
      encore:
        key: <same value as SLSKD_API_KEY>
        cidr: 0.0.0.0/0,::/0
```

**Lidarr**: copy the API key from *Settings → General*, and add `/music` as a root folder. Encore picks the quality and metadata profiles; you can change them in Settings → Lidarr.

## Keeping it private

- Publish **only Encore** (port 8080). The compose file keeps Lidarr's and slskd's web UIs off the public internet. slskd only exposes its Soulseek peer port.
- Put it behind HTTPS. For example, with Caddy:

  ```
  music.example.com {
      reverse_proxy encore:8080
  }
  ```
- Sign-ups default to **invite only**. Sessions use HttpOnly, SameSite=Strict cookies, mutating requests need a CSRF header, and failed logins are rate-limited per IP.

## Configuration

Every option can be changed in the UI. Environment variables only seed the first start.

| Variable | Default | |
|---|---|---|
| `PUID` / `PGID` | 1000 | User the app runs as |
| `ENCORE_ADMIN_USER` / `ENCORE_ADMIN_PASSWORD` | admin / random | First admin account |
| `ENCORE_LIBRARY_PATH` | `/music` | Music library inside the container |
| `ENCORE_SIGNUP_MODE` | `invite` | `invite`, `open` or `closed` |
| `ENCORE_SOURCE_ORDER` | `lidarr,slskd,ytdlp` | Order sources are tried in |
| `ENCORE_NAMING_TEMPLATE` | `{albumartist}/{album} ({year})/{discprefix}{track:02d} - {title}` | Rename template |
| `LIDARR_URL` / `LIDARR_API_KEY` / `LIDARR_ENABLED` | | |
| `SLSKD_URL` / `SLSKD_API_KEY` / `SLSKD_ENABLED` / `SLSKD_DOWNLOAD_PATH` | | |
| `YTDLP_AUTO_UPDATE` | `true` | Upgrades yt-dlp at every container start |
| `SPOTIFY_CLIENT_ID` / `SPOTIFY_CLIENT_SECRET` | | Playlist import |
| `NAVIDROME_*`, `JELLYFIN_*`, `PLEX_*` | | Library refresh after imports |
| `DISCORD_WEBHOOK`, `NTFY_URL`, `WEBHOOK_URL` | | Notifications |

## Development

```bash
pip install -r requirements.txt pytest
ENCORE_CONFIG_DIR=./data ENCORE_LIBRARY_PATH=~/Music uvicorn app.main:app --reload --port 8080
cd tests && pytest -q
```

The backend uses FastAPI, SQLite and mutagen. The frontend is plain ES modules with no build step.
