# Surround library (October 2026)

Dolby Atmos and other multichannel (5.1, 7.1) versions of songs, kept apart from the stereo library and played on a
surround setup. Written Oct 21, 2026 after 0.34.0 (zotify and gamdl).

## The owner's decisions

- **Its own section and folder**: `/music/Surround` by default (inside the music mount), a Surround tab in the apps.
  Each surround album links to its stereo album and back. Encore writes `.plexignore` and `.ndignore` in the library
  root so stereo Plex and Navidrome libraries don't show surround albums twice.
- **Fetching**: a *Get Dolby Atmos version* button on albums and songs, and a setting that fetches the Atmos version by
  itself after a stereo request completes and Apple Music has one. The source is the gamdl plugin with a wrapper-v2
  server, Atmos only (no AAC fallback). An album Apple has Atmos for only in part keeps what exists.
- **A 5.1 or Atmos file a stereo request downloads** goes to the Surround library, and the stereo request keeps
  looking for a stereo copy.
- **Access**: everyone sees and plays Surround; admins fetch Atmos versions.
- **Playback**: all three ways: Kodi, Plex or Jellyfin reading the folder; the Encore web and Android apps on a device
  wired to the receiver; Cadenza bitstreaming to the receiver.

## How it is kept apart

A table of its own, `surround_tracks` (schema 30), with the columns of `tracks` plus `codec`, `profile`, `channels`,
`layout`, `atmos`, `stereo_track_id` and `stereo_album_id`. Every query that reads `tracks` (lists, search, shuffle,
radio, playlists, likes, stats, duplicates, the quality, loudness and translation tasks, sync to the media servers,
"already in the library") sees stereo only, without a filter that one query could forget. Surround ids start at
1 000 000 000, so the track routes (`/api/library/tracks/{id}/stream|info|lyrics|file`) tell them apart by number;
surround album ids are `s` + 15 hex characters (stereo ones are hex only). Requests get a `class` (`stereo` or
`surround`). docs/contracts/surround.md has the details.

## Releases

| Release | What | Size |
|---|---|---|
| 0.35 | Schema 30; detection with ffprobe (codec, channels, layout, Atmos); the surround folder, scanner and linking; imports into Surround (stray 5.1 files of stereo requests too); `class` on requests; gamdl Atmos-only; the button and the automatic fetch; the Surround tab, album links and buttons on web, Android and desktop; the stream plan for E-AC-3, AC-3, TrueHD and DTS (converted for players that can't decode them) | ~150 h |
| 0.36 | Playback in the Encore apps: Android passes E-AC-3 Atmos through HDMI to the receiver; web and desktop play 5.1/7.1 (the Atmos bed); multichannel Opus and AAC rungs | ~90 h |
| 0.37 | Cadenza bitstreams E-AC-3, TrueHD and DTS (IEC 61937) to the receiver: the server's stream, the desktop's settings, and the engine work in the Cadenza repository | ~60 h + Cadenza |
| 0.38 | More sources: 5.1 FLAC, DTS and TrueHD in MKA from Soulseek and the indexers; multichannel-aware quality checks; the file manager's *Import into Surround* | ~80 h |

## What to test on your hardware (0.35)

Set the wrapper on the gamdl card; *Get Dolby Atmos version* on an album Apple marks Dolby Atmos: the files land
under `/music/Surround` as E-AC-3 with Atmos and the stereo copy stays; an album without Atmos says so; the automatic
setting after a normal request; the Surround tab on the website, the phone and the desktop app, and the Stereo/Surround
links; stereo Shuffle, Stats, Duplicates and Liked show nothing of Surround; `/music/Surround` added to Kodi, Plex or
Jellyfin as its own library, and Kodi bitstreaming Atmos to the receiver.
