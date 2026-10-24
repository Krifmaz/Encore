# Get the rest of the album (any album, on MusicBrainz or not)

The library album page's ⋯ → **Get the rest of the album** requests the album's songs the library doesn't have
yet. It used to stop with "MusicBrainz doesn't know this album" for albums MusicBrainz doesn't list. It never
dead-ends now: the web app, the desktop app and the Android app all follow this contract.

## 1. Where the album is found

`GET /api/discover/album-lookup?artist=&album=` asks, in order:

1. **MusicBrainz**: a release group with the same title (edition notes aside) and artist.
2. **Deezer → Apple Music → Discogs** (the tag sheet's "By…" catalogues; Discogs only with a Discogs token):
   the first one that lists the same title by a matching artist. An exact title beats an edition ("Album"
   over "Album (Deluxe)"), then the most songs win.
3. MusicBrainz's best-scoring release group, when MusicBrainz had only a loose match.

MusicBrainz being down or slow never stops step 2. The answer's `found` says what happened:

| `found` | fields | the apps |
|---|---|---|
| `musicbrainz` | `rgid`, `title`, `artist` | open the album's catalog page (Request on each missing song) |
| `elsewhere` | `rgid: null`, `ext`, `source`, `title`, `artist`, `year`, `cover`, `owned_tracks`, `tracks` (each with `in_library`, `library`, `request_status`, `request_label`) | show the track list: Request on each missing song, **Request the N missing songs**, and **Pick a download** for admins |
| `none` | `rgid: null`, `title`, `artist`, `message` | show the "type it in" dialog (§3) |

Older apps read only `rgid` and show their own note when it is null.

## 2. Requesting the missing songs

`POST /api/discover/album-rest/request`

```json
{ "artist": "Band", "album": "Lost Tapes", "year": "2019", "cover": "https://…",
  "tracks": [{ "title": "Missing One", "artist": "Band", "length": 210, "isrc": "USX222222222" }] }
```

or, typed in by hand, `"text": "1. Song (3:45)\n2. Other song"` (with `"artist_lines": true` when lines are
"Artist - Title"). Each line may start with its number ("1.", "01 -", "1)", "A1", "#3") and end with its
length ("3:45", "(3:45)", "[1:02:03]"); both are read off. At most 500 songs.

Each song not in the library and not already requested becomes a song request ("only this song",
`album_mode: song`), found by artist and title at the download sources. The weekly limit counts the album once.

→ `{ "requested": 2, "already_requested": 0, "in_library": 1, "status": "queued", "ids": [12, 13] }`

The toast: "Requested 2 songs", "Requested 3 songs (1 already requested) – waiting for approval", or
"Every missing song is already requested".

## 3. No catalogue has it

The dialog says the server's `message` ("No track list for “X” on MusicBrainz, Deezer, Apple Music or Discogs.
Type its songs in, or search the download sources for the album by name.") and offers:

- **Request the missing songs**: the typed list, through §2.
- **Search downloads for the album**: `POST /api/requests` with
  `{ "kind": "album", "artist", "title", "album", "fill_in": true }` – an album request by name alone. Soulseek,
  the torrent indexers (qBittorrent …), streamrip and YouTube Music search for it by artist and title; the songs
  of it already in the library don't end it as "Already in library" (`detail.fill_in`). Without `fill_in` an
  album request still needs a MusicBrainz id or a catalogue entry.
- **Pick a download** (admins, web and Android): the picker, searching every source for the album by name.
