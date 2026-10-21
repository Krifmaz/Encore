# Encore

A self-hosted music request and library app. Your users sign in, search the whole MusicBrainz catalogue, request artists, albums or songs, and play what is already in your library. All of it happens in one modern web UI. Behind the scenes, Encore has its own Soulseek client and runs your torrent/Usenet indexers (through Prowlarr), streamrip and yt-dlp for you, each one a plugin you install and update with one click. Only Encore needs to be reachable from the internet.

Encore is a single Rust program with the web app built in: fast, small, and no Lidarr needed.

## Features

- **Accounts**: login and sign-up with invite-only, open or closed modes. The admin panel manages users, roles, weekly request limits, auto-approve, password resets and session revocation.
- **Discover**: search MusicBrainz for artists, albums and songs, with *In library* and *Requested* badges. Request anything with one click.
- **Request pipeline**: with *highest quality first* on, each request tries **Qobuz/Tidal/Deezer (streamrip) → FLAC on Soulseek (24-bit preferred) → torrents/Usenet via Prowlarr → MP3 on Soulseek → YouTube Music**, falling back automatically. Exact ISRCs are used whenever the source provides them.
- **Whole albums**: when a requested song is on an official studio album or EP, Encore downloads the whole release instead, in the best quality any source has. Compilations, live albums and soundtracks are never used. If the album doesn't have that exact version (say, a remix), the song is fetched on its own.
- **For you**: discovery only – *Artists for you*, *Albums for you* and *Songs for you* built from what you play most on Last.fm (or in Encore), plus your ListenBrainz *Weekly Exploration*; anything already in the library is left out, everything shown is one click to request. **Made for you** on the home page (once ListenBrainz is connected) plays your *Daily Jams*, *Weekly Jams* and *Weekly Exploration* playlists with what you own and requests the rest. *Admin → Auto-download* can grab the top N new songs from each week's exploration list automatically.
- **Scrobbling**: what you play in Encore is scrobbled to Last.fm and ListenBrainz ("now playing" when a song starts, a scrobble after half the song or 4 minutes). Each user switches it on or off on the *Connections* page.
- **Group listen**: several people on this server hear the same queue at the same time, each on their own device and with their own quality settings. Start one from the player's ⋯ menu or *Your devices*; others join with the code, the invite link or Home's *Listening together now*. The host controls playback unless *Everyone can control* is on, everyone adds songs to Up next (showing who added each one), and a pause on your own device pauses only you. With the admin's and the host's permission, people without an account can listen from the link on a web page.
- **Similar artists and "More like this"**: similar artists come from Last.fm, falling back to ListenBrainz and then Deezer. ✨ on any song shows similar tracks from Last.fm, or a Deezer radio without a key. Genre tags open genre pages with top artists from Last.fm, or from MusicBrainz without a key.
- **Fills MusicBrainz gaps**: artist pages also check Deezer, Apple Music, YouTube Music, Bandcamp and Discogs (vinyl, bootlegs and old releases; needs a free token or a Discogs link on MusicBrainz). Releases are matched by barcode (UPC), then ISRC, then title, year and track count; anything MusicBrainz doesn't list shows as *Not in MusicBrainz*. Those requests download from Soulseek, streamrip, or the exact YouTube Music/Bandcamp release, tagged from the source's data. *Add to MB* opens Harmony to import the release into MusicBrainz, and Encore re-tags it automatically once it's there.
- **Artist pages**: the full MusicBrainz discography (albums, EPs, singles, compilations, live, soundtracks, remixes…) with *All / Missing / In library* filters and *Request N missing* per section. *Request discography* opens a dialog with every release on the page grouped by type (albums, EPs, singles, live, compilations …), a box per release, *Select all / Deselect all* per group and overall and a *Request N releases* button that counts the ticks; the boxes start ticked for what Settings → Downloads → Release types allows (like Lidarr's metadata profiles: Album, EP, Single … and the extras to skip – live, remix, compilation, interview, instrumental, sped up, karaoke, covers …; env `RELEASE_TYPES` / `RELEASE_SKIP` as CSV) and isn't owned or requested yet, and only the ticked releases are requested (`release_ids` on the artist request; the older `release_filter` {kinds, skip} still works); skipped kinds hide behind a *+N hidden* chip on the artist page and in New releases, and song requests never pick another version (live, remix, sped up, edit …) than the one asked for. Each page also shows the artist photo, a Wikipedia bio, popular tracks (play the ones you have, request the rest), *Liked from this artist*, *In your library* (the songs and albums you have, including ones MusicBrainz doesn't list), and Play/Shuffle. Library artists open the same page.
- **Report a problem** (⋯ on songs, albums and the player): wrong song, wrong version, bad or quiet audio, fake lossless, glitches. Encore blocks that exact source (the Soulseek file, torrent, Usenet post, streaming item or YouTube video), keeps the file in `config/trash` for 7 days and fetches the music again from somewhere else; the new copy keeps the song's plays, likes and playlist spots. Every download is checked before it enters the library – fake lossless (a lossy file saved as FLAC), upsampled hi-res and the wrong length are turned away and the next copy is tried, very quiet or clipped audio is flagged – and the *Quality scan* task (off by default) flags suspect songs already in the library. Settings → Blocked lists reports, blocks (with Undo) and flagged songs.
- **Lossless upgrades**: albums you only have as MP3 or YouTube rips can be re-checked periodically. A lossless copy replaces the old files, and playlists follow it automatically.
- **YouTube Music**: playlists, albums and your Liked Music import with proper artist and album data. Connect your account for two-way 👍 sync and playlist push. Songs and whole albums are downloaded from official YouTube Music uploads in the original stream quality (256 kbps with Premium cookies).
- **Admin → Auto-download** controls what downloads on its own (liked songs from any app, new tracks in followed playlists), the album and quality rules, lossless upgrades, and which users have it switched on.
- **Manual download picker**: *Pick download* on any album or song, or *Pick* on a request. It searches Soulseek, Qobuz, Tidal, Deezer, YouTube Music and your Prowlarr indexers at once and shows each result's quality (Qobuz says the exact bit depth and rate, "FLAC 24/192"; Tidal its tier, hi-res, MQA or 16/44.1), size, file count and uploader queue/speed. Every source has a tab: one that is signed out, not connected or switched off says so with a link to its settings instead of disappearing (Qobuz and Tidal search with the account streamrip is signed in with; Tidal with your developer Client ID when one is set). You choose the exact one, or paste any link. Admins can limit it to admins in Settings.
- **Soulseek page** (menu → Soulseek, for everyone allowed to pick downloads): search the network directly, browse any user's shared folders, download whole folders or a tick-list of files (they become ordinary requests, tagged and imported like any other), and watch downloads and uploads under *Transfers*. Runs on the Soulseek engine Encore manages (the *Soulseek (slskd)* plugin, Settings → Plugins).
- **Song or whole album, per request**: in Discover, *Song* or *+ Album*. In the Downloader, *Just the song / Get the whole album / Admin default*, which also applies to playlist imports.
- **Downloader**: a Spotify, Apple Music, Deezer, Tidal or Qobuz link (playlist, album or track) is matched track by track, and each song is fetched from the best source. YouTube, SoundCloud, Bandcamp, Mixcloud and about 1,800 other sites download directly.
- **Download now**: each source's line is visible on the Requests page ("2nd in line for Deezer", streamrip one download at a time on each of Deezer, Qobuz and Tidal, yt-dlp two, …). *Download now* moves a request to the front of its line, tries a storage or Soulseek wait at once (unless the Soulseek server itself asked Encore to wait), switches a Soulseek download stuck in someone's upload queue to a user with a free slot, or gives up a torrent with no progress for the next source – never more at once than a source allows. Admins have *Download all now*.
- **Disk problems wait instead of failing**: every folder downloads go into is checked (read-only, full, missing, not writable by Encore); while one can't be written, the requests that save there wait ("Waiting for the downloads folder"), admins see one banner on every page with what to do, and Diagnostics lists each folder. Imports into a library disk with a problem wait the same way, with the downloaded files kept.
- **Playlists and Liked Songs**:
  - Import from Spotify (including private playlists and Liked Songs once you connect your account), Deezer, Tidal, Apple Music, Qobuz, YouTube Music, SoundCloud, Bandcamp, ListenBrainz, or a pasted "Artist – Title" list.
  - Merge playlists, follow a source so new tracks are pulled in automatically, and "Download missing" in one click.
  - **Share** any playlist, Liked Songs included: the link always plays the playlist as it is now (a web page with Play and Shuffle, no account needed) until you press *Stop sharing*. People on your server can **add a shared playlist to their own Playlists**, where it follows the owner's changes and stays read-only, or save a copy; the owner sees how many added it, never who.
- **Sync everywhere**: each user connects their own accounts on the *Connections* page. Liked songs sync two-way (likes *and* un-likes) between Jellyfin/Finamp, Plex/Plexamp, Navidrome and ListenBrainz. Playlists are mirrored to whichever servers you tick.
- **Correct versions, no duplicates**:
  - One version-aware matcher is used everywhere. It tells originals apart from remixes (including *which* remixer), live, acoustic, radio and extended edits, instrumentals, slowed/sped-up versions, re-recordings ("Taylor's Version") and covers by other artists. Remasters and "feat." credits count as the same song.
  - Downloads are fingerprinted with **AcoustID/Chromaprint** before import. Wrong audio is rejected and the next source is tried.
  - A song that's already in the library is not imported a second time; a better-quality file replaces the old one. The *Duplicates* tab finds identical copies already in the library, and *Wrong audio* lists files whose content doesn't match their tags.
- **Web player**: play albums, songs and playlists in the browser. MP3, FLAC, AAC and Opus stream as the original file, with no conversion and full seeking. ALAC, APE, WavPack, AIFF, WMA and DSD are converted to FLAC on the fly, which keeps them lossless; whatever a browser can't decode is converted automatically. **Settings → Playback** sets the quality: *Auto* plays the original, measures the connection while it plays and steps the stream down as far as needed (down to 96 kbps AAC/Opus, 64 kbps only on a very poor connection), then back up as soon as the connection allows; a stream that breaks off mid-song (a dropped mobile connection, a locked phone) is picked up again at the same spot instead of skipping to the next song, *Original* never converts, *Data saver* always uses the small stream (AAC/Opus 96 kbps); the next song is pre-loaded once the current one is safely buffered. **Sound (on this device)** adds volume normalization (track or album, a target level, from the songs' ReplayGain tags or Encore's own measurement), a parametric EQ with presets, headphone crossfeed, balance, mono and crossfade (0–12 s, with an optional *Keep albums gapless*). They all run in the listener's browser, so they cost the server nothing, and the Signal path says when they're on. The quality badge opens the **Signal path**: what the file is (codec, bit depth, sample rate), what Encore sent (untouched, FLAC, or lossy and why) and what the device does with it. The full player has a **lyrics mode** that gives synced lyrics the whole column (a tap on a line seeks to it), and long titles scroll.
- **Offline**: the website keeps working when the server can't be reached. ⋯ → *Download for offline* on a song, an album, a playlist or Liked Songs saves the songs in the browser with their covers and lyrics (the original file when the browser plays it, otherwise a converted copy marked "(converted)"); **Downloads** in the side bar (below Library) lists them with the space used, *Make persistent* and *Remove*. When the server is away, a browser that was signed in during the last 30 days opens on the Downloads page with everything else greyed out, plays the saved songs, and sends the plays and likes made meanwhile once the server is back, with the times you listened. Downloaded songs also play from the device while online (Settings → Playback → *Play downloaded songs from this device*). It needs HTTPS (or `localhost`): browsers allow offline storage only on secure addresses. On an iPhone or iPad, add Encore to the Home Screen and download from there: Safari clears a website's storage after a week without a visit, and the Home Screen app keeps its own (downloads made in a Safari tab don't follow it). The browser also keeps the app itself, and says "A new version of Encore is ready" with *Reload* after you update the server. The desktop app has its own offline songs; the phone app's come with 0.30.
- **Remote control**: every signed-in device (phone, browser tab, desktop app, the car) shows up under *Your devices*. See what each one plays, control it from the full-screen player, send what you're playing to a different device, or **link** one as a locked remote for another until you unlink – commands and state are pushed instantly over the server's event stream. Share links carry the song's title, artist, album, cover and quality, so chat apps preview them.
- **Cadenza** ([github.com/Krifmaz/cadenza](https://github.com/Krifmaz/cadenza)): Encore's own upsampling and bit-perfect playback engine, built into the desktop app. Install the app on the computer connected to your DAC, sign in, and music plays through Cadenza straight into the DAC – exclusive mode, long upsampling filters, dither and noise shaping, DSD output up to DSD2048, optional GPU (NVIDIA) processing – while the window shows the normal Encore app. In the app, Settings → Playback → *Play through Cadenza on this device* turns the processing on and opens its card (output device, exclusive mode, PCM and DSD rates limited to what the device takes, link limit, filter, dither, noise shaping, headroom, EQ, convolution, *Test sound*); switched off, the engine plays the file bit-perfect. The Signal path shows every stage the engine ran. Cadenza can also run on its own on a small box next to the DAC (`cadenza serve`); link to it under *Your devices* and the web app's Settings → Playback → *Cadenza engine* section controls it.
- **Server processing with Cadenza (optional, off by default)**: an admin can let the server upsample streams for browsers and phones with Cadenza's filters (Settings → Server processing). It needs the *Server processing* permission per user, and has limits for streams at once, highest rate and longest filter. Each song is rendered once into a disk cache (size limit, *Clear cache*), the next song is rendered ahead, and seeking in a rendered song happens on the device – no new work on the server.
- **Roon**: Encore registers as a Roon extension (approve it once in Roon → Settings → Extensions). Your Roon zones then appear under *Your devices* with what they play, play/pause/next/previous and volume, and *Play in Roon…* searches Roon's library and starts the first match in a zone. Settings → Integrations → Roon; in Docker enter the Core's address, since multicast discovery doesn't cross the bridge network.
- **Android Auto**: the Android app is a media app for the car – browse recently added, Made for you, playlists, liked songs, albums and artists on the car screen, ask for a song or artist by voice, and the car player shows up under *Your devices* so you can push a queue to it from any browser. The now-playing row has Shuffle, Repeat, Like and Lyrics, and *Car buttons* on the phone reorders them, moves them behind ⋯ or adds *Artist* (more songs by the artist after this one) and *Signal path*. The quality shows under the title ("FLAC 24/96"), Browse starts with *More by <artist>*, live lyrics show large in the cover, and the second car app, **Encore Lyrics**, puts the song's lyrics over the whole screen.
- **Where it came from**: every downloaded track shows its source on the album page (Soulseek user and folder, YouTube Music link, Deezer/Tidal/Qobuz release, torrent/Usenet release or a direct link), when it was downloaded, who requested it and when, and when it was added.
- **Artwork for everything**: Cover Art Archive first, then the request's own cover, then Deezer, iTunes or YouTube Music. Art is embedded in the files so Finamp and Plexamp show it too. *Library → Find missing artwork* fixes existing albums.
- **Library**: browse albums, artists and songs with one search over all three (initials work: "crazy gb" finds Crazy by Gnarls Barkley; Discover shows library matches first), every artist of a song is its own link and a song or album by several artists ("Kanye West & Ty Dolla $ign", "A feat. B") counts under each of them instead of becoming an artist of its own (split by the files' ARTISTS tags or MusicBrainz's credits, which keep bands like "Simon & Garfunkel" whole, else by the artists already in the library), match albums to MusicBrainz with an old → new preview, write full tags and cover art, rename files using your template, bulk auto-tag, edit tags by hand, and download albums as zips.
- **Translated titles**: song, album and artist names in another script shown translated or in Latin letters, for each listener who turns it on (Settings → Language). They come from corrections by hand, official names (MusicBrainz, Apple), romanization built into the server (Japanese kana, Korean, Cyrillic, Greek and more), **DeepL** with your API key (titles only, with a monthly character budget), and last the optional local model (the Translator plugin). Settings → Language shows each source's state.
- **File manager** (admins, Settings → System → Files, also in the desktop and phone apps): browse the library, staging, the download folders and Trash; upload files and whole folders, make folders, rename, move (likes and playlists follow the songs), move to trash and restore, edit a file's tags, and *Import…* a folder into the library, either matched and moved in or kept where it is (*Keep the files where they are* adds it to *Extra library folders*, which Encore scans but never moves, renames or re-tags). Nothing outside those folders can be reached, and only Trash deletes for good.
- **Plugins** (admins, Settings → Plugins, also in the desktop and phone apps): the download sources (yt-dlp, streamrip, Soulseek, Torrents & Usenet), the torrent and Usenet clients, Cadenza server processing, the Translator and the Jellyfin, Plex and Navidrome connections are plugins. Each one installs, updates, is switched off, tested and removed on its own, with its settings and its log on its card, and needs no new Encore image. Plugins written by other people install with *Upload plugin* or *Install from URL* and run in a sandbox. See [Plugins](#plugins).
- **Integrations**: Navidrome/Jellyfin/Plex rescans after imports, Discord/ntfy/webhook notifications, and an API key for scripts.
- **Make it yours**: themes (dark, OLED black, light, match device), presets, any accent colour, and a logo that follows it.

## Quick start

```bash
git clone https://github.com/Krifmaz/Encore encore && cd encore
cp .env.example .env        # set paths, admin password, API keys
docker compose up -d --build
```

Open `http://your-server:8080` and sign in with `ENCORE_ADMIN_USER` / `ENCORE_ADMIN_PASSWORD`. If you didn't set a password, one is generated and printed in `docker logs encore`, and saved to `config/encore/initial_admin_password.txt`.

Then go to **Settings → Plugins**, install the sources you want (yt-dlp is one click), fill in their cards and use **Test**. A new install has no plugin installed, so the download sources' cards appear once their plugin is. Invite people from **Invites**.

### Prebuilt image

GitHub Actions publishes `ghcr.io/krifmaz/encore` for amd64 and arm64:

| Tag | What it is |
|---|---|
| `:latest` | The newest release |
| `:0.20.1` … | One exact release |

Every release is a normal GitHub release, tagged `v<version>` (plain MAJOR.MINOR.PATCH numbers), with the notes, server archives, compose file and Android/desktop installers attached: [github.com/Krifmaz/Encore/releases](https://github.com/Krifmaz/Encore/releases). If the repository is private, the image is too – run `docker login ghcr.io` once with a token that has `read:packages`.

### Backups

Encore backs up its database by itself: a copy is taken **before every upgrade** that changes the database, **once a day**, and before a restore. They live in `/config/backups` in the container (`config/encore/backups` on the host with the compose file above) as `encore-YYYYMMDD-HHMMSS.db`, with `-before-upgrade` on the pre-upgrade ones. The last 7 days are kept, then one a week for 8 weeks, plus the last 3 pre-upgrade copies. **Settings → Diagnostics → Backups** lists them with *Back up now*, *Download*, *Restore…* and *Upload a backup…* – download one now and then and keep it somewhere else.

To restore: *Diagnostics → Backups → Restore…* (Encore takes a safety copy of the current data first, then reopens the database – everything saved since that backup is replaced). Or, with the container stopped, copy the file over `config/encore/encore.db` (and delete `encore.db-wal` / `encore.db-shm` if they exist) and start it again.

A database written by a newer Encore is never touched by an older one: the server stops with `This database was written by a newer Encore (schema N, this server knows M) …` (exit code 3) and tells you to start the newer image again or restore a backup. `:latest` now always means the newest release, so switching back to it after trying a pinned tag never runs an older server against a newer database. Going back to 0.27 or older also deletes the songs downloaded in browsers and the plays and likes they have not sent yet (the old service worker clears them); open Encore online once first, so those are sent.

### Updates and restart

**Settings → Diagnostics → Updates** (admins) shows the version you run and the newest release on GitHub with its notes; the server checks once a day (the *Check for Encore updates* task; `ENCORE_CHECK_UPDATES=false` turns it off), and the Settings page says so when there is a new one. If the repository is private, the check needs the *GitHub token* setting (Settings → General) with read access to it; without one GitHub answers as if there were no release. *Restart server* stops Encore cleanly and starts it again inside the container: the image runs the server in a loop where exit code 75 means "start again" and any other exit code ends the container as before, so `restart: unless-stopped` behaves as it always did. Run from the Linux or Windows archive, *Restart server* ends the process with exit code 75 and your service manager starts it again (systemd: `Restart=on-failure` and `RestartForceExitStatus=75`).

*Update now* pulls the new `ghcr.io/krifmaz/encore` image and replaces the container with one with the same name and settings (environment, volumes, ports, networks, restart policy, labels), Watchtower-style, through a short-lived helper container whose steps `docker logs` shows; if the new one can't be created or started, the old one is started again, and once the new one runs the old one is removed (its volumes stay). It works from 0.28.0 on, with the public image (the pull carries no registry login). It needs the Docker socket mounted into the container – optional, and without it the card only says why the button is missing:

```yaml
services:
  encore:
    image: ghcr.io/krifmaz/encore:latest
    restart: unless-stopped
    volumes:
      - /opt/appdata/encore:/config
      - /srv/media/music:/music
      - /srv/downloads:/downloads
      # Optional, only for "Update now". Warning: the socket gives the container control of Docker on this host;
      # mount it only if you want Update now.
      - /var/run/docker.sock:/var/run/docker.sock
```

The socket gives the container control of Docker on this host; mount it only if you want *Update now*. With the socket mounted, every program Encore runs (slskd, yt-dlp, streamrip, Cadenza, the translator) has the same access to Docker, and so does a plugin you add when the plugin sandbox is at *Limits only* (in full isolation it does not see the socket). Encore runs as `PUID`/`PGID`, so the socket must be writable by that user: when the container starts, the image adds Encore's user to the group that owns the socket. A socket owned by root's group (gid 0) is the exception: the card then says the socket is not writable; set `PGID=0` (Encore's files then get that group too) or `PUID=0`. Set `ENCORE_SELF_UPDATE=off` to hide the button even with the socket mounted; `off` also keeps Encore's user out of the socket's group, so nothing Encore runs can use the socket. Without the socket, update as before: `docker compose pull && docker compose up -d`.

**Hotfixes.** Every push to the main branch builds a hotfix: a GitHub prerelease `hotfix-<last release>.<n>` (e.g. `hotfix-0.35.2.17`) with the server archives, and the image `ghcr.io/krifmaz/encore:hotfix` (`:0.35.2.17` pins it). The channel picker on the Updates card (`ENCORE_UPDATE_CHANNEL=hotfix`) makes *Update now* offer them; on *Stable releases*, the default, only full releases count. A full release is newer than every hotfix before it (0.35.3 > 0.35.2.17), so a hotfix server moves on to it. Only the newest three hotfixes are kept on GitHub, and after *Update now* the old image is deleted from the host (with Encore's untagged leftovers; an image another container still uses stays). The desktop and Android apps come with full releases only.

## Apps

- **Android**: download `Encore-android-<version>.apk` from the [Releases page](https://github.com/Krifmaz/Encore/releases), install it, and pick your server (e.g. `http://192.168.1.10:8080`). It is a native Android app (Kotlin, Media3, Cadenza on the phone): music keeps playing with the screen off, with lock-screen and notification controls; *Share → Encore* from Spotify, YouTube Music or a browser downloads that link; songs and albums can be saved for offline listening, and when the server is away the app opens its offline player.
- **Android Auto**: the app includes a car media service (browse and voice search of your library, native playback straight from the server). Because the APK is not installed from Google Play, Android Auto hides it until you allow it once: open the **Android Auto** app on the phone → scroll to *Version*, tap it about ten times until *Developer settings* unlocks → ⋮ → *Developer settings* → turn on **Unknown sources**. Then *Customize launcher* and tick Encore. Open Encore on the phone once first so it knows your server and sign-in; the car then starts it on its own.
- **Desktop** (Windows, macOS, Linux): same Releases page – `Encore-<version>-windows-x86_64.exe` (installs for your user, no administrator rights), `Encore-<version>-macos-universal.dmg`, `Encore-<version>-linux-x86_64.AppImage` or `encore_<version>_amd64.deb`. A native Rust app (Iced, no web view) with the same pages as the web app, every song playing through Cadenza on the computer, and the computer as one of *Your devices*. Media keys and the system's media controls work (lock screen, Now Playing, MPRIS), there's a tray and a mini player, it remembers your servers, keeps songs for offline and updates itself from the release (checked against `SHA256SUMS`). If macOS will not open it the first time: System Settings → Privacy & Security → Open Anyway. It replaces the classic Tauri desktop app, whose last release was 0.27 (0.29 removed it): if you still have that one, install this app instead. See [clients/desktop/README.md](clients/desktop/README.md).
- **Any phone**: open Encore in the browser and choose *Add to Home screen*. On HTTPS it installs as an app.
- **Web interface in Rust**: the web app is the Rust/WebAssembly one (`web-rs`), served at `/` (the old `/next/` address redirects there). A browser that blocks WebAssembly for the site (Brave Shields, Vanadium with JIT off, iOS Lockdown Mode, Edge's enhanced security) gets a message saying how to allow it.

## Plugins

**Settings → Plugins** (admins; also in the desktop and phone apps) is where Encore's download sources, download clients, tools and connections are installed. Each one installs, updates, is switched off, tested and removed on its own; its card has its settings, *Test*, *Logs* and an *Update automatically* switch. Programs go into `/config/plugins/<id>/`, so they survive a new image and no new image is needed for a new yt-dlp.

| Plugin | What it does |
|---|---|
| yt-dlp | YouTube Music for requests, and links from YouTube, SoundCloud, Bandcamp, Mixcloud and about 1,800 other sites; the YouTube sign-in is part of its card |
| streamrip | Qobuz, Tidal and Deezer with your own accounts, lossless |
| Soulseek (slskd) | The Soulseek engine Encore runs for you, with the VPN for Soulseek |
| Torrents & Usenet (Prowlarr) | Your indexers through Prowlarr; with a client plugin each: qBittorrent, Transmission, Deluge, rTorrent / ruTorrent, a torrent watch folder, SABnzbd, NZBGet, a Usenet watch folder |
| Cadenza (server processing) | The Cadenza program for upsampling streams on the server (its settings stay on Server processing) |
| Translator | llama.cpp and a local model for the translated and romanized names the other sources leave (official names, the built-in romanization and DeepL work without it) |
| Jellyfin, Plex, Navidrome | The connections to your other music server (their settings stay on Integrations) |

- **A fresh install** starts with every plugin available and none installed: press *Install* on the ones you want (yt-dlp is one click; the picker's tabs say which plugin a source needs).
- **An upgrade from 0.30 or older** keeps working with no new setup: what you used (yt-dlp, streamrip, Soulseek, Cadenza, the Translator when its model is there, Prowlarr and your client, your media server) is taken over where it is in `/config` (nothing is moved), switched on with its settings, and a program that is missing is installed once in the background.
- **Updates**: the *Update plugins* task (Settings → Scheduled tasks, daily; it was *Update download tools*) checks every installed plugin and installs new versions for the ones whose *Update automatically* is on. *Check for updates* on the page checks at once; *Update* on a card installs.

### Plugins you add

*Upload plugin* takes a plugin's zip (up to 200 MB); *Install from URL* takes the https address of a zip or of a `plugin.toml`, or a GitHub repository as `owner/repo@ref` (`owner/repo/folder@ref` for a plugin in a folder; the repository must be public). A plugin you add is marked *Third-party*, and its card says what it can reach. Install only plugins you trust. To write one, start with [docs/plugins/](docs/plugins/README.md) and the example plugin in [docs/plugins/example-tone/](docs/plugins/example-tone/), which you can install with *Install from URL* and `Krifmaz/Encore/docs/plugins/example-tone@main`. Encore's catalog holds no plugin whose purpose is removing DRM.

### The plugin sandbox and the compose option

The sandbox line on Settings → Plugins says how plugins you add are run (the catalog's own programs run as before either way):

- **Full isolation**: each runs under bubblewrap and sees only its own folders, the download it is writing and, when it asks, your music read-only and the network. Not your config folder, database or saved passwords.
- **Limits only, see the compose option**: limits on memory, processor time and processes, low priority and a cleared environment, but it runs as Encore's user and can read what Encore can.

Docker's default security profiles stop bubblewrap, so a plain container gets *Limits only*. For full isolation, let the container make namespaces:

```yaml
services:
  encore:
    image: ghcr.io/krifmaz/encore:latest
    security_opt:
      - seccomp=unconfined     # Docker's seccomp profile allows namespaces only to containers with CAP_SYS_ADMIN
      - apparmor=unconfined    # only on hosts with AppArmor (Ubuntu, Debian): its profile forbids the mounts bubblewrap makes
```

(`docker run`: `--security-opt seccomp=unconfined --security-opt apparmor=unconfined`; Unraid: the same in *Extra Parameters*.) This switches off those two Docker protections for the whole container, so choose it when you add plugins written by other people; with the catalog's plugins alone, *Limits only* is fine. `cap_add: SYS_ADMIN` or `privileged: true` are not needed and give far more. If the line still says *Limits only*, the words under it name the reason; [docs/plugins/SANDBOX.md](docs/plugins/SANDBOX.md) lists each one (Ubuntu 23.10 and newer hosts, `/proc`, running outside Docker).

## Wiring up the services

Encore and the download clients must see the **same files at the same paths**:

| Host folder | Encore | qBittorrent / SABnzbd |
|---|---|---|
| Music library | `/music` | – |
| Downloads | `/downloads` | `/downloads` |

**Soulseek** is run by Encore, inside the Encore container: no separate container. Encore uses [slskd](https://github.com/slskd/slskd) as its Soulseek engine, downloads it itself and runs it headless (no web UI of its own) on 127.0.0.1. The first time, open *Settings → Plugins* and press **Install** on *Soulseek (slskd)*; later releases show up on its card as **Update** (or switch on its *Update automatically*). Nothing is downloaded by itself unless that switch is on. An install upgraded from 0.30 or older keeps the slskd it had.

On the *Soulseek (slskd)* card (Settings → Plugins), turn it on and enter a username and password (or set `SOULSEEK_USERNAME` / `SOULSEEK_PASSWORD`). There is no sign-up form on Soulseek: the account is created the first time a new username and password log in. Don't use the same login in another Soulseek client at the same time, because the network signs the other one out. Forward TCP port 2234 on your router to Encore (the compose file publishes it; change it with `SOULSEEK_PORT`) so users behind firewalls can send to you. Downloads land in `/downloads/slskd` (the downloads folder setting; unfinished files in its `.incomplete` folder) and are imported from there. Encore shares your library back to the network, read-only and only the library folder, with 2 upload slots by default (`SOULSEEK_SHARE`, `SOULSEEK_UPLOAD_SLOTS`): the network runs on sharing, and many users refuse downloads to people who share nothing. The Soulseek card shows the engine's state (not installed, starting, running, crashed, waiting for the VPN); *Test connection* adds whether it is signed in to the Soulseek server.

Encore writes the engine's settings (`/config/slskd/slskd.yml`) from its own every time it starts it, restarts it when they change or when it stops, and keeps its output in `/config/slskd/encore-run.log`. The program itself lives in `/config/plugins/slskd` (one installed before 0.32 stays in `/config/tools/slskd` and is used there); *Remove* on its card deletes what the plugin installed, while its database under `/config/slskd` and your settings stay. Upgrading from an Encore with the built-in client or an external slskd container: the login, port, share and folder settings carry over; the old *Client*, *slskd URL/API key* and *listen port from a file* settings are gone, and an slskd container can be removed.

- **Soulseek through a VPN**: Encore has WireGuard built in and sends only Soulseek through it. Tidal, Qobuz, Deezer, Spotify, metadata and the web UI keep your normal connection, so streaming services don't refuse the VPN address. On the Soulseek card, under *VPN for Soulseek*, turn on *Send Soulseek through a VPN* and paste the whole WireGuard `.conf` file from your VPN provider (ProtonVPN: *Downloads → WireGuard configuration*, with NAT-PMP port forwarding on), or set `VPN_ENABLED=true` and `VPN_WIREGUARD_CONFIG`. The engine then reaches the network only through a local SOCKS5 proxy into the tunnel (the Soulseek server, peers, transfers and the distributed network; the server's address is looked up through the VPN's DNS too), and listens on 127.0.0.1 only. With *Port forwarding* on NAT-PMP, the port the VPN forwards becomes the Soulseek listen port, is relayed in from the tunnel and is followed when it changes (the listen port setting is hidden while the VPN is on). *Keep Soulseek off while the VPN is down* (on by default) stops the engine while the tunnel is not up. *Test VPN* shows the handshake, forwarded port and public IP; Diagnostics has the same line. It runs in user space: no gluetun, no `NET_ADMIN`, no tun device, and no Soulseek port to publish:

  ```yaml
  encore:
    image: ghcr.io/krifmaz/encore:latest
    environment: [PUID=1000, PGID=1000, TZ=America/Detroit, ENCORE_LIBRARY_PATH=/music]
    ports: ["8080:8080"]
  ```

**Torrents & Usenet (optional)**: add your indexers to Prowlarr, then install *Torrents & Usenet (Prowlarr)* under *Settings → Plugins*, enter the Prowlarr URL and API key on its card, and pick your torrent client (qBittorrent, Transmission, Deluge, rTorrent/ruTorrent, or a blackhole watch folder for anything else) and/or Usenet client (SABnzbd, NZBGet, or a blackhole folder). Each client is a plugin of its own with its own card; the Torrents & Usenet card offers to install the one you pick. Encore searches Prowlarr, sends the best album release to the right client, and imports it when it finishes. This replaces Lidarr.

**Optional keys** (Settings):

| Feature | Where to get it |
|---|---|
| Audio verification | Free AcoustID application key: acoustid.org/new-application |
| Qobuz/Tidal/Deezer lossless | Your own subscription. Install the *streamrip* plugin under *Settings → Plugins* and enter the Deezer ARL or Qobuz token on its card. Tidal needs a one-time sign-in: *Sign in to Tidal* on the same card. These services' terms may not allow downloading, so use at your own risk. |
| Spotify account connection | developer.spotify.com app with redirect `<Public URL>/api/connections/spotify/callback`. Since Feb 2026, development-mode apps need a Premium owner and allow 5 users. Public playlists still import without it, via the embed page. |
| Tidal import | developer.tidal.com app (redirect `<Public URL>/api/connections/tidal/callback`) |
| Last.fm scrobbling, For you, loved tracks, similar artists/tracks, genres | last.fm/api/account/create (free). Enter the API key **and shared secret**; set the app's callback URL to `<Public URL>/api/connections/lastfm/callback`. Users then sign in with Last.fm on *Connections*. |
| Discogs catalog (releases MusicBrainz lacks) | discogs.com/settings/developers → personal access token (free) |
| YouTube Music sign-in | Google Cloud, YouTube Data API v3, an OAuth client of type *TVs and Limited Input devices*. Users can paste browser request headers instead. |
| YouTube Music Premium audio (256 kbps), fewer bot checks | *Settings → Plugins → yt-dlp → YouTube sign-in* (the yt-dlp card; it was *Settings → Downloads → YouTube sign-in* before 0.32, and old links to it open the card). **Recommended – hand over a sign-in from the Encore browser extension**: press *Connect the browser extension* for a one-time token; download `Encore-youtube-extension-<version>.zip` from the releases and unzip it; Chrome/Edge/Brave: *Extensions → Developer mode → Load unpacked* → the `chrome` folder, then *Details → Allow in Incognito* (Firefox: *about:debugging → Load Temporary Add-on* → `firefox/manifest.json`, then *about:addons → Run in Private Windows → Allow*); paste your Encore address and the token in the extension's options; press **Hand over a YouTube sign-in**. A new private window opens at Google's sign-in; once YouTube Music shows you signed in, the extension sends that window's cookies to Encore and closes the window without signing out – the session now belongs to the server alone, and Encore refreshes it by itself every 4 hours (and when yt-dlp is asked to sign in), also while your computer is off, presenting itself as the browser the hand-off came from; the card says plainly when a refresh renewed nothing. If Google ever ends it, admins get a notice: do the hand-off again. **Everyday browser sync** (optional, in the extension's options) sends your own browser's cookies; Encore never refreshes those. **Pasted cookies** – a `cookies.txt` exported from music.youtube.com, the backup; Encore refreshes it every 4 hours like an open YouTube tab (switch it off there if you prefer). Encore uses the first signed-in one: hand-off, then everyday browser, then pasted. *Test cookies* checks the sign-in in use. |

Set **Settings → General → Public URL** (for example `https://music.example.com`) so the OAuth logins and invite links work.

## Troubleshooting

**Admin → Diagnostics** runs every connection, folder-permission and tool check from inside the container and shows the raw responses, with keys hidden. *Copy report* gives you text you can paste into a bug report. `curl` is also included in the image. The Soulseek engine's own output is in `/config/slskd/encore-run.log`.

| Symptom | Fix |
|---|---|
| A setting won't change in the UI | It's set by an environment variable (the field shows 🔒 and the variable name). Environment variables always win, so change it in your compose file. |
| Upgrading from an older Encore image | Switch the image to `:latest` and keep the same `/config`: the database is shared. Lidarr settings are ignored; set up Prowlarr instead if you want torrents/Usenet. |
| Soulseek: `Not signed in: login refused: INVALIDPASS` | The username already exists on Soulseek with another password. Pick a new username (the account is created at the first sign-in) or enter the right password. |
| Soulseek: *not installed* | Install the *Soulseek (slskd)* plugin under *Settings → Plugins*. |
| A picker tab says *Install the … plugin* | That source's plugin is not installed, or switched off: Settings → Plugins. A fresh install starts with none installed; an upgraded one keeps what it used. |
| Settings → Plugins says *Limits only, see the compose option* | Plugins you add run without full isolation on this host. See [The plugin sandbox and the compose option](#the-plugin-sandbox-and-the-compose-option). |
| Soulseek: *crashed* | The card shows the engine's last words; the whole output is in `/config/slskd/encore-run.log`. Encore starts it again after 5 s, then waits longer each time (up to 5 minutes). |
| Soulseek: *waiting for the VPN* | The VPN is on with *Keep Soulseek off while the VPN is down*, and the tunnel isn't up – see *Test VPN*. |
| Folder not writable by Encore | Diagnostics shows the folder owner and Encore's uid. Match them with `PUID`/`PGID` or `chown`. The Soulseek card (and Save on a folder setting) shows the exact `chown` line; folders set in the environment are created and chowned when the container starts; a download client's folder that already exists (`QBIT_DOWNLOAD_PATH`, `SAB_DOWNLOAD_PATH` and the like, and the blackhole `*_COMPLETED` folders) is left to the client, and an existing blackhole watch folder is handed to Encore only when Encore can't write to it yet. |
| streamrip: *Error loading config* / *produced no files* | Encore checks streamrip's config before every run and writes a fresh one when it doesn't load (the old file is kept as `config.toml.bak-<date>`, the Tidal sign-in, ARL and Qobuz login are carried over); *Reset streamrip config* on the streamrip card (Settings → Plugins) forces it, and Diagnostics has a *streamrip config loads* check. streamrip's whole output is kept with the request (the `log` in `GET /api/requests`). |
| streamrip: *Deezer needs an ARL cookie* | Enter the ARL on the streamrip card (Settings → Plugins); it is written into streamrip's config before every run (streamrip never gets to ask for it on the server). A song asked for again after a failure keeps its one row, with the attempt count. |
| Config ends up somewhere odd under Portainer | Use absolute host paths (`/opt/appdata/encore:/config`), not `./encore`. |

## Keeping it private

- Publish **only Encore**: port 8080 for the web UI and port 2234 for Soulseek peers (none with the VPN on). The Soulseek engine's API listens on 127.0.0.1 inside the container only, with a key only Encore knows.
- Put it behind HTTPS. For example, with Caddy:

  ```
  music.example.com {
      reverse_proxy encore:8080
  }
  ```
- **HTTP/2 for faster covers.** A library grid asks for dozens of covers at once. Over HTTP/1.1 a browser opens only six connections per server, so covers wait in line; over HTTP/2 they all share one connection. HTTP/2 is the proxy's job: Encore speaks plain HTTP/1.1 to the proxy, sends no `Connection` or other hop-by-hop headers, and its cover answers are small, cacheable files (versioned URLs are cached for good), so nothing on Encore's side holds the connection up.
  - **Caddy**: HTTP/2 (and HTTP/3) are on by default for HTTPS sites. Nothing to do.
  - **nginx**: add `http2` to the TLS listener and keep connections to Encore open:

    ```
    server {
        listen 443 ssl;
        http2 on;                      # nginx 1.25.1+; older: listen 443 ssl http2;
        server_name music.example.com;
        location / {
            proxy_pass http://encore:8080;
            proxy_http_version 1.1;
            proxy_set_header Host $host;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            proxy_set_header X-Forwarded-Proto $scheme;
            proxy_buffering off;       # live updates (remote control, request status) arrive at once
        }
    }
    ```
  - **Cloudflare Tunnel**: browsers get HTTP/2 or HTTP/3 from Cloudflare by default (Speed → Optimization → Protocol Optimization). The tunnel itself talks to `http://encore:8080`; nothing else to set.
  - Check it: in the browser's developer tools, Network tab, the Protocol column shows `h2` (or `h3`) for `/api/library/albums/…/cover` requests.
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
| `TORRENT_CLIENT` | `qbittorrent` | Where torrents go: `qbittorrent`, `transmission`, `deluge`, `rtorrent` or `blackhole` |
| `TRANSMISSION_URL` / `TRANSMISSION_USER` / `TRANSMISSION_PASSWORD` / `TRANSMISSION_LABEL` / `TRANSMISSION_DOWNLOAD_PATH` | / / / `encore` / `/downloads/torrents` | Transmission (RPC at `/transmission/rpc`) |
| `DELUGE_URL` / `DELUGE_PASSWORD` / `DELUGE_LABEL` / `DELUGE_DOWNLOAD_PATH` | / / `encore` / `/downloads/torrents` | Deluge Web UI (the label needs the Label plugin) |
| `RTORRENT_URL` / `RTORRENT_USER` / `RTORRENT_PASSWORD` / `RTORRENT_LABEL` / `RTORRENT_DOWNLOAD_PATH` | / / / `encore` / `/downloads/torrents` | rTorrent XML-RPC address, e.g. ruTorrent's `/RPC2` |
| `TORRENT_BLACKHOLE_WATCH` / `TORRENT_BLACKHOLE_COMPLETED` | `/downloads/watch` / `/downloads/torrents` | Blackhole: Encore drops `.torrent`/`.magnet` files into the watch folder and imports the same name from the finished folder |
| `NZB_CLIENT` | `sabnzbd` | Where NZBs go: `sabnzbd`, `nzbget` or `blackhole` |
| `NZBGET_URL` / `NZBGET_USER` / `NZBGET_PASSWORD` / `NZBGET_CATEGORY` / `NZBGET_DOWNLOAD_PATH` | / `nzbget` / / `encore` / `/downloads/usenet` | NZBGet (JSON-RPC) |
| `NZB_BLACKHOLE_WATCH` / `NZB_BLACKHOLE_COMPLETED` | `/downloads/watch` / `/downloads/usenet` | Usenet blackhole: `.nzb` files into the watch folder, finished downloads from the completed folder |
| `SLSKD_ENABLED` | `false` | Master switch for Soulseek |
| `SOULSEEK_USERNAME` / `SOULSEEK_PASSWORD` | | Soulseek login; a new username and password create the account at the first sign-in |
| `SOULSEEK_PORT` | 2234 | TCP port Soulseek listens on – forward it on your router |
| `SOULSEEK_SHARE` / `SOULSEEK_UPLOAD_SLOTS` | `true` / 2 | Share the library (read-only) back to Soulseek, and how many uploads at once |
| `VPN_ENABLED` | `false` | Send only Soulseek through the built-in WireGuard VPN |
| `VPN_WIREGUARD_CONFIG` | | The whole WireGuard `.conf` file from your VPN provider (secret) |
| `VPN_PORT_FORWARDING` | `natpmp` | `natpmp` (ProtonVPN) uses the forwarded port as the Soulseek listen port, `off` keeps the configured port |
| `VPN_KILL_SWITCH` | `true` | Keep Soulseek off while the VPN is not connected |
| `SLSKD_DOWNLOAD_PATH` | `/downloads/slskd` | Soulseek downloads folder |
| `YTDLP_AUTO_UPDATE` | `true` | `false` keeps the *Update plugins* task from updating the yt-dlp plugin by itself (its card's *Update* still works) |
| `SPOTIFY_CLIENT_ID` / `SPOTIFY_CLIENT_SECRET`, `TIDAL_CLIENT_ID` / `TIDAL_CLIENT_SECRET`, `LASTFM_API_KEY` / `LASTFM_SHARED_SECRET`, `DISCOGS_TOKEN` | | Account connections, imports, discovery, catalog |
| `ACOUSTID_API_KEY` | | Audio fingerprint verification |
| `STREAMRIP_ENABLED`, `STREAMRIP_DEEZER_ARL`, `STREAMRIP_QOBUZ_USER` / `STREAMRIP_QOBUZ_TOKEN` | off | Qobuz/Tidal/Deezer source |
| `ENCORE_SYNC_INTERVAL` | 15 | Minutes between like/playlist syncs |
| `ENCORE_DB_READERS` | 4 | Database read connections next to the write one, so pages load while a scan or an import writes (0 = one connection for everything, as before 0.25.3). Off by itself when `/config` is on a network file system (NFS, SMB/CIFS, FUSE); set it to turn them on there anyway. Settings → Diagnostics → Server shows what is in effect |
| `ENCORE_CHECK_UPDATES` | `true` | Look for a new Encore release on GitHub once a day (Settings → Diagnostics → Updates) |
| `ENCORE_UPDATE_CHANNEL` | `stable` | `hotfix`: Settings → Diagnostics → Updates also offers the build of every push to the main branch (see [Updates and restart](#updates-and-restart)) |
| `ENCORE_SELF_UPDATE` | | `off` hides *Update now* in Settings → Diagnostics even when the Docker socket is mounted, and keeps Encore's user out of the socket's group (see [Updates and restart](#updates-and-restart)) |
| `NAVIDROME_*`, `JELLYFIN_*`, `PLEX_*` | | Library refresh after imports |
| `DISCORD_WEBHOOK`, `NTFY_URL`, `WEBHOOK_URL` | | Notifications |

## Development

```bash
# the web app first (Rust → WebAssembly): rustup target add wasm32-unknown-unknown; cargo install trunk
cd web-rs && trunk build --release && cd ..
cd server
cargo test --workspace          # every crate's tests and tests/ (a plain `cargo test` here does the same)
ENCORE_CONFIG_DIR=../data ENCORE_STATIC_DIR=../web-rs/dist ENCORE_LIBRARY_PATH=~/Music ENCORE_PORT=8080 cargo run
```

- `server/`: the server (Rust: axum, SQLite, lofty for tags, Chromaprint via `fpcalc`). yt-dlp, streamrip, slskd, Cadenza and llama.cpp are run as programs that their plugins install into `/config/plugins/`; the Docker image has none of them, only Python 3.14 and a pinned uv for the Python ones, and bubblewrap for the plugin sandbox. It is a Cargo workspace with one `target/` folder and one `Cargo.lock`:
  - `src/main.rs` is the `encore` binary: it builds the state, starts the background work and serves the API and the web app; everything else lives in `crates/`.
  - `crates/core` (shared helpers: errors, ids, text, audio formats, versions), `crates/db` (SQLite, `migrations/`, backups), `crates/config` (settings: every setting is one entry in `settings.toml`, see [docs/contracts/settings-schema.md](docs/contracts/settings-schema.md)), `crates/http` (the HTTP client and one module per outside service), `crates/media` (ffmpeg, tags, covers, quality), `crates/library`, `crates/requests` (the request worker, download sources, Soulseek, YouTube sign-in), `crates/playback` (streams, Cadenza processing, devices, Roon), `crates/sync` (connections, playlists, scrobbling), `crates/jobs` (scheduled tasks, background loops), `crates/vpn`, `crates/plugins` (plugins: the catalog in `catalog/*.toml`, the manifest, the one installer, the sandbox, protocol 1 and the upgrade from 0.30; see [docs/plugins/](docs/plugins/)), `crates/system` (updates, restart, the old `/api/system/tools` routes) and `crates/api` (the routes, the server state and the `TestServer` helper for tests).
  - `tests/` calls the server through its router the way the apps do; `benches/perf.rs` is the speed table of [docs/perf/](docs/perf/).
  - One crate at a time while you work on it: `cargo test -p encore-library`, `cargo check -p encore-api`. Before a pull request, what CI runs: `cargo test --workspace --locked` and `cargo clippy --workspace --all-targets -- -D warnings`, plus `scripts/check-cadenza-pins.sh` from the repository root (whether the server, the desktop app and the phone pin the same Cadenza rev; CI shows it without failing until they do).
  - The speed table: `cargo bench --bench perf` (an optimised build; it fills a synthetic library of 60 000 songs and prints a Markdown table after a few minutes). The Docker image builds the whole workspace (`Dockerfile` copies `Cargo.toml`, `Cargo.lock`, `crates/`, `benches/` and `src/`).
- `web-rs/`: the web app (Rust compiled to WebAssembly, built by trunk into `web-rs/dist`; the server serves that folder at `/`). Rebuild after a change and reload.
- `clients/desktop/`: the desktop app for Windows, macOS and Linux (Rust, Iced, with Cadenza playing on this computer) – `cargo run` in that folder; `clients/desktop/README.md` has the flags, the screenshot tour and the packaging (`scripts/desktop-package.sh`).
- **Building the apps for a release on your own machine** (no Actions minutes): `scripts/release-apps.sh` builds this OS's desktop installers (the desktop app through `scripts/desktop-package.sh`) plus the Android APK (the native app in `clients/android`, with Gradle, as the apps workflow builds it), attaches them to the newest GitHub release and writes its `SHA256SUMS` again (`scripts/release-apps.sh 0.20.3` for a specific one; `--android` / `--desktop` for one of the two, `--no-upload` to only build into `out/`). Windows builds the `.exe` file from Git Bash (with NSIS), Linux the `.AppImage`/`.deb` files, a Mac the `.dmg` file; the APK builds anywhere the Android SDK + NDK, cargo-ndk, Gradle and a JDK are installed, and an APK that is uploaded must be signed with the release key (`ANDROID_KEYSTORE_PATH` or `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`). Needs the `gh` CLI signed in.
