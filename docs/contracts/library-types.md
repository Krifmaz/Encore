# Library types, bulk delete to Trash, One-offs, who added it – build contract

Written Oct 21, 2026 (read-only design pass over the tree at b2ae496 + the uncommitted group patch). Target: the
release after 0.35.2 (call it 0.36.0). Schema **31**. Three owner asks of Oct 21, built together because they share
the album list, the album page and the track rows:

1. "group delete albums or by category like live" → Library → Albums: **Type filter**, **Select** mode, **Delete N
   albums (X GB)?** → files go to the **Trash of the 0.33 file manager** (restorable). Admins only. Web, phone, desktop.
2. "how do we handle one off?" → a per-album **format** (Album / EP / Single / **One-off**) guessed automatically,
   **Change type** in the album ⋯ (stored per album, wins over the guess, survives rescans), One-offs leave the Albums
   grid and get a **One-offs** shelf on artist pages and a **One-offs** entry in the Type filter.
3. "where can i see who requested it" → **who added each song is stored on the song** (`added_by`, `added_via`,
   `requested_at`) at import, backfilled from the requests still there; album page + song ⋯ say "Requested by X on
   DATE" / "Uploaded by X" / "Found in the music folder" / "Requested by someone – record cleared" on all clients.

Out of scope (do not touch): zotify/gamdl; the Requests page; group listen; changing which release `import_single`
tags a lone song with (noted in §11 as a possible follow-up); Remove artist (stays a permanent delete).

---------------------------------------------------------------------------------------------------------------------

## 0. What exists today (facts the design builds on)

- Albums are not a table: `tracks.album_id = sha1("albumartist|album")[..16]` (`library/src/scan.rs album_key`; a song
  without an album tag is `dir|<folder>`). Edit details / Tag-match / a folder move of an untagged album change it.
- `/api/library/albums` (`api/src/routes/library.rs albums_answer`) is one cached GROUP BY over `tracks` per
  `Db::library_version` (`cached_list`), pages are slices. `kind=albums` = `HAVING COUNT(*) > 2` (web/desktop Albums
  tab), `kind=singles` = `COUNT(*) <= 2` (web/desktop Singles tab), no kind = all (phone). `filter=` untagged / tagged /
  lossless. `library/src/sections.rs` repeats the same HAVING for the side index (offsets must match the list).
- Tracks carry `mb_releasegroupid`, `mb_albumid`, `mb_albumartistid`, `mb_trackid`, `request_id`, `source`,
  `source_detail`, `source_url`, `downloaded_at`, `added_at`. **No release-type tag is read** (`media/src/tags.rs
  tags_of` ignores `RELEASETYPE` / `MusicBrainz Album Type` / `TCMP`/`cpil`/`COMPILATION` / `TRACKTOTAL`, all of which
  lofty 0.25 maps: `ItemKey::MusicBrainzReleaseType`, `ItemKey::FlagCompilation`, `ItemKey::TrackTotal`). The tagger
  writes no type either (`release_values` puts `"type"`, which `tags.rs key()` drops).
- `encore_config::release_types` already knows kinds/extras: `KINDS`, `EXTRAS`, `canon`, `primary`, `marks`,
  `group_kind` (first MB secondary type, else for Deezer/Apple ext albums the EXT title regexes, else the primary),
  `group_rank`, `label`. `catalog.rs grouped()` uses `group_kind` for the artist page sections. MusicBrainz titles are
  never moved by title words except their version part (OWN regexes on brackets / " - …" / disambiguation).
- `mb::release` returns the release group's `primary-type` only (secondary types are in the same answer, unused);
  `mb::recording` (used by `tagger::import_single`) takes the **first release** of the recording and tags the song with
  its title as album, **without** `musicbrainz_albumid`/`releasegroupid` → the 1-song "albums" the owner calls one-offs
  (often a compilation / remix album / soundtrack listed first).
- The MusicBrainz answer cache is in memory, 1 h (`http/src/mb.rs`); there is no stored release-group type anywhere.
- Delete: `DELETE /api/library/albums/{id}` (admin) → `delete_tracks` → `std::fs::remove_file` (permanent).
  (0.38: the Trash is `<library>/.encore-trash`, and while `trash_keep` is off – the default – a delete is for good:
  the dry run says `keep` and `days`, the real run's `text` is "Deleted N albums", and the menu item is Delete album….)
  The 0.33 file manager Trash (`files/src/trash.rs`) moves an item into `<config>/trash/<unix>-<n>/` with an
  `.origin.json` note (root, path, by, size, saved rows: id, rel, playlist item ids); `rows::Change` takes the rows out
  (playlist entries/likes kept as missing), restore reads the files in again and `rows::give_back` gives back old ids +
  playlist entries. **Restore loses every other row column** (request_id, source, added_at …) because the row is
  re-made by `index_paths`. Trash on another disk = copy (Docker: `/config` vs `/music` usually are) with a room check.
- Who requested it: album route joins `tracks.request_id → requests.user_id → users.username` (`requested_by`,
  `requested_at`). `POST /api/requests/clear` **DELETEs request rows** (`routes/requests.rs` ~547) → the name is lost.
  Scans, Files uploads/imports have no record at all. Soulseek-page downloads (`routes/soulseek.rs download`) *are*
  requests (`create_pick` source slskd, no mbid) and lose the name the same way. Lossless-upgrade requests have
  `user_id NULL` (`requests/src/upkeep.rs`).
- `requests/src/worker/import.rs record_origin` stamps `source, source_detail, source_url, request_id, downloaded_at`
  on rows new since the request (`added_at >= since - 1`).
- `owns_request` (members with the "fix" permission may correct songs they requested) also dies with Clear finished.
- Migrations: files run on every open, idempotent; newest is `0030_surround.sql`, `SCHEMA_VERSION = 30`,
  `LIBRARY_TABLES = ["tracks", "artist_alias"]` (writes to them move `library_version` → every list cache rebuilds).
- Strings rule: every phone (and desktop) UI string must exist in web-rs (the STRINGS check); web-rs is written first.

---------------------------------------------------------------------------------------------------------------------

## 1. Storage – migration `0031_library_types.sql` (schema 31)

`server/crates/db/migrations/0031_library_types.sql`, listed in `db/src/lib.rs MIGRATIONS` as
`(31, "library_types", include_str!("../migrations/0031_library_types.sql"))`, `SCHEMA_VERSION = 31`.
If another build lands a 31 first, renumber to the next free number (file name, MIGRATIONS entry, SCHEMA_VERSION,
this contract). `LIBRARY_TABLES` becomes `["tracks", "artist_alias", "album_types"]`.

```sql
-- Schema 31 (0.36): who brought each song in, the release-type tags of the files, and each album's type
-- (Library → Albums Type filter, One-offs, Change type). docs/contracts/library-types.md.

-- who added the song: kept on the song, so Clear finished (which deletes request rows) no longer loses it
ALTER TABLE tracks ADD COLUMN added_by INTEGER;      -- users.id (NULL: nobody / not known)
ALTER TABLE tracks ADD COLUMN added_via TEXT;        -- request | link | soulseek | upload | import | scan
ALTER TABLE tracks ADD COLUMN requested_at REAL;     -- the request's created_at (request, link, soulseek)
-- the release's type as the file's tags say it (NULL: not read yet – scanned before 0.36; '' / 0: no such tag)
ALTER TABLE tracks ADD COLUMN releasetype TEXT;      -- RELEASETYPE / MUSICBRAINZ_ALBUMTYPE / "MusicBrainz Album Type"
ALTER TABLE tracks ADD COLUMN tracktotal INTEGER;    -- TRACKTOTAL / TOTALTRACKS / the "/n" of TRCK
ALTER TABLE tracks ADD COLUMN compilation INTEGER;   -- COMPILATION / TCMP / cpil (1 = part of a compilation)
-- the Surround library has the same columns as tracks (the scanner writes either, scan::COLS)
ALTER TABLE surround_tracks ADD COLUMN added_by INTEGER;
ALTER TABLE surround_tracks ADD COLUMN added_via TEXT;
ALTER TABLE surround_tracks ADD COLUMN requested_at REAL;
ALTER TABLE surround_tracks ADD COLUMN releasetype TEXT;
ALTER TABLE surround_tracks ADD COLUMN tracktotal INTEGER;
ALTER TABLE surround_tracks ADD COLUMN compilation INTEGER;
CREATE INDEX IF NOT EXISTS idx_tracks_added_by ON tracks(added_by);

-- one row per album that has facts or an owner's choice (album_id = tracks.album_id)
CREATE TABLE IF NOT EXISTS album_types (
    album_id TEXT PRIMARY KEY,
    chosen TEXT,               -- the owner's format: album | ep | single | one_off (NULL: automatic)
    chosen_by INTEGER,
    chosen_at REAL,
    release_source TEXT,       -- musicbrainz | deezer | apple | discogs: where the facts below come from
    release_id TEXT,           -- the MusicBrainz release-group id (or the provider's album id)
    release_type TEXT,         -- primary type: Album | EP | Single | Broadcast | Other ('' none given)
    release_secondary TEXT,    -- JSON array of secondary types, MusicBrainz words: ["Live"], ["Compilation","Remix"]
    release_tracks INTEGER,    -- songs on the release (0: unknown)
    looked_up_at REAL          -- when the facts were taken (import or lookup). NULL: never; set with
                               -- release_type NULL: MusicBrainz didn't know it (looked again after 30 days)
);

-- backfill of who added the songs already there, from the requests still in the table (only rows not filled
-- yet, so running it again on every start changes nothing; json_valid guards hand-made detail text)
UPDATE tracks SET
    added_by = (SELECT r.user_id FROM requests r WHERE r.id = tracks.request_id),
    requested_at = (SELECT r.created_at FROM requests r WHERE r.id = tracks.request_id),
    added_via = COALESCE((SELECT CASE
            WHEN r.kind = 'url' THEN 'link'
            WHEN r.mbid IS NULL AND json_valid(r.detail) AND json_extract(r.detail, '$.pick.source') = 'slskd'
                 AND json_extract(r.detail, '$.pick.payload.username') IS NOT NULL THEN 'soulseek'
            ELSE 'request' END
        FROM requests r WHERE r.id = tracks.request_id), 'request')
    WHERE added_via IS NULL AND (request_id IS NOT NULL OR source IS NOT NULL OR downloaded_at IS NOT NULL);
UPDATE tracks SET added_via = 'scan' WHERE added_via IS NULL;
-- the same two statements for surround_tracks
```

Notes for the builder:
- A request row that is gone (cleared) leaves `added_by NULL, requested_at NULL, added_via 'request'` → shown as
  "Requested by someone – record cleared". A request whose user was deleted keeps `added_by` = that id → "a deleted
  user" (§2.4).
- The two UPDATEs run on every open; after the first one nothing is NULL (every write path in §2.1 sets `added_via`),
  so each later run is one table scan (~5 ms on 20 000 songs). Do **not** guard them with a settings flag: the layout
  test (`migrations_run_again_change_nothing`) only compares tables/indexes, and an idempotent WHERE is the house style.
- **One-time Rust backfill from the activity log** (`encore_files::backfill_added_from_log`, run once at startup,
  marked done in `settings` key `added_from_log_v31`): the 0.33 file manager logged `File manager: <user> uploaded
  <root>:<rel>`, `… imported into the library <root>:<rel>`, `… imported in place <root>:<rel>` (≤ 2000 activity rows
  are kept). For each such line, rows whose path is that file (or under that folder) with `added_via='scan'` and
  `added_at >= line.ts - 120` become `upload` / `import` with `added_by` = that user's id (username → id; an unknown
  name leaves the row alone). Cheap (≤ 2000 lines, indexed path lookups); a failure only logs a warning.

