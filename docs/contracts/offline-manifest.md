# Offline manifest

The one record every Encore client writes for a song saved on the device, where each client keeps it,
how an older index is upgraded, and what the server adds for clients that work offline. Written for
0.28.0 (the website works offline); the desktop app (Iced, `clients/desktop`) follows it from 0.28.0 and
the phone app from 0.30.

## The index

The index is one JSON document: an **array of records**, oldest first, with no wrapper object and no
version number (the shape grows by adding optional fields). A record is keyed by
`(source_server, id)`: the same song id from two servers is two records, and saving a song again
replaces its record instead of adding a second one. A client that keeps one server's songs only (the
web app) keys by `id` alone.

Readers are lenient: an object holding the array under `songs`, `tracks` or `records` reads too, numbers
written as strings read as numbers, a record that appears twice keeps its last entry, and fields a reader
does not know are ignored. Writers always write the plain array; a writer may drop fields it does not
know (the web store does), so a new field must never be the only place something important is kept.

## The record

```json
{
  "id": 1234,
  "title": "So What",
  "artist": "Miles Davis",
  "album": "Kind of Blue",
  "album_id": "a1b2c3",
  "ext": "flac",
  "bytes": 61234567,
  "quality": "FLAC 24/96",
  "cover": true,
  "lyrics": false,
  "saved_at": 1792310400,
  "source_server": "https://music.example.com",
  "duration": 562.4,
  "track": 1,
  "disc": 1,
  "format": "flac",
  "artists": ["Miles Davis"]
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The server's track id (`tracks.id`). Older indexes wrote it as a string of digits: readers accept both, writers write a number. |
| `title` | string | yes | The song title. |
| `artist` | string | yes, may be `""` | The track artist; the album artist when the track has none. |
| `album` | string | yes, may be `""` | The album title. |
| `album_id` | string | no (`""`) | The server's album id (text on the server); `""` when the song has none. A number is read as its decimal text, null as `""`. |
| `ext` | string | yes | The saved audio file's extension, lower case, no dot: `flac`, `mp3`, `m4a`, `aac`, `ogg`, `opus`, `wav`, `aiff`, `dsf`, `dff`. |
| `bytes` | integer | yes | The saved audio's size in bytes (not the cover, not the lyrics). |
| `quality` | string | yes, may be `""` | The label the player shows for the saved copy (see "The quality label"). Upgraded records without one take the upper-case `ext`; `""` only when neither was known. |
| `cover` | bool | no (false) | The album cover was saved with the song. |
| `lyrics` | bool | no (false) | The lyrics were saved with the song. |
| `saved_at` | number | yes | When the song was saved, in Unix seconds. Writers write a whole number; readers accept a fraction (desktop 0.27 wrote one). |
| `source_server` | string | yes | The server's base URL the song came from, scheme, host and port, no trailing slash (`https://music.example.com`, `http://192.168.1.10:8080`). The web app writes `location.origin`. |
| `duration` | number | no (0) | Seconds; 0 when unknown (null reads as 0). |
| `track` | integer | no (0) | The track number on its disc; 0 when unknown. |
| `disc` | integer | no (0) | The disc number; 0 when unknown. |
| `format` | string | no (`ext`) | The codec of the saved bytes, lower case: `flac`, `alac`, `mp3`, `aac`, `opus`, `vorbis`, `wav`, `aiff`, `dsf`, `dff`. For a converted copy this is the codec it was converted to, not the codec of the file on the server. |
| `artists` | array | no (`[]`) | The song's `artists` list as the server sent it (names, or objects with a `name`), when it sent one; an empty list means "use `artist`". |

"Required" means every writer writes the field and a record without `id` is skipped on read. Every
other missing field takes the default in brackets.

### Fields only file-based clients write

The desktop app and the phone keep the songs as files and add two fields the web app does not write:

| Field | Type | Meaning |
|---|---|---|
| `file` | string | The audio file's name in the offline folder (desktop), or its path relative to `Music/Encore/` (phone; on Android 10 and later the index lives in app storage, so this is not relative to the index there). |
| `art` | string | The cover file's name (desktop) or its path relative to the folder that holds the index (phone), `""` when there is none. `cover` is true exactly when this names a file that exists. |

These are paths inside the client's own folder, never URLs and never absolute paths, so the folder can
move without the index breaking.

### The quality label

