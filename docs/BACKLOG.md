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
- Settings regrouped (Downloads & sources / Library, tagging & IDs / Email & notifications / …);
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
