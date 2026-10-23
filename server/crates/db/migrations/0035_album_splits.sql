-- Schema 35 (0.37): albums of one name split by release (encore_library::split). An artist can give several
-- albums the same title (Weezer's self-titled Blue, Green, Red … albums): their tags make one album id
-- (scan::album_key), so each other MusicBrainz release group, or year when the files have none, gets its own.
CREATE TABLE IF NOT EXISTS album_splits (
    album_id TEXT NOT NULL,     -- the album id the songs had (scan::album_key, after album_merges)
    part TEXT NOT NULL,         -- 'rg:<release group id>' or 'y:<year>'
    to_id TEXT NOT NULL,        -- the album those songs are now
    split_at REAL NOT NULL,
    PRIMARY KEY (album_id, part)
);

-- MusicBrainz's disambiguation of a release group ("Blue Album"): albums of one name are shown with it
CREATE TABLE IF NOT EXISTS rg_names (
    rg TEXT PRIMARY KEY,
    disambiguation TEXT NOT NULL DEFAULT '',
    looked_up_at REAL NOT NULL
);