The same words the player shows for a song, so the Downloads list and the signal path agree:

- lossless with a known bit depth and sample rate: `<Codec> <bits>/<kHz>` – `FLAC 16/44.1`, `FLAC 24/96`,
  `ALAC 24/192`. The rate is in kHz with one decimal at most and no trailing `.0` (44100 → `44.1`,
  96000 → `96`); with only the rate `FLAC 44.1 kHz`, with only the depth `FLAC 24-bit`, with neither `FLAC`;
- lossy: `<Codec> <kbps>k` – `MP3 320k`, `AAC 256k`, `Opus 160k`; the codec alone when the bitrate is
  unknown;
- a converted copy (see below): the label of what was saved, then ` (converted)` – `AAC 256k (converted)`,
  `Opus 256k (converted)`, `FLAC (converted)`.

The codec name is the upper-case `format`, except `Opus`, `Vorbis` and `WavPack`, which the player writes
that way. Bit depth, sample rate and bitrate come from `GET /api/library/tracks/{id}/info` (`bit_depth`,
`sample_rate`, `lossless`, `bitrate`), else from the library row (`bitrate`, kbps); for a lossy conversion
the bitrate asked for.

## Converted copies

A copy is **converted** when the server's answer was not the original file: the client asked for a
conversion (`?q=aac`, `?q=opus`, `?transcode=1`), or the server converted on its own (`x-encore-stream`
anything but `original`: `flac`, `flac-fallback`). A converted copy is marked by `quality` ending in
`(converted)`, and only by that: clients test `quality.ends_with("(converted)")` (after trimming trailing
spaces). There is no separate field.

The web app saves the original when `/info` says `offline_ok` (or `stream.kind == "original"`) and the
browser plays the codec; otherwise it asks for a lossy copy at 256 kbit/s, AAC where the browser decodes
AAC, else Opus (`?q=aac&kbps=256`, `?q=opus&kbps=256`): `AAC 256k (converted)`, `ext` `aac` or `opus`.
If the server still answers with something other than `original` for a plain stream, the copy is
`FLAC (converted)`, `ext` `flac`.

While online, a client offers **Download again in original quality** on a converted copy. It asks
`GET /api/library/tracks/{id}/info` first: when that answers `offline_ok: true` (and the client decodes
the codec), it downloads the original and replaces the record and the audio under the same key; otherwise
nothing is downloaded, the converted copy and its record stay, and the client says so.

What the server sends for each request (0.27 codec negotiation, unchanged in 0.28):

| Request | Body | `x-encore-stream` |
|---|---|---|
| `/api/library/tracks/{id}/stream` (browser, no decoders header) | the original when the browser plays the codec, else FLAC made on the fly | `original` or `flac` |
| the same with `X-Encore-Decoders` or `?codecs=` | the original when the codec is listed, else FLAC | `original` or `flac-fallback` |
| `?transcode=1` | FLAC made on the fly, same sample rate and bit depth (lossless, but converted) | `flac` |
| `?q=aac&kbps=<32–320>` | AAC (ADTS), default 256 kbps | `aac-<kbps>` |
| `?q=opus&kbps=<32–320>` | Opus in Ogg | `opus-<kbps>` |

A converted stream has no `Content-Length` and does not seek; a client reads it to the end and counts
the bytes itself.

## Where each client keeps it

### Web (0.28.0)

Everything is in the Cache API, in the cache **`encore-offline-v1`** on the page's origin. The keys are
synthetic URLs under `/offline/`: the server has no such route and the service worker never answers
them; only the page's offline store reads and writes them with `cache.match` / `cache.put`.

