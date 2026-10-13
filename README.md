# Encore

A self-hosted music request and library app. Your users sign in, search the whole MusicBrainz catalogue, request artists, albums or songs, and play what is already in your library. All of it happens in one modern web UI. Behind the scenes, Encore has its own Soulseek client and runs your torrent/Usenet indexers (through Prowlarr), streamrip and yt-dlp for you. Only Encore needs to be reachable from the internet.

Encore is a single Rust program with the web app built in: fast, small, and no Lidarr needed.

## Features

- **Accounts**: login and sign-up with invite-only, open or closed modes. The admin panel manages users, roles, weekly request limits, auto-approve, password resets and session revocation.
- **Discover**: search MusicBrainz for artists, albums and songs, with *In library* and *Requested* badges. Request anything with one click.
- **Request pipeline**: with *highest quality first* on, each request tries **Qobuz/Tidal/Deezer (streamrip) → FLAC on Soulseek (24-bit preferred) → torrents/Usenet via Prowlarr → MP3 on Soulseek → YouTube Music**, falling back automatically. Exact ISRCs are used whenever the source provides them.
- **Whole albums**: when a requested song is on an official studio album or EP, Encore downloads the whole release instead, in the best quality any source has. Compilations, live albums and soundtracks are never used. If the album doesn't have that exact version (say, a remix), the song is fetched on its own.
- **For you**: discovery only – *Artists for you*, *Albums for you* and *Songs for you* built from what you play most on Last.fm (or in Encore), plus your ListenBrainz *Weekly Exploration*; anything already in the library is left out, everything shown is one click to request. **Made for you** on the home page (once ListenBrainz is connected) plays your *Daily Jams*, *Weekly Jams* and *Weekly Exploration* playlists with what you own and requests the rest. *Admin → Auto-download* can grab the top N new songs from each week's exploration list automatically.
- **Scrobbling**: what you play in Encore is scrobbled to Last.fm and ListenBrainz ("now playing" when a song starts, a scrobble after half the song or 4 minutes). Each user switches it on or off on the *Connections* page.
- **Remote control**: every signed-in device shows up under the devices button in the player. See what each one plays, control it, move music between them, or **link** a device as a locked remote so everything you play on it plays on the other one, until you unlink.
- **Similar artists and "More like this"**: similar artists come from Last.fm, falling back to ListenBrainz and then Deezer. ✨ on any song shows similar tracks from Last.fm, or a Deezer radio without a key. Genre tags open genre pages with top artists from Last.fm, or from MusicBrainz without a key.
- **Fills MusicBrainz gaps**: artist pages also check Deezer, Apple Music, YouTube Music, Bandcamp and Discogs (vinyl, bootlegs and old releases; needs a free token or a Discogs link on MusicBrainz). Releases are matched by barcode (UPC), then ISRC, then title, year and track count; anything MusicBrainz doesn't list shows as *Not in MusicBrainz*. Those requests download from Soulseek, streamrip, or the exact YouTube Music/Bandcamp release, tagged from the source's data. *Add to MB* opens Harmony to import the release into MusicBrainz, and Encore re-tags it automatically once it's there.
- **Artist pages**: the full MusicBrainz discography (albums, EPs, singles, compilations, live, soundtracks, remixes…) with *All / Missing / In library* filters and *Request N missing* per section. Each page also shows the artist photo, a Wikipedia bio, popular tracks (play the ones you have, request the rest), your library albums including ones MusicBrainz doesn't list, and Play/Shuffle. Library artists open the same page.
- **Lossless upgrades**: albums you only have as MP3 or YouTube rips can be re-checked periodically. A lossless copy replaces the old files, and playlists follow it automatically.
- **YouTube Music**: playlists, albums and your Liked Music import with proper artist and album data. Connect your account for two-way 👍 sync and playlist push. Songs and whole albums are downloaded from official YouTube Music uploads in the original stream quality (256 kbps with Premium cookies).
- **Admin → Auto-download** controls what downloads on its own (liked songs from any app, new tracks in followed playlists), the album and quality rules, lossless upgrades, and which users have it switched on.
- **Manual download picker**: *Pick download* on any album or song, or *Pick* on a request. It searches Soulseek, Deezer, YouTube Music and your Prowlarr indexers at once and shows each result's quality, bit depth, size, file count and uploader queue/speed. You choose the exact one, or paste any link. Admins can limit it to admins in Settings.
- **Soulseek page** (menu → Soulseek, for everyone allowed to pick downloads): search the network directly, browse any user's shared folders, download whole folders or a tick-list of files (they become ordinary requests, tagged and imported like any other), and watch downloads and uploads under *Transfers*. Works with the built-in client and with slskd.
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
- **Web player**: play albums, songs and playlists in the browser. MP3, FLAC, AAC and Opus stream as the original file, with no conversion and full seeking. ALAC, APE, WavPack, AIFF, WMA and DSD are converted to FLAC on the fly, which keeps them lossless; whatever a browser can't decode is converted automatically. **Settings → Playback** sets the quality: *Auto* plays the original, measures the connection while it plays and steps the stream down as far as needed (down to 32 kbps AAC/Opus), then back up as soon as the connection allows, *Original* never converts, *Data saver* always uses the small stream (AAC/Opus 96 kbps); the next song is pre-loaded once the current one is safely buffered. **Sound (on this device)** adds a parametric EQ with presets, headphone crossfeed, balance, mono and crossfade (0–12 s, with an optional *Keep albums gapless*). They all run in the listener's browser, so they cost the server nothing, and the Signal path says when they're on. The quality badge opens the **Signal path**: what the file is (codec, bit depth, sample rate), what Encore sent (untouched, FLAC, or lossy and why) and what the device does with it.
- **Remote control**: every signed-in device (phone, browser tab, desktop app, the car) shows up under *Your devices*. Link one as a remote for another, send what you're playing to a different device, or control it from the full-screen player – commands and state are pushed instantly over the server's event stream. Share links carry the song's title, artist, album, cover and quality, so chat apps preview them.
- **Cadenza** (separate project, [github.com/Krifmaz/cadenza](https://github.com/Krifmaz/cadenza)): an open-source upsampling and bit-perfect playback engine that runs on your own computer or a small box next to your DAC and plays straight into it. Link to it under *Your devices* and its switches (bit-perfect, filter, dither, rate) appear in Settings → Playback.
- **Server processing with Cadenza (optional, off by default)**: an admin can let the server upsample streams for browsers and phones with Cadenza's filters (Settings → Apps & sources → Server processing). It needs the *Server processing* permission per user, and has limits for streams at once, highest rate and longest filter. Each song is rendered once into a disk cache (size limit, *Clear cache*), the next song is rendered ahead, and seeking in a rendered song happens on the device – no new work on the server.
- **Roon**: Encore registers as a Roon extension (approve it once in Roon → Settings → Extensions). Your Roon zones then appear under *Your devices* with what they play, play/pause/next/previous and volume, and *Play in Roon…* searches Roon's library and starts the first match in a zone. Settings → Apps & sources → Roon; in Docker enter the Core's address, since multicast discovery doesn't cross the bridge network.
- **Android Auto**: the Android app is a media app for the car – browse recently added, Made for you, playlists, liked songs, albums and artists on the car screen, ask for a song or artist by voice, and the car player shows up under *Your devices* so you can push a queue to it from any browser.
- **Where it came from**: every downloaded track shows its source on the album page (Soulseek user and folder, YouTube Music link, Deezer/Tidal/Qobuz release, torrent/Usenet release or a direct link), when it was downloaded, who requested it and when, and when it was added.
- **Artwork for everything**: Cover Art Archive first, then the request's own cover, then Deezer, iTunes or YouTube Music. Art is embedded in the files so Finamp and Plexamp show it too. *Library → Find missing artwork* fixes existing albums.
- **Library**: browse albums, artists and songs, match albums to MusicBrainz with an old → new preview, write full tags and cover art, rename files using your template, bulk auto-tag, edit tags by hand, and download albums as zips.
- **Integrations**: Navidrome/Jellyfin/Plex rescans after imports, Discord/ntfy/webhook notifications, and an API key for scripts.
- **Make it yours**: themes (dark, OLED black, light, match device), presets, any accent colour, and a logo that follows it.

## Quick start

```bash
git clone https://github.com/Krifmaz/Encore encore && cd encore
cp .env.example .env        # set paths, admin password, API keys
docker compose up -d --build
```

Open `http://your-server:8080` and sign in with `ENCORE_ADMIN_USER` / `ENCORE_ADMIN_PASSWORD`. If you didn't set a password, one is generated and printed in `docker logs encore`, and saved to `config/encore/initial_admin_password.txt`.

Then go to **Settings** to configure the services and use **Test connection**. Invite people from **Invites**.

### Prebuilt image

GitHub Actions publishes `ghcr.io/krifmaz/encore` for amd64 and arm64:

| Tag | What it is |
|---|---|
| `:latest` | The newest release |
| `:0.20.1` … | One exact release |

Every release is a normal GitHub release, tagged `v<version>` (plain MAJOR.MINOR.PATCH numbers), with the notes, server archives, compose file and Android/desktop installers attached: [github.com/Krifmaz/Encore/releases](https://github.com/Krifmaz/Encore/releases). If the repository is private, the image is too – run `docker login ghcr.io` once with a token that has `read:packages`.

## Apps

- **Android**: download `Encore-android-<version>.apk` from the [Releases page](https://github.com/Krifmaz/Encore/releases), install it, and pick your server (e.g. `http://192.168.1.10:8080`). It is the same Rust app as the desktop one (Tauri) plus a small Kotlin plugin for the media notification: music keeps playing with the screen off, with lock-screen and notification controls; *Share → Encore* from Spotify, YouTube Music or a browser downloads that link; songs and albums can be saved for offline listening, and when the server is away the app opens its offline player.
- **Android Auto**: the app includes a car media service (browse and voice search of your library, native playback straight from the server). Because the APK is not installed from Google Play, Android Auto hides it until you allow it once: open the **Android Auto** app on the phone → scroll to *Version*, tap it about ten times until *Developer settings* unlocks → ⋮ → *Developer settings* → turn on **Unknown sources**. Then *Customize launcher* and tick Encore. Open Encore on the phone once first so it knows your server and sign-in; the car then starts it on its own.
- **Desktop** (Windows, macOS, Linux): same Releases page – a small Rust app (Tauri) that opens your server in its own window. Media keys and the system's media controls work (lock screen, Now Playing, MPRIS), there's a tray with what's playing and play/pause/next, and it remembers your servers. The macOS build is unsigned: right-click → Open the first time.
- **Any phone**: open Encore in the browser and choose *Add to Home screen*. On HTTPS it installs as an app.
- **Rust web interface (preview)**: the web app rewritten in Rust/WebAssembly is served at `/next/` beside the standard one – Settings → General → About has the link there and back. Same sign-in, same features; it becomes the default once it matches.

## Wiring up the services

Encore and the download clients must see the **same files at the same paths**:

| Host folder | Encore | slskd (optional) | qBittorrent / SABnzbd |
|---|---|---|---|
| Music library | `/music` | `/music` (read-only share) | – |
| Downloads | `/downloads` | `/downloads` | `/downloads` |

**Soulseek** is built in: no slskd container needed. In *Settings → Apps & sources → Soulseek*, turn it on and enter a username and password (or set `SOULSEEK_USERNAME` / `SOULSEEK_PASSWORD`). There is no sign-up form on Soulseek: the account is created the first time a new username and password log in. Don't use the same login in slskd or another Soulseek client at the same time, because the network signs the other one out. Forward TCP port 2234 on your router to Encore (the compose file publishes it; change it with `SOULSEEK_PORT`) so users behind firewalls can send to you. Downloads land in `/downloads/slskd` (the downloads folder setting) and are imported from there. Encore shares your library back to the network, read-only and only the library folder, with 2 upload slots by default (`SOULSEEK_SHARE`, `SOULSEEK_UPLOAD_SLOTS`): the network runs on sharing, and many users refuse downloads to people who share nothing. *Test connection* shows whether you are signed in, how many files are shared and the port.

- **Using slskd instead**: set `SOULSEEK_CLIENT=slskd` (or pick *slskd (external)* under *Client*), uncomment the slskd service in the compose file and fill in `SLSKD_URL` / `SLSKD_API_KEY`. slskd saves to `/downloads/slskd`, and Encore imports from there. Add an API key to `slskd.yml` (in slskd's `/app` folder). Limit the key to private networks, which covers every Docker network:

  ```yaml
  web:
    authentication:
      api_keys:
        encore:
          key: <same value as SLSKD_API_KEY>
          cidr: 10.0.0.0/8,172.16.0.0/12,192.168.0.0/16
  ```

  - **slskd behind gluetun** (`network_mode: container:gluetun-slsk`): slskd has no hostname of its own, so use the VPN container's name, e.g. `SLSKD_URL=http://gluetun-slsk:5030`.
- **Built-in client through a VPN**: run Encore in gluetun's network namespace (`network_mode: container:gluetun`, publish 8080 on gluetun with `FIREWALL_INPUT_PORTS=8080`) and set `SOULSEEK_PORT_FILE=/gluetun/forwarded_port` with gluetun's folder mounted read-only – the port the VPN forwards is used as the Soulseek listen port and followed when it changes. The commented block in `docker-compose.yml` has the full example.
  - **slskd permissions:** slskd ignores `PUID`/`PGID`. Run it with `user: "1000:1000"` and `chown -R 1000:1000` its config, downloads and incomplete folders, or it can't write to them.
  - Since slskd 0.26.0 the top-level `permissions:` key in `slskd.yml` moved to `transfers.download.destination.permissions`; an old config stops slskd from starting (see Troubleshooting).

**Torrents & Usenet (optional)**: add your indexers to Prowlarr, then in *Settings → Apps & sources → Torrents & Usenet* enter the Prowlarr URL and API key, plus qBittorrent and/or SABnzbd. Encore searches Prowlarr, sends the best album release to the right client, and imports it when it finishes. This replaces Lidarr.

**Optional keys** (Settings):

| Feature | Where to get it |
|---|---|
| Audio verification | Free AcoustID application key: acoustid.org/new-application |
| Qobuz/Tidal/Deezer lossless | Your own subscription. Enter the Deezer ARL or Qobuz token in *Settings → streamrip*. Tidal needs a one-time sign-in: `docker exec -it encore streamrip --config-path /config/streamrip/config.toml url <tidal link>`. These services' terms may not allow downloading, so use at your own risk. |
| Spotify account connection | developer.spotify.com app with redirect `<Public URL>/api/connections/spotify/callback`. Since Feb 2026, development-mode apps need a Premium owner and allow 5 users. Public playlists still import without it, via the embed page. |
| Tidal import | developer.tidal.com app (redirect `<Public URL>/api/connections/tidal/callback`) |
| Last.fm scrobbling, For you, loved tracks, similar artists/tracks, genres | last.fm/api/account/create (free). Enter the API key **and shared secret**; set the app's callback URL to `<Public URL>/api/connections/lastfm/callback`. Users then sign in with Last.fm on *Connections*. |
| Discogs catalog (releases MusicBrainz lacks) | discogs.com/settings/developers → personal access token (free) |
| YouTube Music sign-in | Google Cloud, YouTube Data API v3, an OAuth client of type *TVs and Limited Input devices*. Users can paste browser request headers instead. |
| YouTube Music Premium audio (256 kbps) | Paste a `cookies.txt` exported from music.youtube.com under *Settings → YouTube Music*, then press *Test cookies*. |

Set **Settings → General → Public URL** (for example `https://music.example.com`) so the OAuth logins and invite links work.

## Troubleshooting

**Admin → Diagnostics** runs every connection, folder-permission and tool check from inside the container and shows the raw responses, with keys hidden. *Copy report* gives you text you can paste into a bug report. `curl` is also included in the image, e.g. with slskd: `docker exec -it encore curl -s http://slskd:5030/health`.

| Symptom | Fix |
|---|---|
| A setting won't change in the UI | It's set by an environment variable (the field shows 🔒 and the variable name). Environment variables always win, so change it in your compose file. |
| Upgrading from an older Encore image | Switch the image to `:latest` and keep the same `/config`: the database is shared. Lidarr settings are ignored; set up Prowlarr instead if you want torrents/Usenet. |
| Soulseek: `Not signed in: login refused: INVALIDPASS` | The username already exists on Soulseek with another password. Pick a new username (the account is created at the first sign-in) or enter the right password. |
| `401 … API key rejected` (slskd) | The key in Encore doesn't match `web.authentication.api_keys` in `slskd.yml`, or the key's `cidr` doesn't include Encore's IP. Don't put quotes around values in `- KEY=value` lines. |
| `Can't find host 'slskd'` | Use the service or container name on the same Docker network. Behind gluetun, use the gluetun container's name. |
| slskd: `/incomplete` (or downloads) not writable | Run slskd with `user: "1000:1000"` and `chown -R 1000:1000` its folders. |
| slskd won't start after an update: "permissions" error | Since slskd 0.26.0 the top-level `permissions:` key moved to `transfers.download.destination.permissions` (slskd PR #1756), and an old config stops slskd from starting. Move the block. The mode now also applies to folders. |
| Folder not writable by Encore | Diagnostics shows the folder owner and Encore's uid. Match them with `PUID`/`PGID` or `chown`. |
| Config ends up somewhere odd under Portainer | Use absolute host paths (`/opt/appdata/encore:/config`), not `./encore`. |

## Keeping it private

- Publish **only Encore**: port 8080 for the web UI and port 2234 for Soulseek peers. If you run slskd, the compose file keeps its web UI off the public internet; slskd only exposes its own Soulseek peer port.
- Put it behind HTTPS. For example, with Caddy:

  ```
  music.example.com {
      reverse_proxy encore:8080
  }
  ```
- Sign-ups default to **invite only**. Sessions use HttpOnly SameSite=Lax cookies, every mutating request needs a CSRF header, and failed logins are rate-limited per IP.
- The **Encore API key** (Settings → General, bottom) acts as the owner account. Send it as an `X-Api-Key` header; regenerate it if it leaks.
- Saved keys and passwords are never sent to the browser with the settings. The eye and copy buttons fetch one at a time, only for the owner, and each view is written to the activity log. Values set by environment variables can be viewed but not changed in the UI.

## Configuration

Every option can be changed in the UI. Environment variables only seed the first start.

| Variable | Default | |
|---|---|---|
| `PUID` / `PGID` | 1000 | User the app runs as |
| `ENCORE_ADMIN_USER` / `ENCORE_ADMIN_PASSWORD` | admin / random | First admin account |
| `ENCORE_LIBRARY_PATH` | `/music` | Music library inside the container |
| `ENCORE_SIGNUP_MODE` | `invite` | `invite`, `open` or `closed` |
| `ENCORE_DISCOVER_AUTO_COUNT` | 0 | Songs to auto-request from each Weekly Exploration (0 = off) |
| `ENCORE_CATALOG_MERGE` | `true` | Show Deezer/Apple releases MusicBrainz lacks |
| `ENCORE_MANUAL_SEARCH` | `everyone` | Who can use the manual download picker: `everyone` or `admins` |
| `ENCORE_SOURCE_ORDER` | `qobuz,tidal,deezer,slskd,torznab,ytdlp` | The one search order: `qobuz`, `tidal`, `deezer` (through streamrip), `slskd`, `torznab`, `ytdlp` |
| `ENCORE_NAMING_TEMPLATE` | `{albumartist}/{album} ({year})/{discprefix}{track:02d} - {title}` | Rename template |
| `TORZNAB_ENABLED`, `PROWLARR_URL` / `PROWLARR_API_KEY`, `QBIT_URL` / `QBIT_USER` / `QBIT_PASSWORD`, `SAB_URL` / `SAB_API_KEY` | | Torrents & Usenet through Prowlarr |
| `SLSKD_ENABLED` | `false` | Master switch for Soulseek, whichever client is used |
| `SOULSEEK_CLIENT` | `auto` | `auto` (built-in as soon as a username is set, otherwise slskd), `builtin` or `slskd` |
| `SOULSEEK_USERNAME` / `SOULSEEK_PASSWORD` | | Soulseek login for the built-in client; a new username and password create the account at the first sign-in |
| `SOULSEEK_PORT` | 2234 | TCP port the built-in client listens on – forward it on your router |
| `SOULSEEK_SHARE` / `SOULSEEK_UPLOAD_SLOTS` | `true` / 2 | Share the library (read-only) back to Soulseek, and how many uploads at once |
| `SLSKD_URL` / `SLSKD_API_KEY` | `http://slskd:5030` | Only for the slskd client |
| `SLSKD_DOWNLOAD_PATH` | `/downloads/slskd` | Soulseek downloads folder, for both clients |
| `YTDLP_AUTO_UPDATE` | `true` | Upgrades yt-dlp at every container start |
| `SPOTIFY_CLIENT_ID` / `SPOTIFY_CLIENT_SECRET`, `TIDAL_CLIENT_ID` / `TIDAL_CLIENT_SECRET`, `LASTFM_API_KEY` / `LASTFM_SHARED_SECRET`, `DISCOGS_TOKEN` | | Account connections, imports, discovery, catalog |
| `ACOUSTID_API_KEY` | | Audio fingerprint verification |
| `STREAMRIP_ENABLED`, `STREAMRIP_DEEZER_ARL`, `STREAMRIP_QOBUZ_USER` / `STREAMRIP_QOBUZ_TOKEN` | off | Qobuz/Tidal/Deezer source |
| `ENCORE_SYNC_INTERVAL` | 15 | Minutes between like/playlist syncs |
| `NAVIDROME_*`, `JELLYFIN_*`, `PLEX_*` | | Library refresh after imports |
| `DISCORD_WEBHOOK`, `NTFY_URL`, `WEBHOOK_URL` | | Notifications |

## Development

```bash
cd server
cargo test
ENCORE_CONFIG_DIR=../data ENCORE_STATIC_DIR=../web ENCORE_LIBRARY_PATH=~/Music ENCORE_PORT=8080 cargo run
```

- `server/`: the server (Rust: axum, SQLite, lofty for tags, Chromaprint via `fpcalc`). yt-dlp and streamrip are run as programs; the Docker image includes them.
- `web/`: the web app, plain ES modules with no build step. Edit and reload.
- `clients/app/`: the desktop and Android app (Rust, Tauri) – `cargo tauri dev` or `cargo tauri android dev` in that folder; `plugins/media/` is the Kotlin half (media notification, share intent, downloads).
- **Building the apps for a release on your own machine** (no Actions minutes): `scripts/release-apps.sh` builds this OS's installer plus the Android APK and attaches them to the newest GitHub release (`scripts/release-apps.sh 0.20.3` for a specific one; `--android` / `--desktop` for one of the two, `--no-upload` to only build into `out/`). Windows builds the `.exe` from Git Bash, Linux the `.AppImage`/`.deb`/`.rpm`, a Mac the `.dmg`; the APK builds anywhere the Android SDK + NDK and a JDK are installed. Needs the `gh` CLI signed in.
