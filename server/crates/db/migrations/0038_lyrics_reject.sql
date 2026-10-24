-- Schema 38: lyrics a report said are wrong for a song (⋯ → Report a problem → Wrong lyrics / Lyrics out of
-- sync). The audio stays as it is: the lyrics file next to the song is deleted and the next lookup
-- (encore_library::lyrics) skips every LRCLIB version whose text matches a row here, so other lyrics are picked.
CREATE TABLE IF NOT EXISTS lyrics_reject (
    track_id INTEGER NOT NULL,
    kind TEXT NOT NULL,                     -- words (any version with these words) | synced (this exact timing) | plain (these words without timing) | instrumental
    key TEXT NOT NULL,                      -- SHA-256 (hex) of the normalised text; '' for instrumental
    report_id INTEGER,                      -- the report that rejected them
    created_at REAL NOT NULL,
    PRIMARY KEY (track_id, kind, key)
);
-- a lyrics report's own statuses in reports.status: lyrics_searching → lyrics_found | lyrics_none (| dismissed,
-- which deletes its rows here again)
