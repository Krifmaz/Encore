-- Schema 24: release types on requests (kept in the request's detail), Report a problem with its
-- source blocks and parked files, the quality checks, and contributions to AcoustID and MusicBrainz.

-- giving back (contribute.rs): AcoustID fingerprint submissions for songs whose MusicBrainz recording the
-- audio confirmed, each fingerprint + recording sent once (status queued → submitted → imported | error)
CREATE TABLE IF NOT EXISTS contrib_acoustid (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id INTEGER,
    fp_hash TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    duration INTEGER NOT NULL,
    mbid TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    submission_id INTEGER,
    acoustid TEXT,
    error TEXT,
    created_at REAL,
    submitted_at REAL,
    checked_at REAL,
    UNIQUE (fp_hash, mbid)
);
CREATE INDEX IF NOT EXISTS idx_contrib_acoustid_status ON contrib_acoustid(status);

-- ISRCs sent to MusicBrainz for audio-confirmed recordings that lacked them (status queued → submitted | skipped | error)
CREATE TABLE IF NOT EXISTS contrib_isrc (
    recording TEXT NOT NULL,
    isrc TEXT NOT NULL,
    track_id INTEGER,
    status TEXT NOT NULL DEFAULT 'queued',
    error TEXT,
    created_at REAL,
    submitted_at REAL,
    PRIMARY KEY (recording, isrc)
);

-- report a problem (routes/report.rs): what someone said was wrong with a song or album, and what Encore
-- did about it (status filing → replacing → replaced | no_replacement | dismissed; filing while the files are parked)
CREATE TABLE IF NOT EXISTS reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER,
    kind TEXT NOT NULL,                     -- track | album
    track_id INTEGER,
    album_id TEXT,
    reason TEXT NOT NULL,                   -- wrong_song | wrong_version | bad_audio | loudness | fake_lossless | glitches | other
    note TEXT,
    artist TEXT, title TEXT, album TEXT,
    source TEXT,                            -- where the reported file came from
    status TEXT NOT NULL DEFAULT 'replacing',
    request_id INTEGER,
    message TEXT,
    created_at REAL NOT NULL,
    updated_at REAL
);
CREATE INDEX IF NOT EXISTS idx_reports_request ON reports(request_id);

-- sources never to download from again: slskd user|path|size (or user|folder), torrent infohash, usenet
-- nzb guid, streamrip service:id, YouTube video id
CREATE TABLE IF NOT EXISTS source_block (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,                     -- slskd | torrent | usenet | streamrip | youtube
    key TEXT NOT NULL,
    label TEXT,
    report_id INTEGER,
    user_id INTEGER,
    created_at REAL NOT NULL,
    UNIQUE (kind, key)
);

-- the exact source each downloaded file came from (what a report blocks)
CREATE TABLE IF NOT EXISTS track_origin (
    track_id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    label TEXT
);

-- reported songs waiting for their replacement: the library row (JSON) and where the file went in
-- <config>/trash; the replacement takes over the same track id (plays, likes, playlists stay)
CREATE TABLE IF NOT EXISTS track_parked (
    id INTEGER PRIMARY KEY,                 -- the track id
    report_id INTEGER,
    row TEXT NOT NULL,
    trash_path TEXT,
    parked_at REAL NOT NULL
);

-- quality checks per track (quality.rs): status ok | suspect | unchecked, issues JSON [{code, text}]
CREATE TABLE IF NOT EXISTS track_quality (
    track_id INTEGER PRIMARY KEY,
    status TEXT NOT NULL,
    issues TEXT NOT NULL DEFAULT '[]',
    summary TEXT,
    cutoff_hz REAL,
    lufs REAL,
    true_peak REAL,
    mtime REAL,
    checked_at REAL
);
