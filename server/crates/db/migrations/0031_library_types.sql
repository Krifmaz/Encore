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
UPDATE surround_tracks SET
    added_by = (SELECT r.user_id FROM requests r WHERE r.id = surround_tracks.request_id),
    requested_at = (SELECT r.created_at FROM requests r WHERE r.id = surround_tracks.request_id),
    added_via = COALESCE((SELECT CASE
            WHEN r.kind = 'url' THEN 'link'
            WHEN r.mbid IS NULL AND json_valid(r.detail) AND json_extract(r.detail, '$.pick.source') = 'slskd'
                 AND json_extract(r.detail, '$.pick.payload.username') IS NOT NULL THEN 'soulseek'
            ELSE 'request' END
        FROM requests r WHERE r.id = surround_tracks.request_id), 'request')
    WHERE added_via IS NULL AND (request_id IS NOT NULL OR source IS NOT NULL OR downloaded_at IS NOT NULL);
UPDATE surround_tracks SET added_via = 'scan' WHERE added_via IS NULL;
