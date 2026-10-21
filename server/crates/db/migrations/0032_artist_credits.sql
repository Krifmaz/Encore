-- Schema 32 (0.36): every artist of a song. "Kanye West & Ty Dolla $ign" is two artists, not a third one.

-- the artists a file's ARTISTS / ALBUMARTISTS tags credit (MusicBrainz Picard and Encore write them), with their
-- MusicBrainz ids when the id tags pair up: JSON [{name, mbid?}]. '[]': read, the file has none; NULL: not read yet
-- (the album-types pass reads them for songs scanned before, album_types::read_missing_tags)
ALTER TABLE tracks ADD COLUMN artist_credit TEXT;
ALTER TABLE tracks ADD COLUMN albumartist_credit TEXT;
ALTER TABLE surround_tracks ADD COLUMN artist_credit TEXT;
ALTER TABLE surround_tracks ADD COLUMN albumartist_credit TEXT;

-- artist credits MusicBrainz gave for songs whose files only have the joined artist text ("A feat. B"):
-- `mbid` is a recording id (a song's credit) or a release id (its album's), credit as above. Kept apart from
-- the files' own tags, so a rescan doesn't lose them.
CREATE TABLE IF NOT EXISTS mb_credits (
    mbid TEXT PRIMARY KEY,
    credit TEXT NOT NULL
);

-- when each release's credits were last looked up (ok=0: the look-up failed), so look-ups don't repeat
CREATE TABLE IF NOT EXISTS mb_credit_lookups (
    release_id TEXT PRIMARY KEY,
    looked_up_at REAL NOT NULL,
    ok INTEGER NOT NULL DEFAULT 0
);
