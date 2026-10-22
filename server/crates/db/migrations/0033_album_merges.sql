-- Schema 33 (0.36): albums merged into another (the same release from MusicBrainz and another source shown
-- as one album, encore_library::merge). A scan writes the songs of `from_id` under `to_id`. Files never move.
CREATE TABLE IF NOT EXISTS album_merges (
    from_id TEXT PRIMARY KEY,   -- the album id the tags make (scan::album_key)
    to_id TEXT NOT NULL,        -- the album it is shown as (never itself a from_id)
    merged_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_album_merges_to ON album_merges(to_id);
