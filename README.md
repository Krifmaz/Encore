# Encore

A self-hosted music request and library app. Your users sign in, search the whole MusicBrainz catalogue, request artists, albums or songs, and play what is already in your library. All of it happens in one modern web UI. Behind the scenes, Encore runs Lidarr, slskd (Soulseek) and yt-dlp for you. Only Encore needs to be reachable from the internet.

## Features

- **Accounts**: login and sign-up with invite-only, open or closed modes. The admin panel manages users, roles, weekly request limits, auto-approve, password resets and session revocation.
- **Discover**: search MusicBrainz for artists, albums and songs, with *In library* and *Requested* badges. Request anything with one click.
- **Request pipeline**: with *highest quality first* on, each request tries **Qobuz/Tidal/Deezer (streamrip) → FLAC on Soulseek (24-bit preferred) → Lidarr → MP3 on Soulseek → YouTube Music**, falling back automatically. Exact ISRCs are used whenever the source provides them.
- **Whole albums**: when a requested song is on an official studio album or EP, Encore downloads the whole release instead, in the best quality any source has. Compilations, live albums and soundtracks are never used. If the album doesn't have that exact version (say, a remix), the song is fetched on its own.
- **For you**: your ListenBrainz *Weekly Exploration* and *Daily Jams* playlists, with songs you own marked and playable and everything else one click to request. *Admin → Auto-download* can grab the top N new songs from each week's exploration list automatically. Recommendations need listening history, so point Navidrome (built-in), Jellyfin/Finamp (ListenBrainz plugin) or Plexamp at ListenBrainz first.
- **Similar artists and "More like this"**: similar artists come from Last.fm, falling back to ListenBrainz and then Deezer. ✨ on any song shows similar tracks from Last.fm, or a Deezer radio without a key. Genre tags open genre pages with top artists from Last.fm, or from MusicBrainz without a key.
- **Fills MusicBrainz gaps**: artist pages also check Deezer, Apple Music, YouTube Music, Bandcamp and Discogs (vinyl, bootlegs and old releases; needs a free token or a Discogs link on MusicBrainz). Releases are matched by barcode (UPC), then ISRC, then title, year and track count; anything MusicBrainz doesn't list shows as *Not in MusicBrainz*. Those requests skip Lidarr (it can't monitor them) and download from Soulseek, streamrip, or the exact YouTube Music/Bandcamp release, tagged from the source's data. *Add to MB* opens Harmony to import the release into MusicBrainz, and Encore re-tags it automatically once it's there.
- **Artist pages**: the full MusicBrainz discography (albums, EPs, singles, compilations, live, soundtracks, remixes…) with *All / Missing / In library* filters and *Request N missing* per section. Each page also shows the artist photo, a Wikipedia bio, popular tracks (play the ones you have, request the rest), your library albums including ones MusicBrainz doesn't list, and Play/Shuffle. Library artists open the same page.
- **Follow artists** (replaces Lidarr's monitoring): follow an artist and Encore checks MusicBrainz for new albums/EPs (singles optional) every 12 hours, then downloads them automatically in the best quality. Optionally grab the back catalogue you're missing, or follow every artist in your library in one click (*Following* page).
- **Lossless upgrades**: albums you only have as MP3 or YouTube rips can be re-checked periodically. A lossless copy replaces the old files, and playlists follow it automatically.
- **YouTube Music**: playlists, albums and your Liked Music import with proper artist and album data. Connect your account for two-way 👍 sync and playlist push. Songs and whole albums are downloaded from official YouTube Music uploads in the original stream quality (256 kbps with Premium cookies).
- **Admin → Auto-download** controls what downloads on its own (liked songs from any app, new tracks in followed playlists), the album and quality rules, lossless upgrades, and which users have it switched on.
- **Manual download picker** (like Lidarr's interactive search): *Pick download* on any album or song, or *Pick* on a request. It searches Soulseek, Deezer, YouTube Music and Lidarr's indexers at once and shows each result's quality, bit depth, size, file count and uploader queue/speed. You choose the exact one, or paste any link. Admins can limit it to admins in Settings.
- **Song or whole album, per request**: in Discover, *Song* or *+ Album*. In the Downloader, *Just the song / Get the whole album / Admin default*, which also applies to playlist imports.
- **Downloader**: a Spotify, Apple Music, Deezer, Tidal or Qobuz link (playlist, album or track) is matched track by track, and each song is fetched from the best source. YouTube, SoundCloud, Bandcamp, Mixcloud and about 1,800 other sites download directly.
- **Playlists and Liked Songs**:
  - Import from Spotify (including private playlists and Liked Songs once you connect your account), Deezer, Tidal, Apple Music, Qobuz, YouTube Music, SoundCloud, Bandcamp, ListenBrainz, or a pasted "Artist – Title" list.
  - Merge playlists, follow a source so new tracks are pulled in automatically, and "Download missing" in one click.
- **Sync everywhere**: each user connects their own accounts on the *Connections* page. Liked songs sync two-way (likes *and* un-likes) between Jellyfin/Finamp, Plex/Plexamp, Navidrome and ListenBrainz. Playlists are mirrored to whichever servers you tick.
- **Correct versions, no duplicates**:
  - One version-aware matcher is used everywhere. It tells originals apart from remixes (including *which* remixer), live, acoustic, radio and extended edits, instrumentals, slowed/sped-up versions, re-recordings ("Taylor's Version") and covers by other artists. Remasters and "feat." credits count as the same song.
  - Downloads are fingerprinted with **AcoustID/Chromaprint** before import. Wrong audio is rejected and the next source is tried.
  - A song that's already in the library is not imported a second time; a better-quality file replaces the old one. The *Duplicates* tab finds identical copies already in the library, and *Wrong audio* lists files whose content doesn't match their tags.
- **Web player**: play albums, songs and playlists in the browser. MP3, FLAC, AAC and Opus stream as the original file, with no conversion and full seeking. ALAC, APE, WavPack, AIFF and WMA are converted to FLAC on the fly, which keeps them lossless.
- **Where it came from**: every downloaded track shows its source on the album page (Soulseek user and folder, YouTube Music link, Deezer/Tidal/Qobuz release, Lidarr or a direct link) and when it was downloaded.
- **Artwork for everything**: Cover Art Archive first, then the request's own cover, then Deezer, iTunes or YouTube Music. Art is embedded in the files so Finamp and Plexamp show it too. *Library → Find missing artwork* fixes existing albums.
- **Library**: browse albums, artists and songs, match albums to MusicBrainz with an old → new preview, write full tags and cover art, rename files using your template, bulk auto-tag, edit tags by hand, and download albums as zips.
- **Integrations**: Navidrome/Jellyfin/Plex rescans after imports, Discord/ntfy/webhook notifications, and an API key with Swagger docs at `/api/docs`.

## Quick start

```bash
git clone https://github.com/Krifmaz/Encore encore && cd encore
cp .env.example .env        # set paths, admin password, API keys
docker compose up -d --build
```

Open `http://your-server:8080` and sign in with `ENCORE_ADMIN_USER` / `ENCORE_ADMIN_PASSWORD`. If you didn't set a password, one is generated and printed in `docker logs encore`, and saved to `config/encore/initial_admin_password.txt`.

Then go to **Settings** to configure the services and use **Test connection**. Invite people from **Invites**.

### All-in-one: no Lidarr/slskd containers needed

`ghcr.io/krifmaz/encore:aio` runs **slskd, Lidarr, Prowlarr, qBittorrent and SABnzbd inside Encore**. They are configured and connected to each other on first start:

- Encore talks to slskd and Lidarr.
- Lidarr gets `/music` as its root folder, with qBittorrent and SABnzbd as download clients.
- Prowlarr is linked to Lidarr, so indexers you add there sync automatically.

Each service listens on localhost only. Open its web UI from **Admin → Built-in services**; it runs behind your Encore admin login. Left for you to do: your Soulseek login (on that page), indexers in Prowlarr, and a Usenet server in SABnzbd's wizard. See [`docker-compose.aio.yml`](docker-compose.aio.yml), which routes everything through gluetun for a VPN. Pick which services to run with `ENCORE_EMBEDDED=all` or a list such as `slskd,lidarr,prowlarr,qbittorrent`.

### Prebuilt image

GitHub Actions publishes `ghcr.io/krifmaz/encore:latest` and the all-in-one `ghcr.io/krifmaz/encore:aio` (amd64 and arm64). If the repository is private, the image is too – run `docker login ghcr.io` once with a token that has `read:packages`.

## Apps

- **Android**: download `Encore-android.apk` from the [Releases page](https://github.com/Krifmaz/Encore/releases), install it, and enter your server address (e.g. `http://192.168.1.10:8080`). Music keeps playing with the screen off, with lock-screen and notification controls. *Share → Encore* from Spotify, YouTube Music or a browser downloads that link.
- **Desktop** (Windows, macOS, Linux): same Releases page. Media keys work, there's a tray with play/pause/next, and it remembers your server.
- **Any phone**: open Encore in the browser and choose *Add to Home screen*. On HTTPS it installs as an app.

## Wiring up the services

Encore, Lidarr and slskd must see the **same files at the same paths**:

| Host folder | Encore | Lidarr | slskd |
|---|---|---|---|
| Music library | `/music` | `/music` (root folder) | `/music` (read-only share) |
| Downloads | `/downloads` | `/downloads` | `/downloads` |

**slskd** saves to `/downloads/slskd`, and Encore imports from there. Add an API key to `slskd.yml` (in slskd's `/app` folder). Limit the key to private networks, which covers every Docker network:

```yaml
web:
  authentication:
    api_keys:
      encore:
        key: <same value as SLSKD_API_KEY>
        cidr: 10.0.0.0/8,172.16.0.0/12,192.168.0.0/16
```

- **slskd behind gluetun** (`network_mode: container:gluetun-slsk`): slskd has no hostname of its own, so use the VPN container's name, e.g. `SLSKD_URL=http://gluetun-slsk:5030`.
- **slskd permissions:** slskd ignores `PUID`/`PGID`. Run it with `user: "1000:1000"` and `chown -R 1000:1000` its config, downloads and incomplete folders, or it can't write to them.

**Lidarr**: copy the API key from *Settings → General*, and add `/music` as a root folder. Encore picks the quality and metadata profiles; you can change them in Settings → Lidarr.

**Optional keys** (Settings):

| Feature | Where to get it |
|---|---|
| Audio verification | Free AcoustID application key: acoustid.org/new-application |
| Qobuz/Tidal/Deezer lossless | Your own subscription. Enter the Deezer ARL or Qobuz token in *Settings → streamrip*. Tidal needs a one-time sign-in: `docker exec -it encore streamrip --config-path /config/streamrip/config.toml url <tidal link>`. These services' terms may not allow downloading, so use at your own risk. |
| Spotify account connection | developer.spotify.com app with redirect `<Public URL>/api/connections/spotify/callback`. Since Feb 2026, development-mode apps need a Premium owner and allow 5 users. Public playlists still import without it, via the embed page. |
| Tidal import | developer.tidal.com app (redirect `<Public URL>/api/connections/tidal/callback`) |
| Last.fm loved tracks, similar artists/tracks, genres | last.fm/api/account/create (free) |
| Discogs catalog (releases MusicBrainz lacks) | discogs.com/settings/developers → personal access token (free) |
| YouTube Music sign-in | Google Cloud, YouTube Data API v3, an OAuth client of type *TVs and Limited Input devices*. Users can paste browser request headers instead. |
| YouTube Music Premium audio (256 kbps) | Paste a `cookies.txt` exported from music.youtube.com under *Settings → YouTube Music*. |

Set **Settings → General → Public URL** (for example `https://music.example.com`) so the OAuth logins and invite links work.

## Troubleshooting

**Admin → Diagnostics** runs every connection, folder-permission and tool check from inside the container and shows the raw responses, with keys hidden. *Copy report* gives you text you can paste into a bug report. `curl` is also included in the image: `docker exec -it encore curl -s http://lidarr:8686/ping`.

| Symptom | Fix |
|---|---|
| A setting won't change in the UI | It's set by an environment variable (the field shows 🔒 and the variable name). Environment variables always win, so change it in your compose file. |
| `401 … API key rejected` (slskd) | The key in Encore doesn't match `web.authentication.api_keys` in `slskd.yml`, or the key's `cidr` doesn't include Encore's IP. Don't put quotes around values in `- KEY=value` lines. |
| `Can't find host 'slskd'` | Use the service or container name on the same Docker network. Behind gluetun, use the gluetun container's name. |
| slskd: `/incomplete` (or downloads) not writable | Run slskd with `user: "1000:1000"` and `chown -R 1000:1000` its folders. |
| slskd won't start after an update: "permissions" error | Since slskd 0.26.0 the top-level `permissions:` key moved to `transfers.download.destination.permissions` (slskd PR #1756), and an old config stops slskd from starting. Move the block. The mode now also applies to folders. |
| Folder not writable by Encore | Diagnostics shows the folder owner and Encore's uid. Match them with `PUID`/`PGID` or `chown`. |
| Config ends up somewhere odd under Portainer | Use absolute host paths (`/opt/appdata/encore:/config`), not `./encore`. |

## Keeping it private

- Publish **only Encore** (port 8080). The compose file keeps Lidarr's and slskd's web UIs off the public internet. slskd only exposes its Soulseek peer port.
- Put it behind HTTPS. For example, with Caddy:

  ```
  music.example.com {
      reverse_proxy encore:8080
  }
  ```
- Sign-ups default to **invite only**. Sessions use HttpOnly SameSite=Lax cookies, every mutating request needs a CSRF header, and failed logins are rate-limited per IP.

## Configuration

Every option can be changed in the UI. Environment variables only seed the first start.

| Variable | Default | |
|---|---|---|
| `PUID` / `PGID` | 1000 | User the app runs as |
| `ENCORE_ADMIN_USER` / `ENCORE_ADMIN_PASSWORD` | admin / random | First admin account |
| `ENCORE_LIBRARY_PATH` | `/music` | Music library inside the container |
| `ENCORE_SIGNUP_MODE` | `invite` | `invite`, `open` or `closed` |
| `ENCORE_EMBEDDED` | (off; `all` in the aio image) | Built-in services to run: `all` or `slskd,lidarr,prowlarr,qbittorrent,sabnzbd` |
| `SOULSEEK_USERNAME` / `SOULSEEK_PASSWORD` | | Soulseek login for the built-in slskd |
| `HIFI_ENABLED` / `HIFI_API_URL` / `HIFI_QUALITY` | off | TIDAL lossless via your own HiFi API instance (Monochrome's backend) |
| `ENCORE_DISCOVER_AUTO_COUNT` | 0 | Songs to auto-request from each Weekly Exploration (0 = off) |
| `ENCORE_CATALOG_MERGE` | `true` | Show Deezer/Apple releases MusicBrainz lacks |
| `ENCORE_MANUAL_SEARCH` | `everyone` | Who can use the manual download picker: `everyone` or `admins` |
| `ENCORE_SOURCE_ORDER` | `lidarr,slskd,ytdlp` | Order sources are tried in |
| `ENCORE_NAMING_TEMPLATE` | `{albumartist}/{album} ({year})/{discprefix}{track:02d} - {title}` | Rename template |
| `LIDARR_URL` / `LIDARR_API_KEY` / `LIDARR_ENABLED` | | |
| `SLSKD_URL` / `SLSKD_API_KEY` / `SLSKD_ENABLED` / `SLSKD_DOWNLOAD_PATH` | | |
| `YTDLP_AUTO_UPDATE` | `true` | Upgrades yt-dlp at every container start |
| `SPOTIFY_CLIENT_ID` / `SPOTIFY_CLIENT_SECRET`, `TIDAL_CLIENT_ID` / `TIDAL_CLIENT_SECRET`, `LASTFM_API_KEY`, `DISCOGS_TOKEN` | | Account connections, imports, discovery, catalog |
| `ACOUSTID_API_KEY` | | Audio fingerprint verification |
| `STREAMRIP_ENABLED`, `STREAMRIP_DEEZER_ARL`, `STREAMRIP_QOBUZ_USER` / `STREAMRIP_QOBUZ_TOKEN` | off | Qobuz/Tidal/Deezer source |
| `ENCORE_SYNC_INTERVAL` | 15 | Minutes between like/playlist syncs |
| `NAVIDROME_*`, `JELLYFIN_*`, `PLEX_*` | | Library refresh after imports |
| `DISCORD_WEBHOOK`, `NTFY_URL`, `WEBHOOK_URL` | | Notifications |

## Development

```bash
pip install -r requirements.txt pytest
ENCORE_CONFIG_DIR=./data ENCORE_LIBRARY_PATH=~/Music uvicorn app.main:app --reload --port 8080
cd tests && pytest -q
```

The backend uses FastAPI, SQLite, mutagen and Chromaprint, and needs Python 3.14 if you want streamrip. The frontend is plain ES modules with no build step.