---------------------------------------------------------------------------------------------------------------------

## 2. Who added it

### 2.1 Write points (every path that makes a `tracks` row sets `added_via`)

| Path | Code | added_via | added_by | requested_at |
|---|---|---|---|---|
| Library scan / any file read in without an owner | `scan.rs store_file` INSERT (via `index_files`) | `scan` | NULL | NULL |
| Request import (album, song, any source) | `requests/src/worker/import.rs record_origin` (UPDATE, same WHERE as request_id) | `request` | request.user_id | request.created_at |
| Request of kind `url` (Downloader "any link") | same | `link` | user | created_at |
| Soulseek page download | `routes/soulseek.rs download` puts `"via": "soulseek"` in the request detail (extra of `create_pick_for`); `record_origin` reads it | `soulseek` | user | created_at |
| Files → Upload | `files/src/upload.rs finish` → `Change` arrivals | `upload` | who.id | NULL |
| Files → Import (tag + move, or kept in place) | `files/src/import.rs apply` (both `index_paths` calls) | `import` | who.id | NULL |
| Files → Move/rename a file *into* the library from a download folder | `files/src/ops.rs` → `Change::moved` → arrived | `import` | who.id | NULL |
| Files → Restore from Trash | `trash.rs restore_one` → `rows::give_back` | the saved value (§5.3) | saved | saved |

Mechanics:
- `scan.rs`: new `pub struct Added { pub by: Option<i64>, pub via: &'static str }` (`Added::SCAN`), `index_files_as(db,
  paths, added)`; `index_files` = `index_files_as(.., Added::SCAN)`. `store_file` writes `added_by, added_via` **only
  on INSERT**; an UPDATE (rescan, retag, same path written again) never touches them.
- `tagger::index_paths_as(st, paths, added)`; `index_paths` keeps `Added::SCAN` (request imports are stamped by
  `record_origin` right after, as `request_id` is today). `rows::Change` gets `pub added: Added` (default SCAN) used for
  its arrivals; upload/import/move callers set it from `Who`.
- `record_origin(st, paths, origin, rid, since)` gains the request row: `added_by`, `added_via`, `requested_at` are set
  in the same UPDATE and under the same WHERE as `request_id`. **A request without a user** (lossless upgrade, any
  `user_id NULL`) does not overwrite `added_by/added_via/requested_at` (the new copy keeps what it inherited, below).
- Inheritance when a new copy replaces an old one: `scan::delete_tracks(ids, Some(replacement))` (used by
  `drop_superseded` and `import_single`'s "replaced a lower-quality copy") copies `added_by, added_via, requested_at`
  from the deleted row onto the replacement **when the replacement's `added_via` is `scan`** (fresh import not stamped
  yet). A user request then overwrites it in `record_origin` (that user brought the new copy); a system request keeps
  the inherited "Requested by alex on Oct 13". `requests/src/report.rs rekey` (a reported song's replacement taking the
  old id) copies the parked row's three values onto the new row the same way.
- `owns_request` (api/routes/library.rs) becomes `owns_song`: true when the user has "fix" and (the request row's user
  is them **or** `tracks.added_by = me AND added_via IN ('request','link','soulseek')`) – so members keep "This is the
  right song" / "Wrong song – remove it" after Clear finished. `delete_track` and `verify_manual` use it.

### 2.2 Read points – API

- `GET /api/library/albums/{id}` (album route): the SELECT keeps `t.*` (now with `added_by, added_via, requested_at`)
  and joins the adder: `LEFT JOIN users ua ON ua.id = t.added_by`. Per track it returns:
  - `added_via`, `added_by` (id), `added_by_name` (username or null), `added_by_deleted` (bool: `added_by` set, no such user)
  - `requested_by` (kept for older apps) = `added_by_name` when `added_via` ∈ request/link/soulseek, else the old join
    (`COALESCE(ua.username, u.username)`), `requested_at` = `COALESCE(t.requested_at, r.created_at)` – **drop the old
    `r.created_at requested_at` alias**, it would collide with the new column inside `t.*`.
  - album level: `"history": [{"via","by","by_deleted","requested_at","songs"}]` – the album's songs grouped by
    (`added_via` class, `added_by`), request-like vias (request/link/soulseek) one class, ordered by `songs` desc then
    earliest `requested_at`/`added_at`; `"added_at"`: MIN(added_at). Clients only format it (§2.4).
