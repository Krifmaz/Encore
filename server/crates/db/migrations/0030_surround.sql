-- Schema 30 (0.35): the Surround library. Dolby Atmos and other multichannel versions of songs live in their own table
-- and folder (`surround_path`), apart from the stereo library: no query that reads `tracks` (lists, search, shuffle,
-- playlists, stats, duplicates, the quality and loudness tasks, sync) can see them. docs/contracts/surround.md.

-- The same columns as `tracks` (so the scanner writes either), plus what a surround file is and the stereo copy it
-- belongs to. Ids start at 1 000 000 000 (seeded below), so a surround id never equals a stereo one and the track
-- routes (/api/library/tracks/{id}/…) tell them apart by the number alone.
CREATE TABLE IF NOT EXISTS surround_tracks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT UNIQUE NOT NULL,
    album_id TEXT,
    mtime REAL, size INTEGER,
    title TEXT, artist TEXT, album TEXT, albumartist TEXT,
    track INTEGER, disc INTEGER, year TEXT, genre TEXT,
    duration REAL, bitrate INTEGER, format TEXT,
    mb_trackid TEXT, mb_albumid TEXT, mb_releasegroupid TEXT,
    mb_artistid TEXT, mb_albumartistid TEXT,
    added_at REAL,
    isrc TEXT, acoustid_id TEXT, verify_status TEXT, verify_detail TEXT, verified_at REAL,
    source TEXT, source_detail TEXT, source_url TEXT, request_id INTEGER, downloaded_at REAL,
    rg_track_gain REAL, rg_track_peak REAL, rg_album_gain REAL, rg_album_peak REAL,
    loudness_lufs REAL, loudness_source TEXT,
    sample_rate INTEGER, bit_depth INTEGER,
    codec TEXT,                              -- eac3 | ac3 | truehd | dts | flac | alac | aac | opus …
    profile TEXT,                            -- ffprobe's profile ("Dolby Digital Plus + Dolby Atmos")
    channels INTEGER,
    layout TEXT,                             -- "5.1(side)", "7.1" …
    atmos INTEGER NOT NULL DEFAULT 0,
    stereo_track_id INTEGER,                 -- the stereo copy of this song, when the library has one
    stereo_album_id TEXT                     -- the stereo album this surround album belongs to
);
INSERT INTO sqlite_sequence (name, seq) SELECT 'surround_tracks', 999999999
    WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'surround_tracks');
CREATE INDEX IF NOT EXISTS idx_surround_album ON surround_tracks(album_id);
CREATE INDEX IF NOT EXISTS idx_surround_stereo_track ON surround_tracks(stereo_track_id);
CREATE INDEX IF NOT EXISTS idx_surround_stereo_album ON surround_tracks(stereo_album_id);

-- A request is for the stereo library (as every request before 0.35) or the Surround one.
ALTER TABLE requests ADD COLUMN class TEXT NOT NULL DEFAULT 'stereo';
CREATE INDEX IF NOT EXISTS idx_requests_class ON requests(class, kind, mbid);

-- Whether Apple Music has a Dolby Atmos version of a stereo album or song (key "album:<id>" / "track:<id>"), so the
-- album page and the automatic fetch do not ask Apple every time.
CREATE TABLE IF NOT EXISTS surround_checks (
    key TEXT PRIMARY KEY,
    checked_at REAL NOT NULL,
    available INTEGER,
    result TEXT                              -- the Atmos result (JSON), what a request downloads
);
