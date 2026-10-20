# Client shapes: the endpoints and JSON the phone calls (0.31 / 0.33, written ahead of the server)

Written Oct 19 by the phone packages K1 (normalization, lyrics mode), K2 (artist sections, one search, car artist,
car quality) and K3 (group listen, file manager) of wf_1d2d89a5-800, made into one document by its integrator. The
server packages implement exactly these shapes (or change the phone in the same run and say so); the web and the
desktop follow the same shapes and use the words in Part 5.

Where it lives: `docs/contracts/client-shapes-0.31-0.33.md` in the repository (the phone's code comments cite that
path with these section numbers).

How to read it:
- Part 1 is the index: every endpoint the 0.31 / 0.33 phone code calls, new, changed, or existing and relied on, with
  its section and contract item (p6 = wf/p6.md, release 0.31, items 1–9; p8 = wf/p8.md, release 0.33).
- Part 2 is the shared objects (the track row's new fields, the group state, the file entry). Parts 3 and 4 are one
  section per endpoint. Part 5 is what has no endpoint (device settings, rules, the car) and the words.
- Older servers: every new route and field is optional for the phone. A 404 or a missing field hides the entry or the
  section with no error shown; normalization says "No loudness data for this song" and plays the song as it is.
  Errors of an action the person started (join, start, a command, a file operation) show the server's `detail`.
- Every error answer of a new route is `{"detail": "<plain sentence>"}`. Writes carry the usual CSRF header
  (`X-Requested-With: encore`, sent by the phone on every non-GET; harmless with a bearer token). Times are Unix
  seconds unless the name ends in `_ms`.

## Part 1 — Index

New or changed routes:

| § | method and path | what changes | contract item | phone code |
|---|---|---|---|---|
| 2.1 | every route that returns library track rows | `gain`, `sample_rate`, `bit_depth` on each row | p6 items 1, 7 | `Models.kt` `Track.from` |
| 3.1 | `GET /api/library/tracks/{id}/info` | `gain` with `source`, `lufs` | p6 item 1 | `SignalPathSheet.kt` |
| 3.2 | `GET /api/library/artists/{name}/liked` | new | p6 item 5 (and 6: the car) | `EncoreApi.artistLiked` |
| 3.3 | `GET /api/library/artists/{name}/tracks` | new | p6 item 5 | `EncoreApi.artistSongs` |
| 3.4 | `GET /api/library/search` | `limit` per section, `counts` | p6 item 9 | `EncoreApi.search` |
| 3.5 | `GET /api/library/albums`, `/artists`, `/tracks` with `?q=` | the search's matcher; no shape change | p6 item 9 | `LibraryScreen.kt` |
| 3.6 | `GET /api/group` | new | p6 item 8 | `GroupApi.list` |
| 3.7 | `GET /api/group/code/{code}` | new | p6 item 8 | `GroupApi.lookup` |
| 3.8 | `POST /api/group` | new | p6 item 8 | `GroupApi.start` |
| 3.9 | `POST /api/group/join` | new | p6 item 8 | `GroupApi.joinCode` |
| 3.10 | `POST /api/group/{id}/join` | new | p6 item 8 | `GroupApi.joinId` |
| 3.11 | `GET /api/group/current` | new | p6 item 8 | `GroupApi.current` |
| 3.12 | `POST /api/group/{id}/leave` | new | p6 item 8 | `GroupApi.leave` |
| 3.13 | `POST /api/group/{id}/end` | new | p6 item 8 | `GroupApi.end` |
| 3.14 | `PATCH /api/group/{id}` | new | p6 item 8 | `GroupApi.patch` |
| 3.15 | `POST /api/group/{id}/command` | new | p6 item 8 | `GroupApi.command` |
| 3.16 | `DELETE /api/group/{id}/members/{member}` | new | p6 item 8 | `GroupApi.removeMember` |
| 3.17 | `GET /api/group/{id}/events` (SSE) | new | p6 item 8 | `GroupListen` |
| 3.18 | `GET /api/time` | new | p6 item 8 | `GroupApi.time` |
| 3.19 | `POST /api/group/guest-join` and the guest's routes | new (the web's guest page; not the phone) | p6 item 8 | – |
| 4.1 | `GET /api/files/roots` | new | p8 | `FilesApi.roots` |
| 4.2 | `GET /api/files/list` | new | p8 | `FilesApi.list` |
| 4.3 | `POST /api/files/upload` | new | p8 | `FilesApi.upload` |
| 4.4 | `POST /api/files/mkdir` | new | p8 | `FilesApi.mkdir` |
| 4.5 | `POST /api/files/rename` | new | p8 | `FilesApi.rename` |
| 4.6 | `POST /api/files/move` | new | p8 | `FilesApi.move` |
| 4.7 | `POST /api/files/trash` | new | p8 | `FilesApi.trash` |
| 4.8 | `GET /api/files/trash` | new (the phone's Trash view) | p8 | `FilesApi.trashItems` |
| 4.9 | `POST /api/files/restore` | new | p8 | `FilesApi.restore` |
| 4.10 | `DELETE /api/files/trash/{id}` | new | p8 | `FilesApi.deleteForever` |
| 4.11 | `POST /api/files/empty-trash` | new | p8 | `FilesApi.emptyTrash` |
| 4.12 | `GET /api/files/tags` | new | p8 | `FilesApi.tags` |
| 4.13 | `PUT /api/files/tags` | new | p8 | `FilesApi.putTags` |
| 4.14 | `POST /api/files/import/preview` | new | p8 | `FilesApi.importPreview` |
| 4.15 | `POST /api/files/import/match` | new | p8 | `FilesApi.importMatch` |
| 4.16 | `POST /api/files/import/apply` | new | p8 | `FilesApi.importApply` |

Existing routes the new phone features call as they are today (only their track rows gain the §2.1 fields):

| method and path | used for | contract item |
|---|---|---|
| `GET /api/library/tracks/{id}/lyrics` → `{lyrics, synced, instrumental, source}` | lyrics mode | p6 item 4 |
| `GET /api/library/artist-tracks?name=` (track rows) | the car's "Shuffle <name>" | p6 item 6 |
| `GET /api/discover/artist/{mbid}` (`name`, `library_albums`) | the artist page's "In your library" albums; the car's artist name for an mbid after a restart | p6 items 5, 6 |
| `GET /api/discover/artist/{mbid}/extras` (`top[].library` track rows) | the car's "Popular songs" | p6 item 6 |
| `GET /api/library/albums?artist=<name>&limit=200` | the car's "Albums"; library-only artist pages | p6 items 5, 6 |
| the catalogue search Discover already calls | Discover; the library is asked beside it (§3.4) | p6 item 9 |

## Part 2 — Shared objects

### 2.1 Track rows: `gain`, `sample_rate`, `bit_depth` (p6 items 1 and 7; schema 27)

Every library track row the server writes gets three fields; the simplest rule for the server is the one function that
builds track rows. The phone reads them on every row it turns into songs: `GET /api/library/tracks` (all forms, incl.
`?ids=`), `/api/library/tracks/{id}`, `/api/library/albums/{id}` (its tracks), `/api/library/search` (songs, top song),
`/api/library/shuffle`, `/api/library/recent`, `/api/library/plays/recent`, `/api/library/artist-tracks`, §3.2 and
§3.3, `/api/playlists/{id}` (the item rows, next to their `track_id`), the `top[].library` rows of
`/api/discover/artist/{mbid}/extras`, the group's `queue` entries (§2.2), and the rows the web passes in remote
`play` / queue commands (the web passes the server's rows through unchanged).

The usual row as today: `id, title, artist, album, albumartist, album_id, track, disc, year, duration, format,
bitrate, artists[{name, mbid, in_library}], cover_v`, the `_t` / `_r` translations; and now:

```json
"gain": { "track": -7.21, "album": -6.84, "track_peak": 0.988, "album_peak": 1.0 },
"sample_rate": 96000,
"bit_depth": 24
```

| field | type | meaning |
|---|---|---|
| `gain.track` | number (a JSON number, never a string) | ReplayGain track gain in dB relative to the −18 LUFS reference (R128 tags converted: r128 / 256 + 5) |
| `gain.album` | number | album gain in dB, same reference: the tags' album gain when every track of the album has it, else −18 − the energy mean of the tracks' `loudness_lufs` (rules below) |
| `gain.track_peak` | number | linear peak of the track, 1.0 = full scale (true peak when Encore measured it) |
| `gain.album_peak` | number | max of the album's track peaks |
| `format` | string | as today: the file's lower-case extension (`"flac"`, `"m4a"`, `"mp3"`, `"dsf"` …) |
| `bitrate` | integer, kbit/s | as today |
| `sample_rate` | integer, Hz | from lofty's properties (`44100`, `96000`, `2822400` for DSD64); omitted or `0` when unknown |
| `bit_depth` | integer, bits | `16`, `24`; `1` for DSD; omitted or `0` for lossy files and when unknown. For `.m4a` a bit depth > 0 means ALAC |

- The whole `gain` object is omitted when neither `track` nor `album` is known. A single unknown field is omitted
  (null is accepted too). `{}` counts as no data. Two decimals are plenty; the phone shows one.
- Where the numbers come from (the server fills them the same way for tagged and measured songs, so a song Encore
  measured plays normalized like a tagged one):
  - Tags (`loudness_source = 'tags'`): `track` = the track gain tag (R128 converted); `track_peak` = the track peak tag
    (linear); `loudness_lufs` = −18 − `track`.
  - Measured only (`loudness_source = 'measured'`, "Measure loudness"): `loudness_lufs` = ffmpeg ebur128's integrated
    loudness; `track` = −18 − `loudness_lufs`; `track_peak` = 10^(true peak dBTP / 20) (ebur128 reports dBTP; the
    column holds the linear value).
  - A file with only album tags has `loudness_source` NULL until it is measured; then `track` comes from the
    measurement and `album` stays the tags' album gain.
  - `album` (an album = the tracks with the same `album_id`): the tags' album gain when every track of the album has
    one; else −18 − 10·log10(mean over the album's tracks of 10^(`loudness_lufs` / 10)), tagged and measured tracks
    alike (each track counts once); omitted while any track of the album has no `loudness_lufs` yet.
  - `album_peak`: the tags' album peak when the tags' album gain is used and every track has one; else the max of the
    album's known track peaks; omitted when none is known.
- When the fields appear (server 0.31): a song scanned by 0.31 has its tags' values, `sample_rate` and `bit_depth` at
  once; the "Measure loudness" scheduled task (on by default, every 24 hours, the first run 30 minutes after a start)
  first reads the tags of songs scanned before 0.31 again (their ReplayGain tags, sample rate and bit depth), then
  measures the songs without a track gain, a slice of 200 at a time while nobody uses the app, going on where it
  stopped. A song it can't measure (silence, an unreadable file) stays without `gain`. Until then the fields are
  missing, as for an older server.
- Server side: the enrichment pass adds the three fields (`encore_library::loudness::stamp`) to every object with
  `title`, `album_id` and `format` (a playlist entry by its `track_id`); a handler that builds track rows elsewhere
  (the group queue) passes them through it too. `/api/library/shuffle` POST is stamped as well.
- The phone keeps the three fields in its media items, its resume snapshot and its group queue.

### 2.2 GroupState (p6 item 8)

The answer of §3.8, §3.9, §3.10, §3.11, §3.14, §3.15, §3.16 and the data of every `state` event (§3.17).
Personalized for the calling device (`you`): every route that answers a GroupState gets the device id, in the body
(POST, PATCH) or as `?device_id=` (GET, DELETE).

**A member is one (account, device_id)**: the same account on a phone and a browser is two members, each with its
device name; leaving removes only that device; the cap counts members (devices and guests). A device is in one group at
a time: starting or joining another one takes it out of the one it was in (as a leave, §3.12). Guest tokens are for
the web page only (§3.19). Saved groups keep their memberships with the device ids (§3.17).

```json
{
  "id": "g_8f3k2q",
  "code": "K7QM4X",
  "invite_url": "https://music.example.com/#/group/K7QM4X",
  "rev": 42,
  "boot": "b3f1c9",
  "host": {"user": 3, "name": "Alex", "avatar": "/api/users/3/avatar"},
  "everyone_controls": false,
  "visibility": "anyone",
  "guests_allowed": false,
  "guests_possible": false,
  "max_members": 20,
  "server_max_members": 20,
  "queue": [
    { "...": "the track row of §2.1", "qid": "q17", "added_by": {"user": 5, "name": "Sam", "guest": false} }
  ],
  "index": 0,
  "track": { "...": "queue[index], the same row" },
  "playing": true,
  "position": 63.25,
  "anchor_ms": 1792390000123,
  "members": [
    {"member": "m3a", "user": 3, "name": "Alex", "avatar": "/api/users/3/avatar", "device_name": "Firefox on Linux",
     "joined_at": 1792389000, "host": true, "guest": false},
    {"member": "m9", "user": 0, "name": "Kim", "avatar": "", "device_name": "", "joined_at": 1792389500, "host": false, "guest": true}
  ],
  "started_at": 1792389000,
  "you": {"member": "m5b", "host": false, "can_control": false}
}
```

| field | type | meaning |
|---|---|---|
| `id` | string | the group's id (opaque; not guessable from the code) |
| `code` | string | 6 characters from an unambiguous alphabet (no 0/O, 1/I/L), upper case; the phone accepts lower case and spaces when typed |
| `invite_url` | string | `<public origin>/#/group/<code>`: the `base_url` setting, else the request's own scheme and host (as the sign-in mails build links); "" when unknown (the phone then builds `<server it signed in to>/#/group/<code>`) |
| `rev` | integer | grows by one with every change of anything in the state; the phone ignores a state with a lower `rev` than the one it has. A REST answer is the state after its command, which may already hold commands that came at the same moment. A group restored after a restart (§3.17) goes on at the saved `rev` + 1000, past any change of its last 2 seconds that was not saved |
| `boot` | string | an id of the server process (random at start), as in §3.18: the phone tells a restart from an ended group by it |
| `host` | object | `user` (integer account id), `name` (string), `avatar` (string: a server path like `/api/users/3/avatar`, or "" for none; full URLs are accepted too) |
| `everyone_controls` | bool | "Everyone can control" |
| `visibility` | string | `"anyone"` (anyone on this server can see and join) \| `"link"` (only people with the link) |
| `guests_allowed` | bool | the host's per-group switch "Guests can join with the link" |
| `guests_possible` | bool | the admin setting `group_listen_guests` (the phone shows the host's guest switch only when true) |
| `max_members` | integer | this group's cap ("Limit to N people"), 2 … `server_max_members`: the host's limit, never above the server's; a new group has no limit of its own, so it is `server_max_members` (and follows it) until the host sets one |
| `server_max_members` | integer | `group_listen_max` |
| `queue` | array | the queue in order: track rows (§2.1) with `qid` (string, unique within the group, stable while the entry exists: the same song queued twice has two) and `added_by` `{user, name, guest}` (who added the entry; the songs of `start` / `set_queue` count as added by whoever sent them) |
| `index` | integer | the current entry (0-based; any value when the queue is empty) |
| `track` | object or null | the current entry, the same as `queue[index]` (p6's `track`); null when the queue is empty. The phone reads `queue[index]` and ignores it |
| `playing` | bool | |
| `position` | number | seconds into the current song at `anchor_ms` |
| `anchor_ms` | integer | the server's clock (Unix ms) when `position` was true |
| `members` | array | `member` (string id of the membership, what §3.16 names), `user` (integer, 0 for guests), `name`, `avatar` (as `host.avatar`), `device_name` (string, "" for guests), `joined_at` (Unix seconds), `host` (bool), `guest` (bool); ordered by `joined_at` |
| `started_at` | number | Unix seconds |
| `you` | object | the calling device: `member` (its membership id, "" when not a member), `host` (bool: the caller's account hosts the group, also on its other devices), `can_control` (bool: host or `everyone_controls`; never a guest) |

**The server moves on by itself**: when `position` reaches the current song's `duration` while playing, the server
goes to the next entry (`index + 1`, `position` 0, `anchor_ms` = the moment the song ended), bumps `rev` and sends
the state; at the end of the last entry it stops (`playing: false`, `position` = its duration). Clients never send an
"ended" command; the phone predicts the roll-over itself (expected position past the duration → the next entry) so
gapless playback is not interrupted. No repeat and no shuffle mode in a group: on joining or starting, the phone sets
repeat off and ends its shuffle, and its own Shuffle / Repeat (the app, the car, the notification, the lock screen,
other devices' remote commands) answer "Shuffle and repeat are off during a group listen" while in one.

### 2.3 GroupSummary (p6 item 8)

The items of §3.6 and the answer of §3.7.

```json
{"id": "g_8f3k2q", "host": {"user": 3, "name": "Alex", "avatar": "/api/users/3/avatar"},
 "track": {"id": 11, "title": "Crazy", "artist": "Gnarls Barkley", "album_id": "al_7", "cover_v": 1792300000},
 "playing": true, "members_count": 3, "started_at": 1792389000, "visibility": "anyone", "mine": false}
```

`track` is null when the queue is empty (the phone reads `title`, `artist`, `album_id`; the server's enrichment adds
`cover_v`, `artists`); `members_count` counts members as in §2.2; `mine` is true when the account is a member on any
device. The answer of §3.7 also has `guests` (bool: a guest may join now, both switches on).

### 2.4 File entry, import album, release (p8)

**entry** (§4.2):

```json
{"name": "01 Crazy.flac", "kind": "file", "size": 31457280, "mtime": 1792300000,
 "audio": {"format": "flac", "duration": 178.4, "title": "Crazy", "artist": "Gnarls Barkley", "album": "St. Elsewhere", "track_id": 812},
 "image": false}
```

`kind`: `dir` \| `file` \| `link`; a link also has `target` (string, the link's target as text) and `target_kind`
(`dir` \| `file` \| `missing`); `size` bytes (0 for folders); `mtime` Unix seconds; `audio` only for audio files
(`track_id` only when the library has the file); `image` true for jpg/png/webp/gif.

**Links** (every link the same, wherever it points, inside the root or outside it):
- Browsing: `GET /api/files/list` follows a link to a folder only when its target is inside one of the roots;
  otherwise 403 `{"detail": "That link points outside the folders Encore may use"}`.
- The link itself can be renamed, moved and moved to trash (and restored): the link moves, its target is never
  touched. Edit tags and Import are not offered on a link.
- Nothing is written through a link: mkdir, upload, rename, move, trash, tags and import with a path that passes
  through a link (any part of it before the last, or the target folder `path` / `to_path`) answer 403
  `{"detail": "That folder is reached through a link, so Encore doesn't change anything in it"}` (a move or trash
  of several paths lists such an item in `failed` with that reason).
- The phone remembers the first link a browsing went through and, at or below it, shows "This folder is reached
  through a link, so Encore doesn't change anything in it." and offers no write.

**album** (§4.14) and **release** (§4.14, §4.15):

```json
{"folder": "Gnarls Barkley/St. Elsewhere", "artist": "Gnarls Barkley", "album": "St. Elsewhere", "files": 14,
 "release": {"id": "<mbid or provider id>", "title": "St. Elsewhere", "artist": "Gnarls Barkley", "date": "2006-05-08",
             "provider": "MusicBrainz", "track_count": 14, "score": 92},
 "confidence": 0.92,
 "rows": [{"path": "Gnarls Barkley/St. Elsewhere/02 crazy.flac", "old_title": "crazy", "new_title": "Crazy",
           "new_track": 2, "new_disc": 1, "new_path": "Gnarls Barkley/St. Elsewhere (2006)/02 Crazy.flac"}]}
```

`folder` relative to the root; `artist` / `album` from the files' tags (or the folder name); `release` null when
nothing matched well enough (`autotag_threshold`); `confidence` 0…1; `rows` as `tagger::preview` rows (paths
relative to the root of the source, `new_path` relative to the library), empty in `in_place` mode. A release has
`score`, an integer 0…100 as the tagger's match search returns it (the phone also accepts `match` 0…1).

## Part 3 — Endpoints of 0.31

### 3.1 `GET /api/library/tracks/{id}/info` — `gain` added (p6 item 1)

The existing answer gains:

```json
"gain": { "track": -7.21, "album": -6.84, "track_peak": 0.988, "album_peak": 1.0, "source": "tags", "lufs": -10.79 }
```

- The four numbers as in §2.1; `source`: `"tags"` \| `"measured"` (the track's `loudness_source`); `lufs`: the
  track's integrated loudness (`loudness_lufs`), number: the measurement, or −18 − the track gain tag for a tagged
  song (§2.1). Omitted as a whole when nothing is known.
- The phone reads `gain.source` only, for the Signal path note ("From the file's ReplayGain tags." / "Measured by
  Encore."); playback never calls `/info` for the gain, it uses the rows (§2.1).

### 3.2 `GET /api/library/artists/{name}/liked` — new (p6 item 5; the car's node, item 6)

- `{name}`: the artist name, percent-encoded as ONE path segment (a space `%20`). Query `name` (the same name): when
  present the server uses it and ignores the path segment. The apps (phone, web, desktop) always send `name=`, and put
  `_` in the path for a name with `/` ("AC/DC") or `\`, because proxies such as Apache (AllowEncodedSlashes Off) and
  some nginx setups answer 404 or decode `%2F` in a path; for a blank name ("//" matches no route); and for a name
  made only of dots ("." and ".." — also as `%2E` — are path steps the HTTP client resolves, so the request would go
  to another route).
- Query: `mbid` (optional, a MusicBrainz artist id: songs credited to that mbid count too, so a catalogue page whose
  MusicBrainz name differs from the library's spelling still finds them), `limit` (optional, default 200, max 500;
  the phone sends `limit=200`).
- Matching: the caller's liked songs that have a library match, intersected with the artist's track filter (the same
  artist matching as `GET /api/library/artist-tracks?name=`: merged spellings, aliases, collaborations, every
  credited artist of the song). Order: newest like first. Signed-in users only (their own likes).
- 200:

```json
{ "total": 3, "tracks": [ "track rows (§2.1)" ] }
```

  `total` (integer): every liked song of the artist, before `limit`. An artist with nothing liked, or unknown: 200
  with `{"total": 0, "tracks": []}`, not 404.
- Phone: a bare JSON array is also accepted (read as the whole list). 404 or any error → "Liked from this artist" is
  hidden (the artist page and the car's artist node).

### 3.3 `GET /api/library/artists/{name}/tracks` — new (p6 item 5)

- `{name}` and `mbid` as in §3.2. Query: `offset` (default 0), `limit` (default 50, max 500).
- The artist's songs in the library, sorted by album year (oldest first; no year last), then album, disc, track.
- 200:

```json
{ "total": 143, "offset": 0, "tracks": [ "track rows (§2.1)" ] }
```

  `total` (integer): every song of the artist (the phone's "Show all 143"). Unknown artist: 200 with
  `{"total": 0, "offset": 0, "tracks": []}`.
- The phone asks `offset=0&limit=10` for the page, then pages of `limit=200` (up to 5000 songs) when "Show all N" is
  tapped. 404 or error → the Songs part of "In your library" is hidden; its Albums part stays (from
  `library_albums` of `GET /api/discover/artist/{mbid}` on catalogue pages and `GET /api/library/albums?artist=` on
  library-only pages, both as today).

### 3.4 `GET /api/library/search?q=&limit=` — `counts` added (p6 item 9)

- `limit`: per section, 1–50 (the phone sends 12 from Library and Discover, 30 from the Search tab).
- The existing fields keep their shape: `top {kind, item}`, `songs` (track rows, §2.1), `albums` (album rows, singles
  included), `artists` (artist rows `{name, albums, tracks, cover_album}`). New:

```json
"counts": { "artists": 4, "albums": 17, "songs": 230 }
```

  integers, every match per section before the limit (the Library results view's "Show all N"); `albums` counts
  singles too.
- The matcher learns initials and cross-field words as p6 item 9 says; the shape does not change for that.
- Phone: without `counts` (older server) "Show all" (no number) appears when a section came back full (`limit` rows).
  Discover asks this route beside the catalogue search on every query (the newest query wins).

### 3.5 `GET /api/library/albums|artists|tracks?q=` — same matcher, no shape change (p6 item 9)

The tabs filtered by a query call the list routes as today, with the same matcher as §3.4 (initials etc.) so a tab
agrees with the results view: `GET /api/library/albums?q=&sort=&dir=&limit=&offset=`,
`GET /api/library/artists?q=&sort=&dir=`, `GET /api/library/tracks?q=&sort=&dir=&limit=&offset=`. The phone hides the
A–Z scrubber while a query is set, so `GET /api/library/index` needs no `q`.

### 3.6 `GET /api/group` — the group listens this account may see (p6 item 8)

- 200: `[GroupSummary]` (§2.3), a bare array (the phone also accepts `{"groups": [...]}`): every `"anyone"` group plus
  the groups the account is in (on any device), newest first.
- 403 when `group_listen` is off; 404 on servers before 0.31. Either one: the phone hides every group entry, with no
  error. The phone asks it every 20 s while Home's "Listening together now" row or the devices sheet is shown.

### 3.7 `GET /api/group/code/{code}` — who hosts the group behind a code (p6 item 8; added by the phone)

- 200: GroupSummary (§2.3), also for link-only groups (the code is the secret).
- 404 `{"detail": "No group listen with that code"}`.
- Rate limit, shared with §3.9, §3.10 and §3.19 (code guessing): 30 failed tries (404) per account per hour and 60 per
  IP per hour; successful answers do not count. Callers without an account also share one budget of 300 failed tries
  an hour between them, whatever address they come from (an address is only what a proxy says). Past a limit, every
  lookup and join (a right code too) answers 429 `{"detail": "Too many tries. Wait a moment and try again."}` until
  the hour has passed. The address is the connection's, or the one a trusted proxy (loopback, the private networks,
  or `ENCORE_TRUSTED_PROXIES`) appended to `X-Forwarded-For`, never what the client wrote there.
- The phone asks it before "Join <host>'s group listen?".
- Signed out (the web's guest page) it answers only while a guest may join that group (§3.19); otherwise the same 404,
  which counts as a failed try of the address.

### 3.8 `POST /api/group` — start a group listen (p6 item 8)

- Body: `{device_id, device_name, visibility, everyone_controls, tracks: [track ids], index, position, playing}`.
  `device_id` (string): the device id the phone already uses for `/api/remote/*` (the `id` of its heartbeat);
  `device_name` (string): the remote device name ("Android app (Pixel 8)"). `tracks` (≤ 1000 ids, unknown ids left
  out, may be empty), `index` into them, `position` seconds, `playing` bool: what played on the host's device
  becomes the queue.
- 200: GroupState (§2.2); the caller is host and first member. Past 10 starts per account per hour (counted when the
  start is asked for, so a start that then fails counts too): 429
  `{"detail": "You started 10 group listens in the last hour. Try again later."}`. More than 1000 ids: 400; a missing
  or too long `device_id` (1–64 characters, as for /api/remote): 400 `{"detail": "Bad device id"}`.

### 3.9 `POST /api/group/join` — join with a code (p6 item 8)

- Body: `{device_id, device_name, code}`.
- 200: GroupState. Already a member with this device → 200 with the state (idempotent).
- 404 `{"detail": "No group listen with that code"}`; 409 `{"detail": "This group listen is full"}`; 429 as in §3.7.

### 3.10 `POST /api/group/{id}/join` — join from the list (p6 item 8)

- Body: `{device_id, device_name}`.
- 200: GroupState for an `"anyone"` group. 404 "No group listen with that code" for a link-only group the caller is
  not in (nothing leaks; it counts as a failed try of §3.7); 409 full; 429 as in §3.7.

### 3.11 `GET /api/group/current?device_id=` — this device's group (p6 item 8)

- 200: GroupState of the group this device is a member of, else 204.
- The phone calls it once at start to get back into the group it was in (it remembers the id; another device's
  membership does not pull the phone in).

### 3.12 `POST /api/group/{id}/leave` (p6 item 8)

- Body: `{device_id, device_name}` (`?device_id=` works too). 204 (the phone also accepts 200 `{}`), also when the
  device was not in the group or the group is gone. A guest leaves with its token (§3.19): the token stops working and
  the `encore_guest` cookie is cleared.
- The host leaving hands the group to the host's own account on another device when it is still in the group, else to
  the longest-present account member (by `joined_at`; a guest never hosts). When
  the last account member leaves (or is dropped), the group ends: the guests' streams get `ended` with
  `reason: "ended"`.

### 3.13 `POST /api/group/{id}/end` — host only (p6 item 8)

- Body: `{device_id, device_name}`. 204; 403 `{"detail": "Only the host can end the group listen"}` for any other
  member.
- Every other member's stream gets `ended` with `reason: "ended"` (§3.17).

### 3.14 `PATCH /api/group/{id}` — the host's settings (p6 item 8)

- Body: `{device_id, device_name}` plus any of `everyone_controls` (bool), `visibility` ("anyone" \| "link"),
  `guests_allowed` (bool), `max_members` (integer, clamped to 2 … the server's max and never below the current member
  count).
- 200: GroupState; 403 `{"detail": "Only the host can change the group listen"}` for any other member; 400 for a
  `visibility` other than the two.

### 3.15 `POST /api/group/{id}/command` (p6 item 8)

- Body: `{device_id, device_name, op, …}`; every op bumps `rev`. "all" = every member; "ctl" = the host, or everyone
  when `everyone_controls`.

| op | fields | who | effect |
|---|---|---|---|
| `play` | – | ctl | playing from the current position; after the queue ran out (stopped at the end of the last entry it had then): the first entry added since, from 0, or that last song again from 0 when nothing was added |
| `pause` | – | ctl | paused at the server's computed position |
| `seek` | `position` (number, seconds) | ctl | the current song at that position |
| `next` | – | ctl | the next entry at 0; on the last entry: `playing` false, `position` = its duration, `index` unchanged |
| `prev` | – | ctl | back to 0 when past 3 s, else the previous entry at 0; on the first entry: `position` 0 |
| `jump` | `index` (integer), `qid` (string) | ctl | play that entry from 0 |
| `set_queue` | `tracks` ([ids], ≤ 1000), `index`, `position` | ctl | the queue replaced (added_by = the sender), always playing (no `playing` field is read) |
| `enqueue` | `tracks` ([ids]) | all | appended at the end |
| `play_next` | `tracks` ([ids]) | all | inserted after the current entry (after earlier play_next entries that have not played yet) |
| `remove` | `qid`, `index` | ctl | the entry removed (removing the current one plays the next) |
| `move` | `qid`, `from`, `to` | ctl | the entry moved so that its index after the move is `to` (Media3 `moveMediaItem(from, to)`: moving 2 → 5 in a list of 8 leaves it at index 5); `from` is its index before the move |

- `jump`, `remove`, `move`: `qid` wins over `index` / `from` when both are given and disagree; an unknown `qid` answers
  409 `{"detail": "That song is no longer in the queue"}`. Without `qid` the index is used; out of range → 400.

- 200: GroupState after the command. 403 `{"detail": "Only Alex controls playback"}` (the host's name) when not
  allowed; 400 for an unknown op or a missing field; 400 `{"detail": "Only songs from your library can be played in a
  group listen"}` for an `enqueue` / `play_next` whose ids are all unknown. A queue holds at most 5000 songs (400 past
  it). Songs added to an empty queue wait at the first one, not playing.
- **Not a member** (this route and §3.13, §3.14, §3.16): 404 `{"detail": "You aren't in this group listen"}`, the same
  for a group that does not exist, so a link-only group does not show through these routes either.

### 3.16 `DELETE /api/group/{id}/members/{member}?device_id=` — "Remove from the group" (p6 item 8)

- No body; `device_id` in the query (so the answer's `you` is the caller's device, as everywhere else).
- 200: GroupState. Only guests can be removed: an account member answers 403
  `{"detail": "Only guests can be removed from a group listen"}` (accounts leave by themselves). The host and the
  server's admins may (the caller is a member, by `device_id`); other members 403 `{"detail": "Only the host and admins
  can remove someone from a group listen"}`; a member that is gone 404 `{"detail": "That person isn't in the group listen
  any more"}`. The removed guest's token stops working at
  once and its stream gets `ended` with `reason: "removed"`; joining again from the guest page makes a new guest (the
  host can switch "Guests can join with the link" off). The phone offers it on guests to the host and to admins.

### 3.17 `GET /api/group/{id}/events?device_id=` — the event stream (p6 item 8)

- `Accept: text/event-stream`. On connect one `state` event with the full state, then a `state` event on every
  change; a comment line (`: keep-alive`) every 15 s. The server closes the stream after `ended`.

```
event: state
data: {GroupState (§2.2), personalized for this device_id}

event: ended
data: {"reason": "ended"}
```

- `ended` reasons: `"ended"` (the host ended it, or its last account member left or was dropped), `"removed"` (this
  member was removed), `"idle"` (12 h without a change), `"off"` (an admin turned group listens off), and for a guest
  `"expired"` (its 12 hours are over). A stream whose device left the group closes without an event.
- Every 30 s an open stream checks its caller again: an account that was disabled or deleted, or whose session was
  revoked ("Sign out everywhere"), and a guest whose token stopped working (or whose server turned guests off), is
  taken out of the group (a host hands it over, §3.12) and gets `ended` `"removed"`.
- The keep-alive comment is `: keep-alive`. A state event is the state as it is when it is sent, so a burst of changes
  may come as fewer events; their `rev`s only grow.
- 404 when the group does not exist or this device is not a member of it (also after being dropped). A device whose
  stream has been gone 2 minutes is dropped from `members`; when its stream comes back with 404 the phone joins again
  with the code silently (§3.9), so a phone that was offline for a while gets back in. When that join answers 404
  too and §3.18's `boot` differs from the one in the last state, the phone says "The group listen ended (the server
  restarted)". A group restored after a restart (`group_listen_keep` on) keeps its `id` and `code`, so the phone's
  silent join brings it back into the same group. The silent join gives the device a new `member` id; the phone
  does not announce its own old membership as a leave. When the join answers 409, 429 or 403, the phone leaves the
  group with that answer's `detail` ("This group listen is full", …); when the server can't be reached (or answers
  5xx) it tries the stream, and with it the join, again; when the code now names another group (the old one ended
  and the code was given out again), it leaves that group at once.
- What `group_listen_keep` saves (debounced 2 s, p6): the group (`id`, `code`, host, `visibility`,
  `everyone_controls`, `guests_allowed`, `max_members`, `index`, `position`, `rev`, `started_at`), the queue with each
  entry's `qid` and `added_by`, and every membership: `member` id, `user_id` or `guest_id`, `device_id`,
  `device_name`, `joined_at` and whether it is the host. No token is saved. Restored paused at the saved position with
  the same members, so `GET /api/group/current?device_id=` (§3.11) finds the phone's group after the restart and `you`
  is personalized as before; a member whose stream does not come back within 2 minutes is dropped as usual. Saving:
  2 s after the last change (at the latest 10 s after the first unsaved one), a playing group every 10 s (its position
  moves), and every group when the server is restarted from Settings. Groups idle for 12 h are not restored. With
  `group_listen_keep` off nothing is saved (what was saved is deleted) and a restart ends every group.
- 403 when `group_listen` has been turned off: the phone leaves with "Group listens are turned off on this server"
  (or the `detail`).

### 3.18 `GET /api/time` — the server's clock (p6 item 8)

- 200 `{"now_ms": 1792390000123, "boot": "b3f1c9"}`: Unix ms; `boot` as in §2.2. Not rate limited, no auth needed.
- The phone measures the offset with three round trips (§5.6).

### 3.19 `POST /api/group/guest-join` and the guest's routes — people without an account (p6 item 8; the web only)

The invite link opens the web app's guest page when nobody is signed in. Only when both switches are on: the server's
`group_listen_guests` ("Let people without an account join with the invite link") and the group's `guests_allowed`
("Guests can join with the link").

- `GET /api/group/code/{code}` signed out: the GroupSummary with `guests: true` (§3.7), or 404.
- `POST /api/group/guest-join` `{code, name}` (no sign-in; send `X-Requested-With: encore` as on every write):
  - 200: GroupState for the guest (`you.member` its membership, `can_control` false) plus `token` (string). The same
    token is set as the cookie `encore_guest` (HttpOnly, SameSite=Lax, Path=/api/group, 12 h), so the page's
    EventSource, `<audio>` and `<img>` requests carry it; `Authorization: Bearer <token>` works too.
  - 400 `{"detail": "Your name needs 2 to 32 characters"}` (spaces at the ends and doubled spaces are dropped).
  - 403 `{"detail": "Only people with an account on this server can join this group listen"}` while the server's
    switch is off, whatever the code; 404 "No group listen with that code" for an unknown code and for a group whose
    own switch is off (answered alike, so this route tells a live code from a wrong one no better than §3.7; a failed
    try of the address and of the signed-out budget, as in §3.7); 409 "This group listen is full" (guests count); 429
    "Too many tries. Wait a moment and try again." (20 guests per group and 60 per address in an hour, the address's
    60 wrong codes, or the 300 wrong codes of everyone signed out).
- What the token may use (with the cookie, writes need `X-Requested-With: encore`):
  - `GET /api/group/current`: its group's state;
  - `GET /api/group/{id}/events`: its group's event stream (§3.17; another group's answers 404);
  - `POST /api/group/{id}/leave`: 204, and the token stops working;
  - `GET /api/group/{id}/tracks/{track}/stream`: a song of the group's queue, with the library stream's query (`q`,
    `kbps`, `t`, `transcode`, `codecs`; no server processing); a song not in the queue: 401;
  - `GET /api/group/{id}/albums/{album}/cover?s=&v=`: the cover of an album of the queue; another album: 401.
  Accounts in the group may use the last two as well. Every other route answers 401 to the token: the library,
  likes, plays and scrobbles, the list, commands, settings, the leaderboard.
- The token is kept as a digest in `sessions` (kind `guest`, `user_id` 0, `device_id` the group, `device_name` the
  membership); it stops working when the guest leaves or is removed (§3.16, `ended` "removed"), when the group ends,
  when the guest is dropped (2 minutes without its stream: join again from the guest page), when an admin turns
  `group_listen_guests` off (every guest route answers 401 at once, and every guest is taken out with `ended`
  "removed"), and after 12 hours (`ended` "expired").

## Part 4 — Endpoints of 0.33 (the admin file manager, p8)

All routes admin only (403 for others) with CSRF on writes. Paths are relative to the root, `/`-separated, "" for the
root itself. A server before 0.33 answers 404 on §4.1: the phone hides "Files" in Settings → Server and in the Home
menu.

### 4.1 `GET /api/files/roots`

200 `[{id, name, path, writable, free_bytes}]` (a bare array): `id` (string slug: `library`, `staging`, `slskd`,
`torrent-<n>`, `usenet-<n>`, `trash`), `name` (shown, "Library"), `path` (absolute path on the server, shown under the
name), `writable` (bool), `free_bytes` (integer, 0 = unknown). Trash is listed last; the phone opens it with §4.8, not
§4.2.

### 4.2 `GET /api/files/list?root=&path=&sort=&dir=&offset=&limit=`

- `sort`: `name` \| `size` \| `mtime`; `dir`: `asc` \| `desc`; `offset` default 0; `limit` default and max 500.
- 200 `{"root": "library", "path": "Artist/Album", "total": 812, "offset": 0, "entries": [entry (§2.4)]}`; folders
  first in every sort.

### 4.3 `POST /api/files/upload?root=&path=&conflict=`

- `conflict`: `rename` (default) \| `replace` \| `skip`. Multipart `file` parts; the phone sends one part per
  request, streamed from the picked document. The part's filename is the name relative to `path`, with `/` for files
  of a folder upload ("Album/CD1/01 Song.flac"; folders are made as needed, each part confined); UTF-8 in the filename
  as browsers send it.
- 200 `{"uploaded": [{"name": "01 Song.flac", "path": "Album/01 Song.flac", "size": 31457280}], "renamed":
  [{"from": "01 Song.flac", "to": "01 Song (2).flac"}], "skipped": [{"name": "02.flac", "reason": "A file with that
  name is already there"}]}` (each list may be empty or missing). `uploaded[].name`, `renamed[].from` / `.to` and
  `skipped[].name` are relative to the upload's `path` (the part's filename as sent, `to` the name it got);
  `uploaded[].path` is relative to the root.
- `replace` never deletes: the file already there is moved to Trash first, exactly as §4.7 does it (its own
  `<config>/trash/<unix>-<n>/` folder with a `.origin.json` {root, path, at, by}, restorable from §4.8, the library row
  taken out like a trash), and only then is the new file renamed into its place (from the upload's own part file,
  see §4.17). When the
  move to Trash fails the part is skipped (`skipped[]`, reason "Couldn't move the old file to Trash") and the old file
  stays. A folder in the way is never replaced (skipped, "A folder with that name is already there"). The UI says it:
  "Replace it (the file that is there goes to Trash)".
- The phone sends `Content-Length` (the picked document's size) and falls back to a chunked body (no length) when the
  document turns out to have another size; the server accepts both, with the same body limit.

### 4.4 `POST /api/files/mkdir`

Body `{root, path, name}` → 200 `{"path": "<the new folder's path>"}`.

### 4.5 `POST /api/files/rename`

Body `{root, path, name}` (`name` = the new last part) → 200 `{"path": "<new path>"}`. A name that is already there →
409 `{"detail": "A file with that name is already there"}` (nothing is replaced; a change of case only on a
case-insensitive disk is a rename, not a collision).

### 4.6 `POST /api/files/move`

Body `{root, from: [paths], to_root, to_path}` → 200 `{"moved": 3, "failed": [{"path": "…", "reason": "…"}]}`. An item
whose name is already in the target folder goes in `failed` with "A file with that name is already there"; nothing
is replaced and the other items still move.

### 4.7 `POST /api/files/trash`

Body `{root, paths: [paths]}` → 200 `{"trashed": 2, "items": ["<trash id>", …], "failed": [{"path", "reason"}]}`.

### 4.8 `GET /api/files/trash`

200 `[{id, name, root, path, at, by, size, kind}]` (a bare array; the phone also accepts `{"items": [...]}`), newest
first: `id` (string, the `<unix>-<n>` folder), `name` (the item's own name), `root` + `path` (where it was, from
`.origin.json`), `at` (Unix seconds), `by` (username), `size` (bytes, folders summed; 0 = unknown), `kind` (`dir` \|
`file`).

### 4.9 `POST /api/files/restore`

Body `{items: [trash ids]}` → 200 `{"restored": 1, "failed": [{"id": "…", "reason": "…"}]}` (back where it was; a
name already there gets " (2)"; missing parent folders are made again). When its root is no longer configured the
item stays in the trash and goes in `failed` with "Its folder is no longer one Encore may use".

### 4.10 `DELETE /api/files/trash/{id}`

204. The only real delete.

### 4.11 `POST /api/files/empty-trash`

Body `{}` → 204.

### 4.12 `GET /api/files/tags?root=&path=`

One audio file → 200 `{"tags": {"title": "", "artist": "", "album": "", "albumartist": "", "tracknumber": "",
"discnumber": "", "date": "", "genre": ""}}` (strings; missing = "").

### 4.13 `PUT /api/files/tags`

Body `{root, path, tags: {…}}` with only the changed keys of the eight above (strings; "" clears the tag) → 200
`{"ok": true}`. The route writes the file itself through `media::tags::write_tags` (any file under the root, staging
included; the keys are `tagger::edit_tags`' keys), with a "clear on empty" mode added to that writer: a key sent as ""
removes that item from the file's tag (today's writer skips empty values, which would leave the old tag). When the
file is a library track (under `library_path` or an extra root), its `tracks` row is updated from the file (as a
rescan of that one file does) with one library version bump.

### 4.14 `POST /api/files/import/preview`

Body `{root, path, mode: "move" | "in_place", release_id?}` → 200 `{"albums": [album (§2.4)]}`: every album folder
under `path` (a folder with audio files; `path` itself when it has them). With `release_id` (one folder in `path`):
that folder previewed as that release.

### 4.15 `POST /api/files/import/match`

Body `{root, path (one album folder), artist, album}` → 200 `{"candidates": [release (§2.4)]}`: the match review's
search (as `GET /api/library/albums/{id}/match`), best first.

### 4.16 `POST /api/files/import/apply`

Body `{root, path, mode, albums: [{folder, release_id}]}` (`release_id` null = import with the files' own tags; in
`in_place` mode always null) → 200 `{"imported": 12, "albums": ["<album_id>", …], "message": "Imported 12 songs"}`.
In `in_place` mode nothing is moved, renamed or retagged: the absolute path of the imported folder (the request's
`path` under its root, canonicalized; not the whole root) is added to `library_extra_roots` when it is outside
`library_path` and outside every extra root already there; an extra root inside the new one is replaced by it (no
nested or duplicate entries). The files are indexed where they are, and organize never moves or renames files in an
extra root.

### 4.17 What the 0.33 server adds to these shapes (Oct 20, package S of P8)

The server implements §4.1–4.16 as written; the phone needs no change. Extra fields a client may use (older
clients ignore them):
- §4.2: `via_link` (bool) on the answer: the folder was reached through a link (browsing only follows a link whose
  target is in one of the roots; the phone already tracks this itself).
- §4.14: a matched album's `release` has both `score` (0…100) and `match` (0…1, the same number: the match's
  confidence). Without MusicBrainz, or past the first 25 album folders of one preview, `release` is null and, in
  `move` mode, `rows` say where the files' own tags put them (`new_path` from the naming template).
- §4.15: candidates have `score` (the search's) and `match` (0…1, as `GET /api/library/albums/{id}/match`
  computes it); the answer also has `provider`. Body field `provider` (default `musicbrainz`) is accepted.
- §4.16: `failed: [{folder, reason}]` for album folders that weren't imported (the others are); `message` then
  says "Imported N songs. M weren't imported. <folder>: <reason>".
- Errors (all `{"detail"}`): 400 a bad name or value ("A name can't contain / or \", "A name can't be empty",
  "A name can't contain invisible or control characters", "That name is too long", "That name isn't allowed"),
  the root itself renamed / moved / trashed; 403 a path outside its root ("That path is outside the folders
  Encore may use"), a link (§2.4's two sentences), a write into Trash ("Files go to the trash with Move to
  trash"); 404 an unknown root ("That folder isn't one Encore may use"), "That file or folder isn't there",
  "That isn't in the trash"; 409 "A file with that name is already there"; 507 a full disk (also "There isn't
  room for it in the trash (<folder>)" when Trash is on another disk than the item).
- §4.1: the torrent and Usenet folders are listed only when they are there on the disk, the client in use first
  (`torrent-1` is its folder); names "Torrents (qBittorrent)", "Finished torrents", "Torrent watch folder",
  "Usenet (SABnzbd)", "Finished Usenet downloads", "Usenet watch folder" and the like; `slskd` is
  "Soulseek downloads", `staging` "Staging".
- The extra library folders of `in_place` are the setting `library_extra_roots` (Settings → Server → Library →
  "Extra library folders", one folder per line, env `ENCORE_LIBRARY_EXTRA_ROOTS`; when the env var sets it, an
  in-place import of a folder outside them answers 409 and says to add it there).
- The scan (the scheduler's "scan" task, Settings → Library → Rescan, and the scan at start) walks `library_path`
  and every extra library folder in one scanning session (`Library::scan_all`). An extra folder that isn't there
  (a disk not mounted) is skipped, not taken as empty: its songs stay in the library, and the scan's line ends
  "(not there, so not scanned: <folder>)". The extra folders are storage roots (a storage error there reads as one).
- Songs in an extra library folder are never moved or renamed by Encore: Organize, Edit details with renaming
  (`tagger::rename_to_tags`), a release's tags from the match review (`retag_album`, through `apply_preview`)
  tag them where they are. The automatic passes leave them alone altogether: the re-tag of albums made from
  Deezer or Apple data (`retag_external`) and the bulk auto-tag. The "artwork" task keeps a picture it finds for
  such an album in Encore's own cover store (`<config>/covers/<album>.jpg`), never in the files or their folder.

The review fixes of Oct 20 (P8 fix stage) add:
- §4.3: each upload writes into a part file of its own, `.<name>.<random>.encore-upload` (hidden: the scan and the
  import pass it by), made new (never a file that is there, so a torrent client's `<name>.part` is never touched).
  Only the upload that made it renames or removes it; a part whose request went away is removed; the rows of the
  files that landed are written also when the request stops part way.
- §4.6, §4.7: a move to another disk removes from the old place only what it copied and is unchanged (a file that
  arrived or changed during the copy stays, and so does its folder); the copy's folders are flushed to the disk
  before the original goes. Renaming or moving a folder that holds an extra library folder moves its songs' rows
  with it; such a folder can't go to Trash (409 "“<name>” holds an extra library folder (<folder>): take that out of
  Settings → Library → Extra library folders first, or move it to Trash by itself").
- §4.9, §4.10, §4.11: an item is removed contents first and its note last; one that can't be removed whole stays in
  the trash, listed (409 "Not all of it could be deleted, so it stays in the trash (…)"). A note whose `stored` or
  `name` isn't one plain name is neither listed nor restored.
- A root that holds Encore's config folder can't list, read or change anything in it through that root (403
  "That is Encore's own config folder: the file manager leaves it alone").
- §4.12, §4.13: the tags are read and written through the file's handle (opened without following a link); the
  format is told from the file's bytes.
- §4.16 `move`: the album folder is listed through its handle and looked at again right before its files are
  tagged and moved, under the file manager's lock (409 "The folder changed while it was being imported: look at it
  again and import it again"). Files go where the release or their own tags put them (a file the release has no
  song for goes by its own tags); a taken name gets " (2)", nothing in the library is replaced. A song that was in
  the library keeps its row (id, likes, plays, shares). What an emptied folder still holds (covers, .nfo, .cue,
  logs, playlists, hidden files) goes to Trash as one item per folder, and the library's copies the import makes
  redundant (loose singles, lower-quality copies; `dedupe_on_import`) go to Trash with their lyrics files, their
  likes and playlist entries moving to the new song. Nothing is deleted for good.
- §4.16 `in_place`: Staging can't be kept in place (409 "Files in Staging can't be kept where they are: …"), nor a
  folder that holds the library folder, Staging or the config folder (409 "That folder holds …"). The request
  worker's clean-ups (a request's staging folder, rejected downloads) leave a folder that is or holds an extra
  library folder alone.
- Songs in an extra library folder are never deleted by an import's dedupe (the request worker's or the file
  manager's): both copies stay.
- The scan takes a folder with no songs at all, while the library has songs in it, for a disk that isn't mounted
  (an empty mount point): its songs stay, and the line ends "(looks empty, so its songs were kept: <folder>)". A
  part the walk can't read keeps the songs under it. Each such warning goes to the activity log once until the
  folder is fine again, not on every scan.

## Part 5 — No endpoint: settings, rules, the car, the words

### 5.1 Volume normalization settings and math (p6 items 2, 3)

Kept on the device (never in `PUT /api/auth/prefs`); the web's localStorage keys are the phone's SharedPreferences
keys too:

| key | type | values | default |
|---|---|---|---|
| `encore.norm.mode` | string | `"off"` \| `"track"` \| `"album"` | `"off"` |
| `encore.norm.target` | integer | −23 … −5 (LUFS, step 1) | −14 |
| `encore.norm.clip` | boolean | | true |

The same on web, desktop and phone (phone: `playback/Normalization.kt`, tested in `NormalizationTest.kt`):
- applied dB = rg_gain + (target − (−18)). Album mode: the album gain, else the track gain. Track mode: the track
  gain, else the album gain. The peak that goes with the gain used (album peak else track peak, and the other way
  round).
- Prevent clipping: applied dB ≤ −20·log10(peak); with no peak known, no boost above 0 dB (a cut is never limited).
- The result is kept within −30 … +15 dB.
- The gain is the first stage, before the preamp and EQ, and works with the effects off. It applies to whatever stream
  arrives (original, AAC rungs, FLAC transcode, a server Cadenza render: server renders stay gain-free, their cache key
  does not change). Off for a song that goes bit-perfect to a USB DAC (phone) or an exclusive bit-perfect output
  (desktop).
- Phone placement: per song when the audio sink is configured for it (`EffectsAudioProcessor.gainAtConfigure`); a
  change in the middle of a song is ramped over one buffer; the transitions player keeps the old song's gain.
- Desktop placement (0.31, `clients/desktop/src/engine/normalize.rs`; the keys live in the app's config under the same
  names): the pinned Cadenza (17673b3) has no per-track gain and decodes the stream itself, so the gain rides on the
  engine's volume – fader dB + gain dB, sent when the engine reports the song started; the volume slider keeps showing
  the fader. Off while "Play through Cadenza on this device" is off (the engine's bit-perfect mode leaves the volume
  alone). A song queued from another device (whose row the Cadenza link drops) gets its gain from
  `GET /api/library/tracks/{id}`. docs/cadenza/2026-10-track-gain.md asks Cadenza for `Track.gain_db`, which also
  ends the few milliseconds at the start of a song that still have the gain of the one before.
- Linked devices (the web and the phone, 0.31): each device keeps its own settings, and a device controlling it
  sees and changes them through the remote, with no new endpoint:
  - The snapshot a device sends with its check-ins (`POST /api/remote/heartbeat` `state`) carries
    `"norm": {"mode": "album", "target": -14, "clip": true}` – also while nothing plays – and, while normalization is
    on and a song plays, `signal.norm`: the Signal path's value for that song (`"−3.2 dB (album gain, target −14 LUFS)"`,
    `"No loudness data for this song"`).
  - The remote command `{"action": "prefs", "norm": {"mode": "track", "target": -12, "clip": true}}` (sent with
    `POST /api/remote/{id}/command` like every other command) sets them on that device; any of the three fields may be
    left out, values out of range are cleaned as above. A device that doesn't know the command ignores it.
  - The web, linked to a device that reports `norm`: Settings → Playback shows "Volume normalization · <device>" with
    that device's values, and a change goes to it with `prefs` ("Sent to <device>"). Its Signal path for that device
    shows a "Normalization" row with `signal.norm`. A device that reports no `norm` (the desktop, §5.9) is left alone:
    the block shows and changes this device's own settings.
  - Queue entries between devices (remote `play_queue` / `enqueue` / `play_next`, the snapshot's `queue`, the saved
    spot) keep `gain`, `sample_rate` and `bit_depth` next to the other fields the web's `slim()` keeps, so a song sent to
    another device plays normalized there too.

### 5.2 Lyrics mode (p6 item 4)

No new endpoint (`GET /api/library/tracks/{id}/lyrics` as today). The mode is remembered on the device under
`encore.lyricsMode` (boolean; phone: SharedPreferences "encore_ui"). Phone sizes: thumbnail 56 dp, current line 28 sp
bold, other lines 22 sp, lines already sung dimmed, the current line kept in the middle (0.3 s ahead, as before), a
tap on a synced line seeks there. "Lyrics" from a song's ⋯ menu opens the full player in lyrics mode.
The web (0.31): localStorage `encore.lyricsMode` `"true"` / `"false"`; the Lyrics tab, the Lyrics buttons and ⋯ → Lyrics
turn it on, "Back to the cover", the thumbnail, the Now playing tab and Escape turn it off; opening the player from the
bar opens lyrics mode while it is on. Sizes: thumbnail 56 px, lines 30 px (26 px at phone width), the lines already
sung dimmed.

### 5.3 The car's artist node (p6 item 6; phone only)

- The browse node `artist:<mbid>` (or `artist:<name>` without an mbid) lists: "Shuffle <name>" (all the artist's
  songs from `GET /api/library/artist-tracks?name=`), "Popular songs" (the `top[].library` rows of
  `GET /api/discover/artist/{mbid}/extras`, only when at least one is in the library), "Liked from this artist"
  (§3.2, only when not empty), then the albums (`GET /api/library/albums?artist=<name>&limit=200`) under an "Albums"
  heading. For `artist:<mbid>` after a phone restart the name comes from `GET /api/discover/artist/{mbid}`.
- The order differs from p6 item 6 (Shuffle, Popular songs, Albums, Liked) on purpose: the liked songs come before the
  long album list, so they stay within the first rows a car shows.
- The Artist button's custom command `app.encore.android.CAR_ARTIST` answers
  `{"androidx.media.utils.extras.KEY_CUSTOM_BROWSER_ACTION_RESULT_BROWSE_NODE": "artist:<mbid or name>"}` and pins the
  artist at the top of the car's Home tab. Nothing for the server, the web or the desktop.

### 5.4 The quality badge (p6 item 7)

Cadenza when the server's or the phone's Cadenza upsampled the song; Lossy for lossy formats (mp3, aac/m4a without a
bit depth, ogg, opus, wma …) and for Data saver / adaptive AAC streams; Hi-res for lossless with `bit_depth > 16` or
`sample_rate > 48000`, and DSD; Lossless otherwise. Detail line: `"FLAC 24/96"`, `"FLAC 16/44.1"`, `"ALAC 24/48"`,
`"DSF DSD64"`, `"MP3 320k"`, `"AAC 256k"`, `"Cadenza 24/192"` (the server's render), `"Cadenza 96 kHz"` (the
phone's). Without `sample_rate` / `bit_depth` the detail is the format alone and nothing is Hi-res except DSD (`dsf`,
`dff`), which is Hi-res by its format ("DSF"). Android Auto only (p6 item 7); the web and the desktop keep their own
bar badge (format and bitrate, or the stream / engine path: "FLAC 1000k", "FLAC → AAC 256k", "FLAC → Cadenza 96 kHz").

### 5.5 Group listen links (p6 item 8)

`https://<server>/#/group/<code>` shared to the app (Share → "Request in Encore") opens the app and asks "Join
<host>'s group listen?" (§3.7 first); the "Join a group listen" box takes a code or the whole link. A link to another
server is refused with "That group listen is on another server (<host>)". No Android intent filter can match links on
an arbitrary server, so links reach the phone only through Share or by pasting.

### 5.6 Group clock and sync (p6 item 8)

Offset = server − phone from three §3.18 round trips (the one with the shortest round trip; `now_ms` taken at its
middle), measured at start/join and again every 10 minutes while connected. The phone's side is its time since boot
(`SystemClock.elapsedRealtime`), not the wall clock, which Android steps when it sets the time from the network (the
web and the desktop do best with their monotonic clocks the same way: `performance.now()`, `Instant`); until measured,
or against a server without §3.18, the phone's wall clock stands in. Expected position = `position + (phone_now +
offset − anchor_ms) / 1000` while playing. Drift above 0.5 s → seek; 0.15–0.5 s → Media3 playback speed 0.97 (ahead) /
1.03 (behind) until within 0.05 s; with bit-perfect USB or Cadenza on the phone, seek only. A seek aims ahead by what
this device's seeks have been taking to start playing (learnt from where the last one landed, separately for whole
files and server-made streams, at most 3 s); after two seeks within 15 s that still land more than 0.5 s off, that
song only seeks for a drift above 3 s and the speed nudge closes the rest. A server-made stream (transcode, Data saver
/ adaptive AAC, the server's Cadenza) seeks by asking for the song again at `t=`, also when the group moves to another
song mid-way. The phone's event stream counts as dead after 45 s without a byte (three missed keep-alives) and
reconnects. After an app restart the phone is back in its group "Paused here" (it never starts playing by itself);
start and join play at once. Crossfade and the library shuffle after the queue are off in a group; gapless stays. A
pause on headphones, another app taking the sound, or a pause by a member who may not control pauses only that phone
("Paused here · Catch up"); the host's (or a controller's) own pause pauses the group (`op: pause`).

The desktop (0.31, `clients/desktop/src/player/group/`) follows the same rule with seeks only: the pinned Cadenza
(17673b3) has no rate control, so a drift of 0.15–0.5 s is left alone and above 0.5 s it seeks, aiming ahead by the
learnt seek time as above (whole files and server-made streams apart). Its side of the clock is `Instant`. The
engine's queue holds a run of the group's queue from the entry it was loaded at; songs added, removed or moved around
the one playing change it without a gap in the music. Cadenza has no crossfade, so there is nothing to switch off;
gapless stays. An engine pause the app did not ask for (another device pausing this computer through the Cadenza
link, the output going away) is "Paused here" while the group plays on; songs another device sends through the link
are not taken into the group (the engine plays the group's queue again at the next look, twice a second). Its
device id and name in every body are the ones of its Cadenza link (`desktop-<hash>`, "Encore on <host>"), and the
event stream carries the app token (`Authorization: Bearer`; the session cookie before the app has a token). A song
of the group the engine can't open (it skips it) is loaded three times, then this computer stays paused until the
group moves on.

The web (0.31, package W-group; `web-rs/src/group/`, `web-rs/src/player/group.rs`) follows the same rule: its side of
the clock is `performance.now()`; 0.15–0.5 s is closed with the `<audio>` element's `playbackRate` 0.97 / 1.03 only
where the browser keeps the pitch (`preservesPitch`, or its `moz`/`webkit` form, set to true and still true), else it
seeks only; a seek aims ahead by the learnt seek time (whole files and server-made streams apart, the phone's
`SeekLag`). Its device id and name are the remote's (`encore.device`, "Chrome on Linux" or the name given in Your
devices). It remembers its group as `encore.group.id` (localStorage): after a reload it is back in it "Paused here"
(browsers want a tap before they play sound anyway). Starting or joining unlinks this browser from a device it was a
remote for, and linking is refused while in a group ("Leave the group listen to control another device"); the other
devices' cards in Your devices lose their controls meanwhile. Commands another device sends to this browser through
the remote (play, pause, next, a queue) go to the group like its own buttons. An invite link opened while signed out
is the guest page when a guest may join (§3.19), else the sign-in page with "Only people with an account on this
server can join this group listen"; the link is asked about again once signed in (sessionStorage
`encore.group.join`). The guest page plays on its own `<audio>` through §3.19's stream and cover routes with the
cookie, never reads the `token` field, and shows no library, no likes and no controls but its own pause.

### 5.7 Words (identical on web, desktop and phone)

The strings check (`scratchpad/int-strings/strings_check_p6.py`) looks the phone's new strings up in web-rs, the
strings reference and this section; the ones only the phone can have are in §5.8.

**Normalization** (Settings → Playback → Sound (on this device), first in the section, before "Use sound effects"):
- "Volume normalization": segmented Off / Track / Album. Help under it by mode: Off "Every song plays at its own
  level." · Track "Every song plays at about the same loudness." · Album "Every album plays at about the same
  loudness; the songs of an album keep their differences."
- "Target level": slider −23 … −5, step 1, the value shown as "−14 LUFS" (U+2212 minus).
- "Prevent clipping": checkbox.
- Help: "Uses the ReplayGain tags in your files, or Encore's own measurement when a file has none. Higher targets are
  louder; −14 LUFS is about what streaming services play at. Prevent clipping never turns a song up further than its
  loudest peak allows. Only on this device."
- Signal path, a row "Normalization" (before "Effects on this device"), shown only while normalization is on. Value:
  "−3.2 dB (album gain, target −14 LUFS)" / "+2.0 dB (track gain, target −14 LUFS)" (one decimal, U+2212 minus, "+"
  for a boost, "0.0 dB" for none); "No loudness data for this song"; "Off for bit-perfect output". Note (any that
  apply, joined by a space): "Lowered to keep the peaks from clipping (the song asked for +7.0 dB)." · "No album gain
  for this song, so its track gain is used." · "No track gain for this song, so its album gain is used." · "It plays
  at its own level until Encore has measured it." · "The DAC gets the samples untouched." · "From the file's
  ReplayGain tags." · "Measured by Encore." Verdict: with a gain other than 0 the song is not "Pure playback"; with
  normalization and no effects: "Encore sent the file untouched; this device changes its volume for normalization
  (Settings → Playback → Sound)."

**Lyrics mode**: "Back to the cover" (a tap on the thumbnail and Back do the same), "Looking for lyrics…", "No lyrics
found for this song.", "Instrumental".

**Artist page**: "Liked from this artist", "Play all", "In your library", "Songs", "Albums", "Show all N", "Popular
songs" (the car).

**One search**: the Library search box "Search artists, albums, songs in your library…" (it replaces "Filter
library…"); the results view "Artists", "Albums", "Songs", "Show all N"; nothing anywhere: "Nothing in your library
matches “q”" and "Search everywhere"; a tab filtered by a query with no rows: "No albums match “q”", "No artists
match “q”", "No songs match “q”" (without a query they stay "No albums found" etc.); Discover: "In your library",
"Nothing in the catalogue matches “q”", and when the catalogue fails while the library answered "Couldn't search the
catalogue: <reason>".

**Group listen**: "Start a group listen", "Join a group listen", "Join with a code", "Join <host>'s group listen?",
"Join this group listen?" (the host is not known), "Group listen", "Group · 3", "Your group listen", "<host>'s group
listen", "Copy invite link", "Invite link copied", "Couldn't copy the link", "Everyone can control", "Anyone in the
group can play, pause, skip and change the queue.", "Everyone can control playback.", "Who can join", "Anyone on this
server can see and join", "Only people with the link", "Guests can join with the link", "People without an account
join on the web with the invite link. They listen; they can't add songs or control playback.", "Limit to N people",
"People (N)", "End for everyone", "End the group listen?", "The music stops for everyone in it.", "Leave", "Remove
from the group", "Couldn't remove <name>", "Host", "Guest", "You", "added by Sam", "Only Alex controls playback", "Sam
joined", "Sam left", "You host the group listen now", "Alex hosts the group listen now", "Paused here", "Catch up",
"Listening together now", "Alex · 3 listening", "Join", "Joining…", "Open", "Group listens", "You host it", "You're in
it", "You're in it on another device", "No group listens on this server right now.", "Leave the group listen to
control another device", "Shuffle and repeat are off during a group listen", "Only songs from your library can be
played in a group listen", "Everyone in the group can add songs with Add to queue.", "Nothing queued. Anyone in the
group can add songs with Add to queue.".
- Start: "Everyone who joins hears the same songs at the same time, each on their own device. What plays here now
  becomes the group's queue." The sheet: "You host this group listen. Everyone hears the same songs at the same time,
  each on their own device." / "Alex hosts this group listen. Everyone hears the same songs at the same time, each on
their own device."
- Join: "Type the code the host gave you, or paste the invite link.", "Code" (the box's hint, and the label over the
  code in the sheet), "Playing now: <title> – <artist>." ("Playing now: <title>." without an artist), "What plays here
  stops, and you hear the group's songs with everyone else.", "You joined Alex's group listen", "You joined your group
  listen".
- Endings: "You left the group listen", "You ended the group listen", "Alex ended the group listen", "You were removed
  from the group listen", "The group listen ended", "Group listens are turned off on this server", "The group listen
  ended (the server restarted)".
- Errors: "That isn't a group listen code or link", "That group listen is on another server (<host>)", "No group
  listen with that code", "This group listen is full", "Couldn't start the group listen", "Couldn't join", "Couldn't
  end the group listen", "Couldn't change the group listen"; from the server: "Too many tries. Wait a moment and try
  again.", "You started 10 group listens in the last hour. Try again later.", "That song is no longer in the queue",
  "Only guests can be removed from a group listen", "You aren't in this group listen", "Only the host can end the
  group listen", "Only the host can change the group listen", "Only the host and admins can remove someone from a group
  listen", "That person isn't in the group listen any more", "Only people with an account on this server can join this
  group listen", "Your name needs 2 to 32 characters", "That song isn't in the group listen".
- Start dialog buttons: "Start" ("Starting…" while it waits), "Cancel". The host's settings: "At most N on this
  server." under "Limit to N people". A member who may not control, in the sheet and in Up next: "Only Alex controls
  playback. You can add songs with Add to queue."
- The car's now-playing subtitle starts with "Group listen · ".

**Files** (Settings → Server → Files on the phone; System → Files on the web): "Files", "Server", "Browse, upload,
move, rename and import in the library and download folders" (the entry's line), "The folders Encore may change.
Deleted files go to Trash first.", "Trash", "Read-only", "<size> free", the sort button "Sort: Name ↑" / "Sort: Name
↓" (the chosen sort, ↑ ascending, ↓ descending), "Name", "Size", "Date modified" with the order "A–Z" / "Z–A",
"smallest first" / "largest first", "oldest first" / "newest first"; "Up one folder", "Show more (N)", "This folder
is empty", "Upload files here, or make a new folder.", "That folder has no files", "No folders here", "Couldn't load
the folders", "Couldn't open the folder", "Couldn't read the folder"; a link's line "Link to <target>" ("Link to
somewhere else" without one); in a folder reached through a link: "This folder is reached through
a link, so Encore doesn't change anything in it."
- From the server (§2.4, Part 4): "That link points outside the folders Encore may use", "That folder is reached
  through a link, so Encore doesn't change anything in it", "A file with that name is already there", "Its folder is
  no longer one Encore may use".
- Actions: "New folder", "Make folder", "Folder made", "A name can't contain / or \", "Upload", "Upload files", "Upload
  a folder", "Rename", "Move", "Move “<name>”", "Move here", "Moved", "Moved N items", "Move to trash", "Move
  “<name>” to trash?", "Move N items to trash?", "You can restore them from Trash. Songs in the library are taken out
  of it until they are restored.", "Moved to trash", "Moved N items to trash", "N weren't done. <name>: <reason>".
- Trash: "Restore", "Restored", "Delete forever", "Delete “<name>” forever?", "This can't be undone.", "Deleted",
  "Empty trash", "Empty the trash?", "Everything in it is deleted forever. This can't be undone.", "The trash is
  empty", "Files you move to trash wait here until you delete them forever.", "N in the trash", "Couldn't open the
  trash".
- Tags: "Edit tags", "Saving…", "Tags saved", "Couldn't read the tags", "Couldn't write the tags", "That song isn't in
  the library".
- Upload: "Upload “<name>”?", "Upload N files?", "When a file with the same name is there", "Keep both (the new one
  gets “(2)”)", "Replace it (the file that is there goes to Trash)", "Skip the new one", "Uploading 3 of 12", "Stop",
  "Uploaded 1 file", "Uploaded N files", "Uploaded N of M. <what went wrong>", "Upload stopped after N of M".
- Import: "Import…", "Import", "Import 1 album" / "Import N albums", "Importing…", "Keep the files where they are",
  "The songs join the library where and as they are: nothing is moved, renamed or retagged.", "Each album is tagged
  as the release it matches and moved into the library.", "Reading the folder…", "Looking for matches…", "No songs in
  this folder", "Other matches", "Show the files", "Hide the files", "(no match)", "· 92% sure", "No match found: it
  is imported with its own tags.", "Searching…", "The search didn't work", "Couldn't use that release", "The import
  didn't work", "Imported 1 song" / "Imported N songs" (when the server's `message` is missing).

### 5.8 Phone-only words (nothing like them on the web or the desktop)

| words | why only the phone |
|---|---|
| "Normalization is off while this phone sends songs bit-perfect to the USB DAC (Cadenza engine below)." | bit-perfect USB output exists only on the phone |
| "Normalization is off while this computer plays bit-perfect (Cadenza on this device, above)." | the desktop's own line for the same case: its plain mode ("Play through Cadenza on this device" off) leaves the volume alone (desktop only) |
| "<name> is at the top of Home" | Android Auto: its now-playing screen can't open a browse node, so the Artist button pins the artist on Home and says so |
| "Encore on your phone didn't answer. Try again in a moment." · "Nothing here yet" | the Encore Lyrics car app's artist list, read from the phone's media session |
| "Share invite link" · "Group listen on Encore" · "Listen with me on Encore (code K7QM4X):" + the link | Android's share sheet (its title, the subject and the text it hands to another app); the web has "Copy invite link" |
| "This server has no file manager yet (Encore 0.33 or newer has one)." | the phone (and the desktop, which says the same) can be newer than its server; the web always comes from its own server |
| "The file can't be read any more" | a document picked in Android's file picker went away (or its permission did) before the upload read it |
| "Wait for the upload that is running" | the phone runs one upload at a time in the background, after the page is left |
| "The file changed while it was uploading" | a picked document whose size differs from what Android's file picker said, twice (the second try is sent without a length) |
| "Uploads" (the notification channel) · "Uploading 3 of 12" + the file's name as the upload's notification | the phone keeps uploading in the background with a foreground service, which Android shows as a notification |

### 5.9 Open on the phone (parity items for later packages)

- The four group listen admin settings (`group_listen` "Allow group listens", `group_listen_guests` "Let people without
  an account join with the invite link", `group_listen_max` "Most people in one group listen" (2–100, unit "people"),
  `group_listen_keep` "Keep group listens through a server restart"; Server → General, card `grouplisten` "Group
  listens", after the General card) are rendered from the settings schema. Done on the phone since 0.30's
  schema-driven Server pages (P5, `features/settings/ServerSettings.kt`): the card shows in Settings → Server →
  General with no phone code of its own. The desktop's admin form is still a static copy of the table
  (`clients/desktop/src/pages/settings/fields.rs`); the card is in it since 0.31 with the same keys and words.
- Normalization for linked devices through the remote `prefs` command (p6 item 2, §5.1): done on the web and on the
  phone in 0.31. The phone (`remote/ServiceHost.kt`) applies `{action: "prefs", norm: {mode, target, clip}}` to its
  `encore.norm.*` settings (any field may be left out, values cleaned as in §5.1), reports `norm` in its snapshot (also
  while idle) and `signal.norm` while a song plays normalized, and its `RemoteDevice.slim()` keeps `gain`,
  `sample_rate` and `bit_depth` (wired in `playback/EncorePlaybackService.kt`). Only the desktop is left: its snapshot
  and its commands are Cadenza's link (`cadenza::encore`), so it waits for the hooks
  docs/cadenza/2026-10-track-gain.md asks for. Until then a web remote linked to the desktop shows and changes its own
  settings.

### 5.10 Sign-ins started on the phone (p6 item 10; no change to what the phone calls)

- The phone keeps calling `POST /api/connections/{spotify|tidal|lastfm}` `{}` and `POST /api/contribute/mb/connect`
  for `{url}`, opens it in a browser tab and polls `GET /api/connections` (MusicBrainz: `GET /api/contribute`) as
  before. New on the server: the start call keeps a single-use state (32 random bytes, the user, the service, the
  device that started it — in the Sessions list's words for the session's user agent, "Encore app on Android",
  never the device name the app sent, which is free text — valid 10 minutes, in memory) and the callback links the
  account to that user even when the tab's browser is not signed in to Encore.
- When the tab's browser is not signed in as that user (signed in to nobody, the usual case on the phone, or as
  someone else), the callback answers a small page of its own instead of the web app: "Link the Spotify account
  “<name>” to the Encore user “<user>”?" (MusicBrainz: "Link the MusicBrainz account “<name>” to this Encore
  server?"), "The Encore user “<user>” started this sign-in on “<device>”.", Link / Cancel (a form posting to
  `POST /api/sign-in/confirm`). Link ends on "The Spotify account “<name>” is now linked to the Encore user
  “<user>”. You can close this page and go back to Encore." The phone's poll then sees the connection.
- What the page protects: for Spotify, Tidal and MusicBrainz it stops account-linking CSRF (those services send the
  browser back only to the registered address, so the victim's browser gets the page). Last.fm's web sign-in takes
  its callback address (`cb`, which carries the state) from the link as given, and its token can be exchanged by
  this server for anyone who brings it: an attacker can point `cb` at their own site and bring the victim's token
  to the callback in their own signed-in browser, where no page is shown. No server change can close that, so for
  Last.fm only the approval page on last.fm protects the person; the Last.fm connection dialog (web, desktop,
  phone) says "Only approve Encore on Last.fm when you started the sign-in yourself."
- `encore_session` never leaves the server's host: `EncoreHttp.kt` `SessionOffHostInterceptor` (a network
  interceptor, so it sees every redirect hop) drops Encore's own cookie and bearer token from a request for another
  scheme, host or port; other cookies and headers stay. Only the server's own 401 signs the app out.