| Key | Content |
|---|---|
| `/offline/index.json` | the index (this document's records, `content-type: application/json`) |
| `/offline/outbox.json` | plays and likes waiting for the server (below) |
| `/offline/audio/<id>` | the audio, with `content-type` of the saved codec and `x-encore-bytes: <bytes>` |
| `/offline/cover/<album_id>` | the album cover (`/api/library/albums/{album_id}/cover?s=600`), one per album, shared by its songs whose `cover` is true; removed with the album's last song |
| `/offline/lyrics/<id>` | the JSON text of `/api/library/tracks/{id}/lyrics`, when `lyrics` is true (saved only when it has words) |

`<id>` is the track id and `<album_id>` the album id with only letters, digits, `-` and `_` kept. The web
store keeps one server's songs (its own origin), so `<id>` alone is the key; `source_server` is
still written so an exported index stays correct. The index and the outbox are read, changed and written
back under one lock: the page's own, plus the Web Locks lock `encore-offline` that every tab of the site
shares where the browser has it (Chrome 69, Firefox 96, Safari 15.4 and later); a flush of the outbox also
holds `encore-offline-flush` across tabs. Every entry the web store writes carries `x-encore-at` (Unix
seconds). A download writes the audio last, right before its record; once per page, after sign-in, audio and
lyrics without a record and covers no record uses are deleted when they are more than an hour old. The service worker's `activate` deletes old `encore-shell-*` caches and
never an `encore-offline-*` cache. The audio is played from an object URL made from the cached
`Response`'s blob; the player revokes it when it moves to the next song.

The signed-in user is remembered for offline mode in `localStorage` `encore.offline.me` as
`{user, cached_at}` (Unix seconds). Offline mode lets downloaded songs play for **30 days** after
`cached_at`; after that the client asks to sign in again when the server is back.

### Desktop (Iced app)

`<data dir>/encore-desktop/offline/index.json`, with the files in the same folder:

- `<data dir>` is the platform's local data folder (`%LOCALAPPDATA%` on Windows,
  `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or `~/.local/share` on Linux);
  `ENCORE_DESKTOP_DATA_DIR` replaces `<data dir>/encore-desktop`;
- audio: `<id>-<server tag>.<ext>`, the cover: `cover-<album_id>-<server tag>.<ext>` (one per album
  and server), where `<server tag>` is 8 hex digits derived from `source_server`;
- a download is written to `<id>-<server tag>.part` and renamed when complete; leftover `.part` files
  are deleted when the app starts saving again;
- the index is written to `index.json.tmp` and renamed over `index.json`.

### Phone (0.30)

- audio: `Music/Encore/<Album artist>/<Album>/NN - Title.ext` on shared storage, so the songs are plain files
  any file manager or player sees: through MediaStore Audio (with `IS_PENDING` while writing) on Android 10
  and later, as plain files on Android 8 and 9;
- on Android 10 and later the index, covers and lyrics stay in the app's own storage, because Android lets an
  app put only audio in `Music/`: `files/offline/.encore-index.json`, `covers/<server tag>-<album_id>.jpg`,
  `lyrics/<server tag>-<id>.json`;
- on Android 8 and 9 the index is `Music/Encore/.encore-index.json` (with a copy in the app's storage) and the
  cover is `cover.jpg` in the album folder;
- `file` is relative to `Music/Encore/`; `art` is relative to the folder that holds the index;
- a converted copy is FLAC with the quality `FLAC (converted)`;
- the queued plays and likes are `files/offline/outbox.json`, `{owner, plays, likes}`, sent only for the user
  who queued them.

## Upgrading an older index

Readers upgrade on read and write the new shape at the next change. Two older shapes exist:

- the classic app (Tauri, `clients/app`): `{id, title, artist, album, ext, file, art}` with `id` a string,
  `file` and `art` asset URLs or file names, and no other fields;
- the desktop app 0.27: the classic fields plus `album_id`, `bytes`, `quality` (`FLAC`, `MP3 320`),
  `saved_at` (with a fraction), `source_server` and `duration`.

| Field | When missing or old |
|---|---|
| `id` | A string of digits becomes a number. Anything else: the record is skipped. |
| `ext` | The extension of `file`; else `format`; else `audio`. |
| `bytes` | The size of the audio file on disk (file-based clients); else 0. |
| `quality` | Kept as it is when present (desktop 0.27 wrote `FLAC`, `MP3 320`). Missing: the upper-case `ext` (`FLAC`). |
| `cover` | True when `art` is not empty (file-based clients also check that the file exists). |
| `lyrics` | False. |
| `saved_at` | The audio file's modified time; else 0. A fraction is kept on read and rounded down on write. |
| `source_server` | Kept as written; a record without it (the classic app's shape) reads as `""`, and the desktop app keeps it that way. A client may fill it with the server it is signed in to now. |
| `album_id` | A number becomes its decimal text; missing or null becomes `""`. |
| `format` | The lower-case `ext`. |
| `file`, `art` | An asset URL (`asset://…`, `https://asset.localhost/…`) becomes its last path segment, decoded. |
| `duration`, `track`, `disc`, `artists` | 0, 0, 0 and `[]`. |

No old record is ever a converted copy: the classic and 0.27 desktop apps did not mark them, so an
upgraded record never ends in `(converted)`.

## The outbox (web)

Plays and likes made while the server cannot be reached wait in `/offline/outbox.json`:

```json
{
  "user_id": 3,
  "plays": [{ "track_id": 1234, "seconds": 241.5, "played_at": 1792310400 }],
  "likes": [{ "track_id": 1234, "liked": true, "at": 1792310460 }]
}
```

- `user_id` is the user who made the changes; a change by another user drops the outbox first, and
  `flush()` asks `GET /api/auth/me` whose session the browser holds and sends only that user's outbox
  (another user's is dropped; an outbox without `user_id` is never sent), so one user's plays and likes
  never go out under another user's session;
- a play is stamped with `played_at` (Unix seconds) when it is queued; the same song at the same second
  is kept once; at most 5000 plays wait (the oldest go first);
- a song's likes and unlikes collapse into the last one (the server's like and unlike change nothing when
  the song is already so, so the last tap is what it gets); `at` is when the heart was pressed;
- when the cache refuses the document (a full device), it is kept in `localStorage`
  `encore.offline.outbox` until the cache takes it again; while that copy exists it is the one read;
- `flush()` sends the plays with the list form of `POST /api/library/plays` (below), 500 per call, then
  the likes one by one (`POST /api/likes/{id}` to like, `DELETE /api/likes/{id}` to unlike; a 404 counts
  as sent, the song is gone); what was sent is removed, what failed stays for the next try.

## Server additions (0.28.0)

### `POST /api/library/plays`: a list of plays

The single form is unchanged: `{"track_id": 1234, "seconds": 241.5}` is saved with `played_at` = now and
answers `{"ok": true}` (404 for an unknown song).

The list form saves plays made earlier, each with its own time:

```json
{ "plays": [ { "track_id": 1234, "seconds": 241.5, "played_at": 1792310400 },
             { "track_id": 1235, "seconds": 30 } ] }
```

A bare JSON array of the same objects is accepted too.

- `played_at` is Unix seconds, clamped to [now − 400 days, now]; missing means now;
- `seconds` is clamped to [0, 7200] as in the single form;
- each play is inserted with its own `played_at` and scrobbled (Last.fm, ListenBrainz) with the start
  time `played_at − seconds`, one scrobble every quarter second after the answer;
- an unknown track id is skipped, not an error; so is a play that is already saved (the same song at the
  same `played_at` for this user, e.g. a list sent again after its answer was lost), so a client can
  safely send a list twice;
- the plays of one call are saved in one statement: all or none;
- the answer is `{"ok": true, "saved": n, "skipped": m}`;
- more than 500 plays in one call: `413`, nothing saved; a list entry that is not a play: `422`.

### `GET /api/library/tracks/{id}/info`: `offline_ok`

Next to `stream: {kind, codec, reason}` the answer carries `offline_ok: bool`: true only when
`stream.kind == "original"`, i.e. the original file plays in the asking client and can be saved as it is.
A client sends the same `X-Encore-Decoders` header (or `?codecs=`) it will use for the download.

### Streams that must not be stored

The converted streams of `/api/library/tracks/{id}/stream` (`flac`, `flac-fallback`, `aac-<kbps>`,
`opus-<kbps>`) and the Cadenza render (`?dsp=1`, `cadenza-<rate>`) answer
`Cache-Control: private, no-store` (before 0.28 the Cadenza render said `private, no-cache` and the
conversions carried the API's default `no-store`). The original file keeps `Cache-Control: private, max-age=3600` and
`Vary: X-Encore-Decoders`. The offline store does not rely on HTTP caching: it reads the body and puts
its own `Response` in `encore-offline-v1`, so `no-store` does not stop a download.

### The web app's files, pre-compressed

The image carries `<file>.br` and `<file>.gz` next to every `.js`, `.wasm`, `.css`, `.html`, `.svg`,
`.webmanifest` and `.json` of the web app; the server sends the brotli one when the browser takes it, else the gzip one
(`Content-Encoding: br` or `gzip`, with `Vary: accept-encoding`) and the plain file to a client without
`Accept-Encoding`; `index.html` for a deep link the same way. The
service worker's shell cache stores what the browser decoded, so nothing changes for the offline store.
