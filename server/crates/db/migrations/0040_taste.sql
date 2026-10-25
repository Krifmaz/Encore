-- Schema 40: what each listener tells the recommendations (encore_library::taste) beyond plays and likes.

-- dislikes: a song (kind 'track', ref = the song's id) or a whole artist (kind 'artist', ref = the artist's name,
-- lower-cased). A disliked song or artist is never queued by a shuffle, a mix or "More like this" again, and pulls
-- down what sounds like it (its genres and era).
CREATE TABLE IF NOT EXISTS dislikes (
    user_id INTEGER NOT NULL,
    kind TEXT NOT NULL,                     -- track | artist
    ref TEXT NOT NULL,                      -- the song's id as text, or the artist's lower-cased name
    name TEXT,                              -- what to show: "Artist – Title" for a song, the artist's name as written
    created_at REAL NOT NULL,
    PRIMARY KEY (user_id, kind, ref)
);

-- skips: a song the listener moved on from early (Next before the play counted). A skip is a mild "not now";
-- several skips of one song in a few weeks count almost like a dislike.
CREATE TABLE IF NOT EXISTS skips (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    track_id INTEGER NOT NULL,
    seconds REAL NOT NULL DEFAULT 0,        -- how far into the song the skip came
    skipped_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_skips_user ON skips(user_id, skipped_at);
