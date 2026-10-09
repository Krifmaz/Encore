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