- `GET /api/library/tracks/{id}/added` (new, any signed-in user; stereo or surround id via `table_of`) →
  `{"via": "request", "by": "alex", "by_id": 3, "by_deleted": false, "requested_at": 1729…, "added_at": 1729…}`;
  404 "Track not found". One indexed row + one user lookup; never MusicBrainz. Used by the song ⋯ menu everywhere
  (lists don't carry it: the list answers and the enrich fixtures stay as they are).

### 2.3 Where it shows (all clients)

- **Album page** – the history line under the artist/year line (today's spot): groups from `history`, at most 3,
  joined with " · ", then " · and N more" if more groups, then the added date: " · added DAY", or "Added DAY" when no
  group has text. Example: `Requested by alex on Oct 13, 2026 · added Oct 14, 2026`.
- **Album page song rows** – the existing source line's "· requested by X on DAY" part now reads `added_by_name` /
  `requested_at` (so it survives Clear finished); same words as today, only for request-like vias with a name.
- **Song ⋯ menu** (every list: album page, Songs tab, playlists, search, queue, player, artist page, car-less) – a
  first, muted, non-tappable line with the text from `/tracks/{id}/added` (fetched when the menu opens; until it
  answers, or when the text is empty, the line is not there). New menu item kinds: web `MenuItem::info(label)`, desktop
  `MenuItem::info(label)` (no message, muted, not hoverable), phone `MenuEntry(label, info = true, onClick = {})`.

### 2.4 The texts (same words on every client; DAY = the client's existing day formatter)

| via | by | by_deleted | requested_at | text |
|---|---|---|---|---|
| request / link / soulseek | alex | – | known | `Requested by alex on DAY` |
| request / link / soulseek | alex | – | none | `Requested by alex` |
| request / link / soulseek | null | false | – | `Requested by someone – record cleared` |
| request / link / soulseek | null | true | known | `Requested by a deleted user on DAY` |
| upload | sam | – | – | `Uploaded by sam` |
| upload | null | true | – | `Uploaded by a deleted user` |
| import | sam | – | – | `Imported by sam` |
| import | null | true | – | `Imported by a deleted user` |
| scan | – | – | – | `Found in the music folder` |
| null / unknown | – | – | – | `` (nothing) |

The en dash in "someone – record cleared" is U+2013 with spaces, as in the other Encore texts. Each client has one
pure function `added_text(via, by, by_deleted, day: Option<&str>) -> String` and one `history_line(groups, added_day)`;
the table above is copied verbatim into each client's tests (§9.4).

---------------------------------------------------------------------------------------------------------------------

## 3. Album types

### 3.1 Two facts per album, one filter key

Every album gets, from the classification (§3.3):
- `format` – what the album is as a thing on the shelf: `album` | `ep` | `single` | `one_off`. **Change type sets this.**
- `kind` – what is on it: `studio` | `live` | `compilation` | `remix` | `soundtrack` | `demo` | `dj_mix` | `mixtape` |
  `karaoke` | `other`. Always automatic.
- `type` – the **Type filter key**, a partition (every album in exactly one, counts add up to the total):
  `one_off` if format is one_off; else `kind` if kind ≠ studio; else `singles` if format is single/ep; else `album`.

Mapping of `release_types` keys to `kind`: Live→`live`; Compilation→`compilation`; Remix→`remix`;
Soundtrack→`soundtrack`; Demo→`demo`; DJ-mix→`dj_mix`; Mixtape/Street→`mixtape`; Karaoke, Instrumental→`karaoke`;
Spokenword, Interview, Audiobook, Audio drama, Field recording, Sped up, Covers→`other`; Acoustic→`studio` (an
acoustic version is still the artist's own album; "Unplugged" live sets are Live through the EXT/MB rules);
primary Broadcast or Other with no secondary → `other`.

Labels (server-owned; the clients show what the server sends):

| filter `type` | filter label (Type dropdown) | | format | chip word | | kind | chip word |
|---|---|---|---|---|---|---|---|
| album | Studio albums | | album | Album | | studio | – |
| live | Live | | ep | EP | | live | Live |
| compilation | Compilations | | single | Single | | compilation | Compilation |
| remix | Remixes | | one_off | One-off | | remix | Remix |
| soundtrack | Soundtracks | | | | | soundtrack | Soundtrack |
| demo | Demos | | | | | demo | Demo |
| dj_mix | DJ mixes | | | | | dj_mix | DJ mix |
| mixtape | Mixtapes | | | | | mixtape | Mixtape |
| karaoke | Karaoke / Instrumental | | | | | karaoke | Karaoke / Instrumental |
| singles | Singles & EPs | | | | | other | Other |
| one_off | One-offs | | | | | | |
| other | Other | | | | | | |

Type dropdown order = the table order (owner's order, Other last). The album page chip (`type.label`):
kind ≠ studio → `<kind word>` + (format ≠ album → ` · <format word>`); kind studio → `<format word>`.
Examples: `Album`, `EP`, `Live`, `Live · EP`, `Remix · Single`, `Compilation · One-off`.

### 3.2 Facts – where they come from (no MusicBrainz call on any request path)

1. **At import** (free: the answers are already fetched):
   - `tagger::import_album_files` / `retag_album` / `apply_preview` with a MusicBrainz release: `mb::release` also
     returns `"secondary": g["secondary-types"]` and `"track_count"` (songs over all media). After `index_paths`, upsert
     `album_types` for the album_id(s) the new files landed in: `release_source='musicbrainz'`, `release_id=rgid`,
     `release_type`, `release_secondary`, `release_tracks`, `looked_up_at=now` (keep `chosen*`). Also write the tags
     `releasetype` = Picard style lowercase `"album; live"` and, when the album artist is Various Artists,
     `compilation` = `1` (`tags.rs key()`: `"releasetype" → ItemKey::MusicBrainzReleaseType`, `"compilation" →
     ItemKey::FlagCompilation`) – other servers (Navidrome, Jellyfin) read them too, and a database restored from an
     older backup gets them back from the files.
   - Deezer / Apple / Discogs releases (`meta::release`, provider ≠ MusicBrainz): `release_source` = the provider,
     `release_type` = the record type title-cased (Album/EP/Single; "Compile" → Album + secondary ["Compilation"]),
     `release_secondary` = `[]` (+Compilation), `release_tracks` = its track list length. Titles are judged by the
     ext path (EXT + OWN regexes) as `release_types::marks` does for ext albums.
   - `tagger::import_single`: `mb::recording` also returns, for the release it picked, `"release_type"`,
     `"release_secondary"`, `"release_track_count"` (sum of `media[].track-count` when MusicBrainz sends them, else 0)
     and `"release_group_id"` (already there). Upsert the facts for the song's album_id. **Do not** write
     `musicbrainz_releasegroupid`/`albumid` into the song's tags or row: `has_release_group`/`annotate_albums` would
     then call the whole compilation "in the library" and block requesting it.
2. **From the files' tags** (scan, free): `tags_of` reads `releasetype` (`ItemKey::MusicBrainzReleaseType`; '' when
   absent), `tracktotal` (`ItemKey::TrackTotal`, else the "/n" of the track number; 0 when absent), `compilation`
   (`ItemKey::FlagCompilation` "1"/"0"; 0 when absent). `scan::COLS` gets `"releasetype", "tracktotal",
   "compilation"` (so surround rows carry them too). Parsing `releasetype`: split on `;` `/` `,` NUL, trim, lowercase
   → first token = primary via `release_types::canon` (album/ep/single/broadcast/other), the rest = secondaries via
   `canon`. An album's value = the most common non-empty value among its songs.
3. **Background pass "Sort albums by type"** (scheduled task, §3.5): reads the tags of songs scanned before 0.36 and
   looks up, at MusicBrainz `Priority::Background`, the albums that have no facts.

### 3.3 Classification rules, in order

Inputs per album (one in-memory pass, §7): `present` (its songs in `tracks`), album title, album artist,
`mb_albumartistid`, the songs' titles and artists, `mb_releasegroupid`, `releasetype`/`tracktotal`/`compilation` of its
songs, max track number, and its `album_types` row.

Facts → an `rg` JSON for `release_types` (`{title, type, secondary, disambiguation:""}` – with `"ext": {}` when the
facts are not MusicBrainz, so the ext title rules apply), picked from the **first** source that has a primary type:
(a) `album_types` facts (MusicBrainz, then provider), (b) the `releasetype` tag, (c) none → `{title, type: "", ext: {}}`.

**kind** (first rule that gives a non-studio answer):
1. MusicBrainz / tag secondaries: `release_types::group_kind(rg)` when it returns a secondary key → mapped (§3.1).
   With MusicBrainz facts the title words are **not** read (as today: "Live Through This" is an album).
2. No MusicBrainz/tag facts: the ext title rules – the first of `release_types::marks(rg)` (EXT + OWN on the whole
   title: "(Live at …)", "Remixes", "Original Motion Picture Soundtrack", "Greatest Hits", "Demos", "Karaoke" …).
3. Various artists or a compilation flag: album artist normalised ∈ {various artists, various, va, v a, verschiedene
   interpreten, artistes divers} or `mb_albumartistid = 89ad4ac3-39f7-470e-963a-56509c546377`, or `compilation = 1`
   on most songs → `compilation`.
4. Song-title share (any source, only while kind is still studio): ≥ 60 % of the album's songs (all of them when
   it has 1 or 2) carry the same marker in brackets or after " - " → that kind:
   - live: `(?i)[\(\[][^\)\]]*\blive\b[^\)\]]*[\)\]]|\s-\s+live\b`
   - remix: `(?i)([\(\[][^\)\]]*|\s-\s+[^-]*)\b(remix(ed)?|rmx|club mix|extended mix|dub mix|vip mix|bootleg)\b`
   - karaoke: `(?i)([\(\[][^\)\]]*|\s-\s+[^-]*)\b(karaoke|instrumental|off[\s-]?vocal|backing track)\b`
5. Else `studio`; a primary Broadcast/Other with no secondary → `other`.

**format** (first rule that decides):
1. **The owner's choice** (`album_types.chosen`) – always wins (`why = chosen`).
2. **One-off** – all of:
   - `present ≤ 3`, and
   - the release is bigger: `total = max(release_tracks, most common tracktotal, max track number)`;
     `total = 0` (unknown) counts as bigger; else `present * 2 ≤ total`, and
   - one of: kind ∈ {compilation, soundtrack, remix, dj_mix}; or the album is Various Artists (rule 3 above); or
     **a lone song of a larger release**: `present ≤ 2`, the album title (`catalog::title_key`) equals none of its song
     titles, and the release is not known to be a Single or EP (primary from the facts, or " - Single"/" - EP" title).
   (`why = partial`: "Only N of its M songs are here" / "Only N of its songs are here".)
   The third bullet is a deliberate reading of the owner's words ("not albums not singles") and keeps 1–2-song
   "albums" out of the Albums grid as the web's `COUNT(*) > 2` does today. Strike that bullet if the owner disagrees:
   then only compilation/remix/VA/soundtrack partials are One-offs (§11).
3. The facts' primary type: Album → `album`, EP → `ep`, Single → `single`, Broadcast/Other → `album`; a Deezer/Apple
   title ending " - Single" / " - EP" → single / ep (`catalog::kind_of`).
4. No primary known: `present ≤ 2` → `single`; else `album` (`why = size`).

`why` keys and texts (server sends both; the Change type dialog shows the text of the automatic answer):
`chosen` "You chose this" · `musicbrainz` "From MusicBrainz" · `provider` "From Deezer" / "From Apple Music" /
"From Discogs" · `tags` "From the file tags" · `title` "From the album title" · `songs` "Most of its songs say so in
their titles" · `various` "Various artists" · `partial` "Only N of its M songs are here" (M unknown: "Only N of its
songs are here"; N = 1: "Only 1 of its …") · `size` "Guessed from the number of songs" · `default` "No type known".
`why` is the rule that decided the **format** when that rule is chosen/partial, else the rule that decided the kind,
else the format rule.

The `release_label` of a One-off (artist shelf sub-line): its kind word when kind ≠ studio, "Various artists" for a VA
album without other kind, else its primary ("Album", "EP", "Single").

### 3.4 Owner's choice

- Stored in `album_types.chosen/chosen_by/chosen_at` keyed by album_id → survives rescans (a rescan never rewrites
  album_types; album_id is stable while the album/album-artist tags are).
- Follows the album when its id changes by Encore's own hand: after Edit details (`/api/library/tags` when every song
  of the album was edited), Tag / match (`retag_album`), Organize, and `rows::Change` (an untagged album's folder
  moved): when **all** songs of old id A now have one new id B and B has no row → `UPDATE album_types SET album_id=B
  WHERE album_id=A`. Facts of A are dropped on a retag (the retag writes the new release's facts).
- Rows of albums with no songs: the background pass deletes them when `chosen IS NULL`; rows with a choice stay (an
  album restored from Trash gets its choice back; a few bytes each).
- Writes to `album_types` move `library_version` (`LIBRARY_TABLES`) so every cached list sees the change at once.

### 3.5 Background pass "Sort albums by type"

- New scheduled task in `jobs/src/scheduler.rs TASKS` (array grows to 15): `("types", "Sort albums by type", "Work out
  which albums are live, compilations, remixes, soundtracks, singles or one-offs: reads the type the files' tags give
  and, for albums without one, asks MusicBrainz at low priority. Imports record the type themselves.")`; settings
  `task_types_enabled` (default on), `task_types_minutes` (default 360) in `config/settings.toml` + `keys()`; run in
  `api/src/scheduler.rs` as `"types" => encore_library::album_types::scheduled(st).await?`; first run 3 minutes after
  a start (the scheduler's boot rule).
- Step A – tags: `SELECT id, path FROM tracks WHERE releasetype IS NULL ORDER BY id LIMIT 200`, read the tags on the
  blocking pool, write `releasetype/tracktotal/compilation` for the batch in one transaction (a missing/unreadable file
  gets `''/0/0`, so it is never read again); repeat until none; waits while `Library.busy` says the app is in use
  (as the loudness pass does). 20 000 songs ≈ 1–3 minutes once, then nothing.
- Step B – MusicBrainz (inside `mb::with_priority(Priority::Background, …)`), at most 300 lookups per run:
  candidates = albums without a `releasetype` tag whose `album_types.looked_up_at` is NULL (or NULL type and older
  than 30 days), in this order: (1) albums with `present ≤ 3` that have a `mb_releasegroupid` or a song with
  `mb_trackid` (the likely One-offs), (2) other albums with a `mb_releasegroupid`. Lookup: rgid → `mb::release_group`
  (primary + secondaries; release_tracks = max(tracktotal, max track number) since the group has no single count);
  no rgid but a recording id → `mb::recording` (cached answer shared with imports), the release whose
  `title_key` equals the album's (else the first) → its group's types and track count. A busy/failed call stamps
  nothing (tried next run); "not found" stamps `looked_up_at` with `release_type NULL`.
- Writes go in batches of 25 albums per transaction (one `library_version` move per batch, not one per second).
- Result line (Settings → Tasks): `"{a} song tags read, {b} albums looked up ({c} not on MusicBrainz)"`; one activity
  line per run when b > 0.

---------------------------------------------------------------------------------------------------------------------

## 4. API (all JSON; Admin routes use the `Admin` extractor = CSRF + admin role)

### 4.1 Lists

`GET /api/library/albums` – existing params plus:
- `type=` one filter key or a comma list (`album`, `live`, … `singles`, `one_off`, `other`; a format key `ep`/`single`
  is accepted as its own set). **When `type` is given it alone decides which albums show** (the `kind` tab rule below is
  not applied); unknown keys → 400 "Unknown album type".
- `kind=` keeps its keys but now means formats: `albums` → format `album` (any kind, not one-off); `singles` → format
  `single` or `ep`; empty → every album **except One-offs**. With `q=` (a search) and no `type`, One-offs are included
  (a search finds everything; the web's `album_kind` already drops `kind` when searching).
- `ids_only=1` → `{"total": N, "ids": [...]}` for the whole filtered list in list order (no paging), at most 5000 ids
  (`"capped": true` when cut). Same cache, no extra query.
- Each item gains `"type": "<filter key>"`. Nothing else changes (tiles, enrich fixtures).
- Filtering happens in memory on the cached list (the SQL and its cache key are unchanged except `HAVING COUNT(*)`
  goes: the kind/type sets replace it).

`GET /api/library/index` (side index) takes the same `type=` and uses the same sets for `kind`/`album_kind`
(`sections.rs` selects `album_id` too and drops the albums the set excludes before labelling), so offsets match.

`GET /api/library/album-types?artist=&filter=` (any user) →
`{"total": 1234, "types": [{"key": "album", "label": "Studio albums", "count": 812}, … in §3.1 order, count 0 included]}`
over every album (One-offs included) under the same `artist`/`filter`. Clients show `"{label} ({count})"`, hide
entries with count 0 (except the selected one), and put "All types" first.

`GET /api/library/artists/{name}/one-offs?name=&mbid=&offset=&limit=` (any user; beside `liked` and `tracks` in
`library_artist.rs`, same artist matching) → `{"total", "offset", "tracks": [TRACK_COLS + "release_label"]}`: the
artist's songs whose album is a One-off, newest `added_at` first, limit 1–500 (default 50), enriched.

`GET /api/discover/artist/{mbid}`: `library_albums` leaves out One-off albums (they are on the shelf instead).

### 4.2 Album page

`GET /api/library/albums/{id}` gains (stereo albums only):
```json
"type": {"key": "live", "label": "Live · EP", "format": "ep", "kind": "live",
         "chosen": null, "auto": {"format": "ep", "kind": "live", "label": "Live · EP"},
         "why": "musicbrainz", "why_text": "From MusicBrainz"},
"history": [{"via": "request", "by": "alex", "by_deleted": false, "requested_at": 1729…, "songs": 12}],
"added_at": 1729…
```
plus the per-track fields of §2.2.

`PUT /api/library/albums/{id}/type` (Admin) body `{"format": "album" | "ep" | "single" | "one_off" | null}` (null =
Automatic) → 200 the `type` object above; 400 "Pick Album, EP, Single or One-off"; 404 "Album not found" (no songs
with that id). Never asks MusicBrainz.

### 4.3 Bulk delete to Trash

`POST /api/library/albums/trash` (Admin) body `{"ids": ["<album_id>", …], "dry_run": true|false}`.
- `ids`: 1–5000 for a dry run, 1–100 for a real run (400 "At most 100 albums at a time" / "Pick at least one
  album"). Unknown ids are ignored and named in `missing`.
- Dry run (touches nothing) →
  `{"albums": 12, "songs": 140, "bytes": 3650722201, "size": "3.4 GB", "text": "Delete 12 albums (3.4 GB)?",
    "skipped": [{"id", "album", "reason"}], "skipped_text": "", "other_disk": true, "room": true, "room_text": "",
    "missing": []}`
  - `albums/songs/bytes` count only what can go (skipped ones excluded); `bytes` = SUM(tracks.size).
  - `text`: `Delete 1 album ({size})?` / `Delete {n} albums ({size})?`; `size` = the web's `ui::fmt::size` exactly
    (1024 steps, `"{v:.1} MB"` from MB up, `"{n} KB"`/`"{n} B"` below), as a new `encore_core::text::size_text`.
  - `skipped_text`: "" or `1 of them can't be moved to the Trash and stays: {first reason}` / `{k} of them can't be
    moved to the Trash and stay: {first reason}`. Reasons: "Its files are outside the folders Encore manages" (no file
    manager root holds them), "Its folder holds an extra library folder".
  - `room`/`room_text`: when the Trash is on another disk and `bytes + 1 GiB > free bytes of the Trash's disk`:
    `room=false`, `room_text = "There isn't room in the Trash for {size} ({free} free on its disk). Empty the Trash
    first, or delete fewer albums."` (clients then show room_text and no Move button).
- Real run → `{"albums": 12, "songs": 140, "bytes": …, "items": ["1729…-1", …], "failed": [{"id", "album",
  "reason"}], "text": "Moved 12 albums to the Trash"}`; 507 with room_text when the room check fails at that moment.
  All albums of the call run on the blocking pool under `encore_files::lock()`.
- `DELETE /api/library/albums/{id}` (existing, Admin) now does the same for one album (to the Trash, not
  `remove_file`) → `{"ok": true, "text": "Moved 1 album to the Trash", "items": [...]}`.
- One activity line per call: `{user} moved {n} albums to the Trash ({songs} songs, {size})` beside the file
  manager's own audit lines.

Clients: dry run over the whole selection → confirm → real runs in chunks of 100 in order, then one final toast built
client-side from the sums (§6.4).

---------------------------------------------------------------------------------------------------------------------

## 5. Bulk delete – how albums go to the Trash

### 5.1 What one album becomes in the Trash

For each album (its rows' paths resolved, `roots::all` + `roots::holding`):
1. Album folder = the deepest common folder of its songs' folders (multi-disc `Album/CD1`, `Album/CD2` → `Album`).
2. **One Trash item for the folder** when: it lies strictly inside a file manager root; it is not the library folder,
   an extra library folder (`library_extra_roots`) or a root itself; it holds no extra library folder; and every
   audio file under it (`names::is_audio`) is a row of an album in **this call's** set (two selected albums sharing a
   folder → that folder once). Non-audio files in it (cover.jpg, .lrc, .nfo) go with it.
3. Otherwise **one Trash item per song file**, with its same-stem `.lrc` / `.txt` moved into the same item as extras
   (below); afterwards `prune_empty(folder, stop = the root or extra library folder holding it)`.
4. No root holds the files → skipped with "Its files are outside the folders Encore manages".
Then `st.library.forget_cover(Some(id))` for every album that went.

### 5.2 Changes in `files/src/trash.rs` / `rows.rs`

- `trash_one(st, who, t, dir, name, change, extras: &[String])`: after the main entry, each extra name in the same
  `dir` is moved into the item folder; the note gets `"extras": [{"name", "stored"}]` and `size` includes them.
  `restore_one` moves the extras back beside the restored entry (free names; a missing extra is skipped with a log
  line). `list` is unchanged. Existing callers pass `&[]`.
- `rows::saved_rows` also keeps, per row, `"keep": {added_at, added_by, added_via, requested_at, request_id, source,
  source_detail, source_url, downloaded_at, verify_status, verify_detail, verified_at, acoustid_id}`;
  `rows::give_back` writes them back after the restore re-read the file (so a restored song is again "Requested by
  alex on Oct 13", keeps its source line and its verification). Notes written before 0.36 have no `keep`: nothing
  changes for them.
- `rows::Change::apply` removals also call `encore_library::surround::stereo_gone(c, &ids, None)` in the same
  transaction (the surround versions of trashed songs lose their stereo link, as `delete_tracks` does).
- The library's albums go through the same `trash_one`/`Change` code as the Files page: one code path, one audit.

### 5.3 Restore

Files → Trash → Restore (unchanged UI) puts the folder or file back, the rows come back with their old ids, playlist
entries, likes and the `keep` columns; the album id is the same (same tags), so its `album_types` row (choice, facts)
applies again.

---------------------------------------------------------------------------------------------------------------------

## 6. Clients (web-rs first – every string below must exist in web-rs; desktop and phone use the same words)

### 6.1 Library → Albums (web `web-rs/src/pages/library.rs`, desktop `clients/desktop/src/pages/library.rs`, phone `ui/screens/LibraryScreen.kt AlbumsTab`)

- Tabs (web/desktop): the "Singles" tab is labelled **Singles & EPs** (key stays `singles`; it lists format single/ep).
  The Albums tab lists format `album`; neither lists One-offs.
- **Type** dropdown (everyone; web `<select id="lib-type">` next to `#lib-filter`, desktop `pick_list`, phone a
  "Type" chip with a dropdown) on the Albums tab (web/desktop) / the albums list (phone): first option **All types**,
  then the server's `types` as `"{label} ({count})"` (count 0 hidden unless selected). Picking one sends `type=`; the
  value is kept in the address (web `#/library?type=live`, `library_hash` gains `type`), in desktop state and phone
  `LibraryState.type`. Counts come from `GET /api/library/album-types` (again after a delete or a type change).
  Empty list with a Type picked: `No albums of this type` (the usual "No albums found" otherwise).
- **Select** (admins only; web button `id="lib-select"` with the check icon, desktop button, phone: long-press a tile
  or the "Select" chip). In select mode:
  - a tile tap toggles it (no navigation); a check mark on selected tiles; web/desktop Esc leaves, desktop Ctrl+A =
    Select all, Delete key = Delete…; phone back leaves.
  - the bar (web: replaces the filter row, sticky; desktop: a row above the grid; phone: contextual top bar) shows
    **`{n} selected`** (`1 selected`), **Select all {N}** (N = the filtered list's total, ids from `ids_only=1`; over
    5000 → selects 5000 and toasts `At most 5000 albums at a time`), **Clear selection**, **Delete…** (danger,
    disabled at 0), **Done**.
  - The selection survives Load more / scrolling; changing tab, Type, filter, sort or search clears it.
- **Delete…** → dry run → dialog:
  - title = server `text` (`Delete 12 albums (3.4 GB)?`)
  - body: `Their songs go to the Trash in Files, where you can restore them. The space is freed when the Trash is
    emptied.` + when `other_disk`: ` The Trash is on another disk, so the files are copied there first – this can take
    a while.` + `skipped_text` on its own line when not empty.
  - buttons **Move to Trash** (danger) / **Cancel**; when `room` is false: the body is `room_text` and only **Close**.
  - While running: `Moving to the Trash… {done} of {n}` (web/desktop in the bar, phone as a toast that updates).
  - Done: `Moved 1 album to the Trash` / `Moved {n} albums to the Trash`; with failures `Moved {k} of {n} albums to the
    Trash – {m} couldn't be moved: {first reason}`; with an **Open Trash** action where the toast takes one (web: link
    `#/settings?s=files&root=trash`; desktop: Files page on the trash root; phone: `Screen.Files` opened on Trash – add
    an optional start root to `Screen.Files`). Then the list, the Type counts and the side index reload; select mode ends.
- Offline (phone offline mode / web offline): no Select, no Type counts (the dropdown is hidden).

### 6.2 Album page (web `render_album`/`album_menu`, desktop `pages/library_album.rs`, phone `ui/screens/AlbumScreens.kt`)

- The chip row gains the type chip `type.label` (web `<span class="badge">`, title attr = `why_text` for admins;
  desktop badge with tooltip for admins; phone `Badge`).
- History line from `history` + `added_at` (§2.3/§2.4) replacing the client-side requesters join.
- ⋯ (admins): **Change type…** (after "Edit details", before "Change artwork…"). Dialog/sheet:
  title **Change type**, text `What is “{album}”?`, radio options **Album**, **EP**, **Single**, **One-off** (help
  under it: `A song or two from a compilation, remix album or soundtrack: shown on the artist's page under One-offs,
  not with the albums.`), **Automatic ({auto label} – {why_text})** e.g. `Automatic (Live · EP – From MusicBrainz)`;
  the current one is checked (Automatic when `chosen` is null). **Save** / **Cancel** → PUT → toast `Type saved` → the
  page reloads.
- ⋯ "Delete album from disk" becomes **Move album to Trash** (danger) with the dialog title `Delete 1 album ({size})?`
  (from a dry run of this one id) and the §6.1 body; done → `Moved 1 album to the Trash` and back to the Library.

### 6.3 Artist pages – One-offs shelf

Web `pages/discover.rs` (MB artist page `ArtistSongs` and `render_artist_by_name`), desktop `pages/artist.rs` +
`pages/library_artist.rs`, phone `ArtistCatalogScreen.kt` (both the MusicBrainz and the by-name page): after the
"In your library" albums, a section **One-offs** with the count (`h2` + faint count, as the other sections), the
line `Songs from compilations, remix albums and soundtracks where you have only a few songs.`, the first 10 songs
(row: title; sub-line `{album} · {release_label}`; tap plays the shelf as a list from that song; ⋯ = the song menu),
**Show all {N}** when there are more (loads 500). Data: `GET /api/library/artists/{name}/one-offs` (with `mbid=` on a
MusicBrainz page, `name=` as the other artist routes do). No section when total is 0.

### 6.4 Exact strings (one list for all three clients)

`Type` · `All types` · `{label} ({count})` · `No albums of this type` · `Select` · `{n} selected` ·
`Select all {N}` · `Clear selection` · `Delete…` · `Done` · `At most 5000 albums at a time` ·
`Their songs go to the Trash in Files, where you can restore them. The space is freed when the Trash is emptied.` ·
` The Trash is on another disk, so the files are copied there first – this can take a while.` · `Move to Trash` ·
`Cancel` · `Close` · `Moving to the Trash… {done} of {n}` · `Moved 1 album to the Trash` ·
`Moved {n} albums to the Trash` · `Moved {k} of {n} albums to the Trash – {m} couldn't be moved: {reason}` ·
`Open Trash` · `Singles & EPs` · `Change type…` · `Change type` · `What is “{album}”?` · `Album` · `EP` · `Single` ·
`One-off` · `A song or two from a compilation, remix album or soundtrack: shown on the artist's page under One-offs,
not with the albums.` · `Automatic ({label} – {why})` · `Save` · `Type saved` · `Move album to Trash` · `One-offs` ·
`Songs from compilations, remix albums and soundtracks where you have only a few songs.` · `Show all {N}` ·
the §2.4 texts · `added {day}` / `Added {day}` · `and {k} more`.
Server-made texts (clients show them as sent): the dry-run `text`, `skipped_text`, `room_text`, type labels,
`why_text`, error texts.

---------------------------------------------------------------------------------------------------------------------

## 7. Performance (20 000 songs, ~2 000 albums)

- **No MusicBrainz call on any request path**: album list, album page, type counts, one-offs, Change type, bulk
  delete and the ⋯ added line read only the database. MusicBrainz is asked only by imports (answers they fetch
  anyway) and the background pass (Background priority, ≤ 300 calls a run, ≤ 1/s by the shared line).
- **Classification cached**: `encore_library::album_types::Types` (in `Library`, like `artists`) holds
  `Arc<HashMap<album_id, Class>>` built once per `library_version` on the blocking pool from one query
  (`SELECT album_id, album, albumartist, mb_albumartistid, title, artist, track, mb_releasegroupid, releasetype,
  tracktotal, compilation FROM tracks`) + `SELECT * FROM album_types`; regexes compiled once (`LazyLock`). Budget:
  ≤ 100 ms release / ≤ 1 s debug for 20 000 songs; concurrent callers wait for one build (no double build).
- Album list: the cached SQL list (unchanged cost) → type stamped once into the cached array when it is built
  (the stamp is part of the cached value: both depend only on `library_version`) → filters are slices in memory
  (≤ 5 ms). `album-types` counts and `ids_only` reuse the same cached array.
- `album_types` writes move `library_version`; the background pass batches them (25 albums/transaction), imports
  already move it. Owner's choice: one write.
- Bulk delete: dry run = one `SELECT album_id, COUNT(*), SUM(size) … WHERE album_id IN (…)` per 500 ids + a folder walk
  only in the real run; real runs are renames on the same disk (copies on another disk, warned in the dialog).
- `/tracks/{id}/added`: one indexed row + one user row. Migration backfill: two UPDATEs, one table scan each after
  the first start.

---------------------------------------------------------------------------------------------------------------------

## 8. Files to touch (map)

Server: `db/migrations/0031_library_types.sql` (new), `db/src/lib.rs` (MIGRATIONS, SCHEMA_VERSION 31, LIBRARY_TABLES,
tests); `media/src/tags.rs` (read releasetype/tracktotal/compilation, write keys); `library/src/scan.rs` (COLS,
Added, index_files_as, store_file INSERT, delete_tracks inheritance); `library/src/album_types.rs` (new: Facts, Class,
classify, Types cache, scheduled pass, labels, why texts, tests); `library/src/lib.rs` (`Library.types`);
`library/src/tagger.rs` (index_paths_as, facts at import, releasetype/compilation tags, album_types carry on retag);
`library/src/sections.rs` (type sets); `library/src/lists.rs` (if TRACK_COLS needs nothing – it doesn't);
`http/src/mb.rs` (release: secondary + track_count; recording: release_type/secondary/track_count);
`config/src/release_types.rs` (nothing required; optionally a `releasetype_tag(primary, secondary)` helper);
`config/settings.toml` (task_types_enabled/minutes); `jobs/src/scheduler.rs` (task "types");
`api/src/scheduler.rs` (run it); `api/src/routes/library.rs` (albums type/ids_only, album route fields, PUT type,
POST trash, DELETE → trash, tracks/{id}/added, owns_song, album-types route); `api/src/routes/library_artist.rs`
(one-offs); `api/src/routes/library_index.rs` (type param); `api/src/routes/discover.rs` (library_albums without
one-offs); `api/src/routes/tagging.rs` (carry album_types on edit/retag); `api/src/routes/soulseek.rs` (via);
`api/src/enrich_tests.rs` (new GET routes in the "every answer enriched" list); `requests/src/worker/import.rs`
(record_origin); `requests/src/report.rs` (rekey copies added_*); `files/src/trash.rs`, `files/src/rows.rs`,
`files/src/upload.rs`, `files/src/import.rs`, `files/src/ops.rs` (Added, extras, keep, stereo_gone), plus
`files::backfill_added_from_log` and a library-albums trash function (`encore_files::albums::trash_albums(st, who,
ids, dry_run)`) called by the api route; `core` (`text::size_text`).
Web: `pages/library.rs` (Type dropdown, Select mode, Delete flow, Singles & EPs label, album chip, Change type,
history line, Move album to Trash, track menu info line), new `pages/library_types.rs` (pure helpers + tests),
`pages/discover.rs` (One-offs shelf), `ui.rs` (`MenuItem::info`).
Desktop: `pages/library.rs`, `pages/library_album.rs`, `pages/artist.rs`, `pages/library_artist.rs`,
`widgets/pop_menu.rs` (`MenuItem::info`), new `pages/library_types.rs` (pure helpers + tests), the track menu
(`widgets/track_row.rs` / wherever `track_menu` lives).
Phone: `ui/screens/LibraryScreen.kt`, `ui/screens/AlbumScreens.kt`, `ui/screens/ArtistCatalogScreen.kt`,
`ui/EncoreRoot.kt` (TrackMenu info line), `ui/components/Common.kt` (`MenuEntry.info`), `data/api/EncoreApi.kt`
(albums type/ids_only, album-types, trash, type PUT, added, one-offs), `data/api/Models.kt`, new
`features/library/AlbumTypesRules.kt` (+ `AlbumTypesRulesTest.kt`), `ui/Screen` (Files start root).
Docs: copy this contract to `docs/contracts/library-types.md`; release notes `docs/releases/0.36.0.md`.

---------------------------------------------------------------------------------------------------------------------

## 9. Tests to add

### 9.1 Server – classification (`library/src/album_types.rs` tests, pure, no files)
1. `musicbrainz_facts_decide_kind_and_format`: Album+[Live] → live/album; Single+[Remix] → remix/single; EP+[] →
   studio/ep; Album+[Compilation,Live] → compilation (first secondary); Album+[] titled "Live Through This" →
   studio/album; Broadcast+[] → other/album; Album+[Soundtrack] 20 of 20 present → soundtrack/album.
2. `releasetype_tags_are_parsed`: "album; live", "Album/Live", "album, compilation", "ep", "single\u{0}remix", "",
   "garbage" → (primary, secondaries) as expected; the album takes the most common non-empty value.
3. `titles_decide_without_facts`: "Rodeo (Live at Coachella)" live; "Live at Wembley" live; "Greatest Hits"
   compilation; "Astroworld (The Remixes)" remix; "Dune (Original Motion Picture Soundtrack)" soundtrack; "Early Demos"
   demo; "Rodeo (Instrumentals)" karaoke; "Song (Karaoke Version)" karaoke; "Live Through This", "Alive", "Coverage",
   "Rodeo (Deluxe)" studio.
4. `song_titles_share`: 8 of 10 "(Live)" → live; 5 of 10 → studio; 1 of 1 "Song - Live" → live; 7 of 10 "(… Remix)"
   → remix; MusicBrainz says Album+[] and 10/10 "(Live)" → live (share applies while kind is studio).
5. `various_artists_and_compilation_flag`: albumartist "Various Artists" / "VA" / MB VA id / compilation=1 on most
   songs → compilation.
6. `partial_releases_are_one_offs` (thresholds): VA 1 song with track no. 7 → one_off; compilation 3 of 6 → one_off;
   compilation 3 of 5 → compilation/album (3·2 > 5); compilation 4 of 30 → compilation/album (> 3 present);
   soundtrack 2 present, total unknown → one_off; remix album 1 of 12 → one_off; lone song "Blinding Lights" on
   "After Hours" with MB Album → one_off (`why partial`, "Only 1 of its 14 songs are here"); lone song on a Single
   whose title differs (B-side) → single; 1-song album titled as the song → single; 2 songs no facts, album title =
   one song → single; 3 songs no facts → album; " - EP" title 4 songs → ep.
7. `the_owners_choice_wins`: chosen album on a guessed one-off compilation → format album, kind compilation, type
   compilation, why chosen; chosen one_off on a studio album → one_off; chosen single on a 12-song album → single;
   `auto` still reports the guess.
8. `types_are_a_partition`: random fixture of 500 albums → every album has exactly one filter key; counts sum to the
   total; labels in §3.1 order; `type.label` examples ("Live · EP", "Compilation · One-off", "Album").
9. `why_texts`: every why key → its exact text, "Only 1 of its …" singular, unknown total wording.
10. `classify_20000_songs_quickly`: 20 000 synthetic rows / 2 000 albums → built < 1 s in debug (print ms); a second
    call at the same library version returns the same `Arc` (no rebuild); one write to `album_types` → rebuilt once.

### 9.2 Server – storage, import, scan, Trash
11. `db`: `migration_files_are_numbered_and_listed` passes with 31; new `schema_31_backfills_who_added`: an old-shape
    DB with tracks + requests (request present → request/user/created_at; request row deleted → request/NULL; kind url →
    link; slskd pick without mbid → soulseek; no request → scan; detail not JSON → no failure) and running the
    migrations twice changes nothing; `the_library_version_moves_only_for_writes_to_its_tables` includes
    `UPDATE album_types …`.
12. `media/tags.rs` (ffmpeg fixtures, skipped without ffmpeg): RELEASETYPE (FLAC), TXXX "MusicBrainz Album Type"
    (MP3), "----:com.apple.iTunes:MusicBrainz Album Type" (M4A), TCMP/cpil/COMPILATION, TRACKTOTAL read; writing
    `releasetype`/`compilation` round-trips.
13. `scan.rs`: a new file gets `added_via='scan'` and the three tag columns (`''`/0 when absent); a rescan after a tag
    change keeps `added_by/added_via/requested_at`; `index_files_as` with `Added{by: 7, via: "upload"}`;
    `delete_tracks(.., Some(new))` copies added_* onto a `scan` replacement only.
14. `album_types` pass (`server_tests/musicbrainz.rs` fake MB): step A reads tags in batches (library version moves
    once per 200); step B looks up candidates in the stated order, at Background priority, stamps not-found, leaves a
    busy answer unstamped, writes 25 per transaction, stops at 300; result line text.
15. `tagger`: album import with a MusicBrainz release stores facts (secondary, track count) and writes
    `releasetype`; `import_single` stores the recording's release facts and does **not** write a release-group id;
    retag moves facts; edit details carries `chosen` to the new album id.
16. `requests worker`: `record_origin` sets added_by/added_via/requested_at (request, url → link, detail via soulseek →
    soulseek); a request with user NULL keeps inherited values; a lossless upgrade replacing a song keeps "alex";
    report rekey keeps the original adder.
17. `files`: upload → `upload` + who; Files import → `import` + who; move into the library → `import`; trash + restore
    of a song keeps old id, playlist entry, `keep` columns; extras (.lrc) go and come back with their song; a pre-0.36
    note (no `keep`, no `extras`) still restores; `Change::apply` removal clears `surround_tracks.stereo_track_id`;
    `backfill_added_from_log` turns logged uploads/imports into upload/import with the user, once.

### 9.3 Server – API (`api/src/routes/*_tests.rs`, new `library_types_tests.rs`)
18. `/albums`: items carry `type`; `kind=albums` → format album only; `kind=singles` → single+ep; no kind → no
    one-offs; `q=` → one-offs included; `type=live`; `type=one_off`; `type=singles`; `type=bogus` → 400; `ids_only=1`
    returns every id of the filtered list beyond `limit`, capped at 5000; `/index` sections with `type=` have offsets
    that land on the same albums as the list pages.
19. `/album-types`: fixed order, exact labels, counts sum to the total, honours `artist`/`filter`.
20. `/albums/{id}`: `type` object, `history` groups (two requesters + an upload → three groups, ordered by songs),
    `requested_by` still filled after `POST /api/requests/clear` deleted the request (the owner's Mufasa case).
21. `PUT /albums/{id}/type`: member 403; bad value 400; unknown album 404; set → list and album page show it; null →
    automatic; a full rescan keeps it.
22. `POST /albums/trash`: member 403; 0 ids / 101 ids real / 5001 ids dry 400; dry run counts, bytes, exact `text`
    ("Delete 2 albums (1.5 MB)?", "Delete 1 album (…)?"), touches nothing; real run: an exclusive folder → one Trash
    item (with cover.jpg); a shared folder → one item per song with its .lrc; a folder that is an extra library folder
    → per song; files outside every root → skipped with the reason; rows gone, playlist entries kept missing; Trash
    list shows the items with `by`; restore → album back with the same id, type choice and "Requested by" line;
    room check → 507 with room_text; `DELETE /albums/{id}` now lands in the Trash.
23. `/tracks/{id}/added`: every via of §2.4 incl. deleted user and cleared request; surround id; 404.
24. `/artists/{name}/one-offs`: only one-off songs, `release_label`, `mbid=` matching, paging; discover artist page's
    `library_albums` has no one-offs.
25. `owns_song`: a member with "fix" can still confirm/remove their song after Clear finished; another member can't.
26. `no_musicbrainz_on_requests`: with `musicbrainz_url` pointing at a closed port, album list, album page,
    album-types, PUT type, one-offs, dry run and `/added` all answer and `st.mb().line_stats()` shows no call.
27. `enrich_tests::every_answer_is_already_enriched` lists the one-offs route; fixtures unchanged for existing routes.
28. scheduler: "types" is listed under Settings → Tasks with its name/description; settings keys exist in
    settings.toml (settings-schema test).

### 9.4 Clients
29. Parity table (copy §2.4 verbatim, DAY passed in as a string) in web `library_types.rs` tests, desktop
    `library_types.rs` tests and phone `AlbumTypesRulesTest.kt`; plus `history_line` cases (no groups → "Added DAY";
    one group → "… · added DAY"; four groups → "A · B · C · and 1 more · added DAY").
30. Web: `library_hash` keeps `type`; the albums URL builder adds `type=` and drops `kind` rules when a type is set;
    select state (toggle, select all from ids, clear on tab/type/filter/search change, survives Load more); chunking
    of 340 ids → 4 calls of ≤ 100; final toast texts (all moved / partial); confirm body with/without other disk;
    type option text "{label} ({count})" and hiding 0 counts; album menu has "Change type…" and "Move album to Trash"
    only for admins; track menu starts with the info line when the text is not empty; `MenuItem::info` is not clickable.
31. Desktop: the same pure tests; `pop_menu` info row draws without a message; Select mode messages (Ctrl+A, Delete,
    Esc); the Singles & EPs tab label.
32. Phone (JVM): `AlbumTypesRules` (added text, history line, selection, chunking, toast texts, type options);
    `MenuEntry(info = true)` not clickable; `EncoreApi` URL building for `type=`/`ids_only=1`; the phone strings check
    (`strings_check.py`) passes – every new phone string is in web-rs.
33. Live check (integrator, Playwright + the phone harness, after the build): Library → Type "Live" → Select → Select
    all → Delete… → Move to Trash → Files → Trash shows the albums → Restore one → album back with its "Requested by"
    line; Change type on a 1-song compilation album → Album → it appears in the Albums grid; artist page One-offs shelf.

---------------------------------------------------------------------------------------------------------------------

## 10. Order of work (suggested split)

S1 server storage + who-added (migration, scan/tags, record_origin, files Added, log backfill, `/added`, album route
history, owns_song) → S2 server types (album_types module, facts at import, mb.rs fields, pass, list/index/types/
one-offs/PUT routes) → S3 server Trash (files extras/keep/stereo_gone, trash_albums, routes) → W web → D desktop →
P phone (strings from web) → integrate (all checks + live) → reviews. S1–S3 touch `routes/library.rs` together: one
agent per file or sequential. The pending follow-up workflow (requests User filter, group Play→enqueue …) touches
`routes/requests.rs` and the Requests pages, not these files; land it first or merge by hand.

## 11. Decisions to confirm with the owner (built as stated unless told otherwise)

- One-off includes "a lone song (1–2 songs) of a larger release that isn't a single/EP" even when the release is a
  studio album (§3.3 format rule 2, third bullet). Without it, a 1-song studio "album" would show in the Albums grid
  (today the web hides ≤ 2-song albums from it).
- Kind filters (Live, Remixes …) do not include One-offs (they have their own entry); "Singles & EPs" lists studio
  singles/EPs, a live EP is under Live.
- The single-album ⋯ delete also goes to the Trash now (one behaviour for both); Remove artist stays a permanent delete.
- Possible follow-up (not in this build): `import_single` could tag a lone song with its studio album
  (`mb::album_for_recording`) instead of the recording's first release, so fewer one-offs are made at all.
- Noticed, not fixed: `tagger::retag_external` joins `requests` and so skips Deezer/Apple albums whose request was
  cleared.
