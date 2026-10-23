-- Schema 36 (0.37): one song shown once (encore_library::twins). A song not tagged from MusicBrainz (a .txt or
-- playlist import, an untagged file) that is a song the library has from MusicBrainz – the same artist, title,
-- version and length – points at the copy that is shown; the song lists, search and shuffle leave it out.
-- Files never move and nothing is deleted: the Duplicates tool still lists both.
ALTER TABLE tracks ADD COLUMN copy_of INTEGER;
-- (the Surround library has every column of tracks: the scanner writes either)
ALTER TABLE surround_tracks ADD COLUMN copy_of INTEGER;
CREATE INDEX IF NOT EXISTS idx_tracks_copy_of ON tracks(copy_of) WHERE copy_of IS NOT NULL;

-- An artist's own release types (Settings → Downloads → Release types for that artist alone): what Request
-- discography, the artist page and New releases count for them. NULL: the server's setting.
CREATE TABLE IF NOT EXISTS artist_release_types (
    mbid TEXT PRIMARY KEY,      -- the MusicBrainz artist
    kinds TEXT,                 -- "Album,EP" (encore_config::release_types keys)
    skip TEXT,                  -- "Compilation,Live,…"
    updated_at REAL NOT NULL
);
