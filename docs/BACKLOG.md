# Encore backlog (reported 2026-10-09)

## Remove
- Localized names feature (all of it).
- 30s song previews.
- Built-in (embedded) services: qBittorrent and slskd. Replace the whole "container in a container"
  approach (ENCORE_EMBEDDED, aio image) with native Encore integrations.
- Lidarr (decision pending – see "Sources" below).

## Android app
- Opens to a black screen until it loads (screenshot 1).
- Changing server/port/profile needs "clear app data" – add a proper built-in server screen
  (saved servers, switch, edit) instead of the prompt dialog.
- Hamburger menu (top left) sits under the status bar and can't be tapped – respect safe-area insets.

## UI
- Album art doesn't scale properly anywhere (home cards, now playing, lists).
- Uploaded artwork doesn't show until reload – bust the cache immediately.
- Settings: regroup into coherent tabs (all downloaders/sources in one, all tagging/ID/verification
  in another, etc.); no horizontal-scrolling tab bar; tidy the label/help/"env var" text layout.
- Way more user-level controls (per-user preferences).

## Playback
- Shuffle = a button that shuffles the queue/library; never repeat a song within one listening session.
- Share a song: public link that plays without login, locked to that one song unless signed in.

## Downloads
- Downloads should basically never fail: when sources fail, always fall back to YouTube Music.

## Accounts
- Password reset by email (SMTP settings for the user's mail service).

## Library
- Automatic folder/file organization from tags: Artist/Album/NN Title, with fallbacks
  (Deezer/Apple/Discogs/AcoustID/YouTube Music metadata) when MusicBrainz doesn't know a release.

## Sources (decision)
- Encore already does MusicBrainz lookup, discographies and follow/monitor, so Lidarr is redundant.
- Prowlarr: query its Torznab/Newznab API directly and send to the user's own qBittorrent/SABnzbd –
  only worth keeping if torrents/usenet are used. Writing our own indexer scrapers instead of Prowlarr
  is not worth it (Prowlarr maintains hundreds of indexer definitions).

## Added 2026-10-10
- Artist discographies: tapping an album doesn't open its track list.
- Artwork fetching is slow or never happens.
- Search: understand plain sentences / loose phrasing ("that song by X about Y").
- Fullscreen player: the close X and Now playing / Lyrics / Up next tabs sit under the
  status bar on Android (same safe-area fix as the hamburger menu).
- Library → Songs on mobile: the table is wider than the screen, so the ⋯ menu only shows after
  swiping sideways. Fit rows to the screen width, with ⋯ always visible.
- Mini player artwork is cropped or zoomed (shows a sliver of the cover instead of the whole square).

## Done 2026-10-10
- Album art fills its frame everywhere (thumbs, mini player, fullscreen, tiles).
- Songs list fits phone width; ⋯ always visible.
- Status bar overlap: Android app pads for system bars; web uses safe-area insets.
- Duplicates: re-imports no longer create "(2)/(3)" copies – the better file is kept.
- "Not in MusicBrainz" albums open their track list (or the library album if owned).
- New artwork shows immediately; failed art lookups retry after 30 min instead of never.
- Android: loading spinner instead of black screen; asks for the server again when the saved one
  answers with an error; "Change server" link on the sign-in page.
- Shuffle button on Library; shuffles never repeat a song within a listening session.
- YouTube Music is always the last download source (setting, on by default).
- Settings regrouped (General / Apps & sources with one drop-down per app / Email & notifications / Auto-download / Scheduled tasks);
  dropdown page picker on phones, no sideways tab bar.
- Password reset by email: SMTP settings + test, "Forgot password?" on sign-in, email on Account page.
- Removed: localized names, 30-second previews.
- Share a song: ⋯ → Share link; the public page plays only that song (no login, no way into the library).
- Search understands sentences ("the song X by Y", "radiohead album …", "that song that goes …" via lyrics).
- Library → Organize files: Artist/Album (Year)/NN Title from tags; missing tags filled from the file
  name, AcoustID and Deezer first.
- Removed the built-in services (aio image: slskd, Lidarr, Prowlarr, qBittorrent, SABnzbd).
  Lidarr/slskd still work as your own external containers.
- Android: built-in server screen with saved servers (add / open / remove), no clearing app data.
- Settings hub (Account, Connections, Users & permissions, Invites, server pages, Auto-download, Tasks,
  Diagnostics, API & security, About/version); gear next to your name; all keys on one page, hidden.
- Activity → Downloader page; Following → For you page.
- Permissions per user + all on/off: auto-approve, auto-approve playlists, pick exact versions, fix their
  own downloads (incl. removing a wrong song they requested), see storage.
- Artwork: cover path remembered, small cached thumbnails – no network for songs you already have.
- Upgrades compare the audio first; a different recording is kept as its own song.
- Fix this song (edit / identify by audio, file moved, never merged); remove wrongly added artists.
- Android offline: Save song/album for offline; when the server is down the app opens the offline
  player with a "Not connected" banner; web app shows a connection banner too.

## Later: more Rust (noted 2026-10-11, not scheduled)
Goal: get the codebase as close to 100% Rust as is practical (~97–99%; CSS, HTML, Dockerfile and
scripts always stay).
1. ~~**Desktop app → Tauri**~~ – done 2026-10-11 (`clients/app`, Rust; Electron removed). Media keys, OS media
   controls with cover, tray, saved servers, dead-server picker.
2. ~~**Android app → Tauri 2 mobile**~~ – done 2026-10-11 (same `clients/app`; a small Kotlin plugin for the
   media notification, share intents, downloads and the offline cookie). CI signs with a generated debug key
   until a release keystore is configured.
3. **Web interface → Rust/WebAssembly** – written 2026-10-11 (`web-rs`, plain web-sys, no framework): every
   page, the player and remote ported 1:1 and served at `/next/` beside the JS UI (link in Settings → About).
   Left: use it for a while, fix what differs from the JS UI, then make it the default and drop `web/js`.
