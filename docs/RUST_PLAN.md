# Encore 2 – Rust server

The Rust server (`server/`) replaces the Python one step by step. It uses **the same database file and
settings**, and serves the same web app, so upgrading keeps every user, playlist, request and setting – and
switching back to the Python image still works (it treats the new *owner* role as admin).

## Decisions
- **No Lidarr.** Encore already does what Lidarr does (MusicBrainz, discographies, follows, tagging,
  upgrades). Torrents/usenet: Encore speaks **Torznab/Newznab** directly – point it at Prowlarr (one place
  for hundreds of sites), Jackett, or any indexer's own API – and sends the release to **qBittorrent** or
  **SABnzbd**. No custom site scrapers (they break constantly; Prowlarr maintains them).
- **Soulseek:** slskd stays the Soulseek client for now; a built-in client (no slskd container) is a later
  option.
- yt-dlp and streamrip stay as helper programs (they only exist in Python).

## Phases
| | Scope | Status |
|---|---|---|
| R0 | Server, database, settings, sign-in, users, invites, settings API. New: **owner** role, **2FA** (authenticator app), **email sign-in links**, **sign out everywhere**, **profile pictures**, readable device names | done |
| R1 | Library: scan, covers + thumbnails, streaming/transcoding, shuffle, likes, share links, storage, organize | done (lyrics lookup, artwork search, organize, duplicates come with R4) |
| R2 | Discover: MusicBrainz, sentence search, artist/album pages, catalog merge, For you, new releases (library artists only) | done (YouTube Music in the catalog merge and link probe move to R3 with yt-dlp) |
| R3 | Requests + worker + sources: slskd, yt-dlp, streamrip, HiFi API, **Torznab/Newznab → qBittorrent/SABnzbd**, picker | done (requests, worker, picker, all sources incl. Prowlarr → qBittorrent/SABnzbd; import/tagging/fingerprints/artwork pulled in from R4) |
| R4 | Tagging/import, fingerprints, AcoustID, lyrics, artwork | |
| R5 | Playlists, connections & sync, follows, auto-download, scheduler, notifications, diagnostics, stats | |
| R6 | Play on other devices (remotes), appearance settings, Docker/CI cutover, remove Python | |

## Running it
    cd server && cargo run --release
    # env: ENCORE_CONFIG_DIR (database), ENCORE_STATIC_DIR (web app), ENCORE_PORT (8080)
