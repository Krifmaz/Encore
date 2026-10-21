# Surround library (0.35)

Dolby Atmos and multichannel versions of songs, kept apart from the stereo library. Plan: docs/backlog/2026-10-surround.md.

## Storage
- `surround_tracks` (schema 30): the columns of `tracks` plus `codec`, `profile`, `channels`, `layout`, `atmos` (0/1),
  `stereo_track_id`, `stereo_album_id`. Ids start at 1 000 000 000 (`encore_library::surround::BASE_ID`), so the
  track routes tell a surround song by its number. Album ids are `s` + 15 hex characters (stereo ones are hex only).
- `surround_checks(key, checked_at, available, result)`: whether Apple Music has an Atmos version. `key` is
  `album:<stereo album id>` or `track:<stereo track id>`; a yes is kept 7 days, a no 2 days.
- `requests.class`: `stereo` (default) or `surround`. A surround request's `detail` holds
  `pick {source:"gamdl", payload, title}`, `surround_key`, `stereo_album_id`, `stereo_track_ids`.
- Files: `<surround root>/<albumartist>/<album>/<disc->NN - Title.ext`. The surround root is `surround_path`, or else
  `<library_path>/Surround`. The stereo scan skips it.

## Settings
`surround_path` (path, "" = `<library_path>/Surround`), `surround_auto_atmos` (bool, false),
`surround_hide_from_stereo` (bool, true: writes `Surround/*` to `.plexignore` and `Surround` to `.ndignore` in the
library root).

## API
- `GET /api/surround/albums?sort=added|name|artist|year&q=&limit=&offset=` → `{albums, stats}`, for everyone.
  An album: `{id, album, albumartist, year, tracks, duration, atmos, quality, stereo_album_id, class:"surround"}`.
  `stats`: `{tracks, albums, atmos, size, duration}`.
- `GET /api/library/albums/{s…}` → `{id, class:"surround", album, albumartist, year, quality, atmos,
  stereo:{album_id}|null, duration, tracks}`. A track: `{id, title, artist, album, albumartist, album_id, track,
  disc, year, duration, bitrate, format (the codec), codec, channels, layout, atmos, quality, sample_rate, bit_depth,
  isrc, stereo_track_id, stereo_album_id, class:"surround"}`, plus `path` for admins.
- A stereo album adds `surround: {have:[{album_id, tracks, quality}], request:{id,status}|null,
  available: bool|null, can_fetch: bool}`. Each of its tracks with a surround version has `surround_id`.
- `GET|POST /api/library/albums/{id}/surround` and `GET|POST /api/library/tracks/{id}/surround`. GET (everyone)
  returns `{have, request, available, can_fetch}`. POST (admins) returns `{request_id}`, or 404 when Apple Music has
  no Atmos version, or 409 when one is already requested or gamdl has no wrapper-v2 server.
- `/api/library/tracks/{id}/stream|info|lyrics|file` and `/api/library/albums/{id}/cover` take surround ids.
  `stream`: browsers get FLAC (multichannel); a client whose `x-encore-decoders` names the codec gets the original.
  `info` adds `class:"surround"`, `codec`, `channels`, `layout`, `atmos`. `POST /api/library/plays` saves no
  surround plays (`saved: 0`).
- The features list (`/api/auth/info` → `features`) has `surround`.

## Quality label
`Dolby Atmos (E-AC-3 5.1)`, `Dolby Atmos (TrueHD 7.1)`, `E-AC-3 5.1`, `FLAC 5.1`: the codec name, then the layout
(from the channel count).
