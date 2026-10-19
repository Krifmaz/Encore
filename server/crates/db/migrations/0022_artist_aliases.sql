-- Schema 22: other names of library artists, and album and artist share links.

-- other names of library artists (MusicBrainz aliases, sort names, earlier names: Ye <-> Kanye West).
-- artist_key / alias_key are artists::key() of the names; filled in the background after scans
CREATE TABLE IF NOT EXISTS artist_alias (
    artist_key TEXT NOT NULL,
    alias TEXT NOT NULL,
    alias_key TEXT NOT NULL,
    PRIMARY KEY (artist_key, alias_key)
);
CREATE INDEX IF NOT EXISTS idx_artist_alias_key ON artist_alias(alias_key);

-- when each artist's aliases were last looked up (ok=0: the lookup failed), so lookups don't repeat
CREATE TABLE IF NOT EXISTS artist_alias_lookups (
    artist_key TEXT PRIMARY KEY,
    mbid TEXT,
    looked_up_at REAL NOT NULL,
    ok INTEGER NOT NULL DEFAULT 0
);

-- album and artist share links: track_id stays for songs (0 for the others)
ALTER TABLE shares ADD COLUMN kind TEXT DEFAULT 'track';
ALTER TABLE shares ADD COLUMN ref TEXT;
