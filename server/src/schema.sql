CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);

CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT UNIQUE COLLATE NOCASE NOT NULL,
    email TEXT,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    disabled INTEGER NOT NULL DEFAULT 0,
    auto_approve INTEGER NOT NULL DEFAULT 0,
    request_limit INTEGER,
    invite_id INTEGER,
    created_at REAL NOT NULL,
    last_login REAL
);

CREATE TABLE IF NOT EXISTS sessions (
    token TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    created_at REAL NOT NULL,
    expires_at REAL NOT NULL,
    user_agent TEXT,
    ip TEXT,
    -- 'web' (cookie; token is the raw cookie value) or 'app' (bearer; token is "sha256:" + hex digest,
    -- the raw token is shown to the app once). App sessions slide: expires_at moves on with use.
    kind TEXT NOT NULL DEFAULT 'web',
    device_id TEXT,      -- stable per app install (a UUID the app generates)
    device_name TEXT,    -- "Pixel 9 Pro"
    last_seen_at REAL
);
-- index sessions_user_device ON sessions(user_id, device_id) is created in db.rs::migrate (after old
-- databases got the column)

CREATE TABLE IF NOT EXISTS invites (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    code TEXT UNIQUE NOT NULL,
    created_by INTEGER,
    role TEXT NOT NULL DEFAULT 'user',
    max_uses INTEGER NOT NULL DEFAULT 1,
    uses INTEGER NOT NULL DEFAULT 0,
    expires_at REAL,
    note TEXT,
    created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER,
    kind TEXT NOT NULL,             -- artist | album | track | url
    mbid TEXT,                      -- artist / release-group / recording id
    artist TEXT,
    title TEXT,
    album TEXT,
    year TEXT,
    cover_url TEXT,
    url TEXT,
    status TEXT NOT NULL,
    source TEXT,
    progress REAL DEFAULT 0,
    message TEXT,
    detail TEXT DEFAULT '{}',
    parent_id INTEGER,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_requests_status ON requests(status);

CREATE TABLE IF NOT EXISTS tracks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT UNIQUE NOT NULL,
    album_id TEXT,
    mtime REAL, size INTEGER,
    title TEXT, artist TEXT, album TEXT, albumartist TEXT,
    track INTEGER, disc INTEGER, year TEXT, genre TEXT,
    duration REAL, bitrate INTEGER, format TEXT,
    mb_trackid TEXT, mb_albumid TEXT, mb_releasegroupid TEXT,
    mb_artistid TEXT, mb_albumartistid TEXT,
    added_at REAL
);
CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album_id);
CREATE INDEX IF NOT EXISTS idx_tracks_rg ON tracks(mb_releasegroupid);
CREATE INDEX IF NOT EXISTS idx_tracks_rec ON tracks(mb_trackid);

CREATE TABLE IF NOT EXISTS dup_ignore (key TEXT PRIMARY KEY, created_at REAL);

CREATE TABLE IF NOT EXISTS playlists (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    kind TEXT NOT NULL DEFAULT 'playlist',   -- playlist | liked
    source TEXT,                             -- spotify | deezer | tidal | youtube | ... | merged | manual
    source_url TEXT,
    follow INTEGER NOT NULL DEFAULT 0,       -- re-import from source periodically
    sync_targets TEXT NOT NULL DEFAULT '[]', -- ["jellyfin","plex","navidrome","listenbrainz"]
    cover_url TEXT,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS playlist_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    title TEXT, artist TEXT, album TEXT, duration REAL,
    isrc TEXT, mbid TEXT, cover_url TEXT,
    source TEXT, source_id TEXT,
    track_id INTEGER,                        -- matched library track
    match_reason TEXT,
    request_id INTEGER,
    added_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pli_pl ON playlist_items(playlist_id, position);

CREATE TABLE IF NOT EXISTS connections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    service TEXT NOT NULL,
    account TEXT,
    data TEXT NOT NULL DEFAULT '{}',
    sync_likes INTEGER NOT NULL DEFAULT 1,
    sync_playlists INTEGER NOT NULL DEFAULT 1,
    last_sync REAL,
    last_error TEXT,
    created_at REAL NOT NULL,
    UNIQUE(user_id, service)
);

-- last state seen on each service, to turn diffs into likes/unlikes
CREATE TABLE IF NOT EXISTS sync_state (
    connection_id INTEGER NOT NULL,
    scope TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (connection_id, scope)
);

CREATE TABLE IF NOT EXISTS loc (kind TEXT NOT NULL, original TEXT NOT NULL, lang TEXT NOT NULL, value TEXT NOT NULL, source TEXT,
    PRIMARY KEY (kind, original, lang));
CREATE TABLE IF NOT EXISTS task_runs (key TEXT PRIMARY KEY, last_run REAL, duration REAL, result TEXT, error TEXT);
CREATE TABLE IF NOT EXISTS plays (
    id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, track_id INTEGER, artist TEXT, title TEXT, album TEXT,
    seconds REAL NOT NULL DEFAULT 0, played_at REAL NOT NULL);
CREATE INDEX IF NOT EXISTS idx_plays_user ON plays(user_id, played_at);
CREATE TABLE IF NOT EXISTS follows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    mbid TEXT NOT NULL,
    name TEXT NOT NULL,
    types TEXT NOT NULL DEFAULT '["Album","EP"]',
    seen TEXT NOT NULL DEFAULT '[]',        -- release groups already handled
    last_checked REAL,
    last_found REAL,
    found_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at REAL NOT NULL,
    UNIQUE(user_id, mbid)
);

CREATE TABLE IF NOT EXISTS password_resets (
    user_id INTEGER PRIMARY KEY,
    digest TEXT NOT NULL,
    expires_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS shares (
    token TEXT PRIMARY KEY,
    track_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts REAL NOT NULL,
    level TEXT NOT NULL,
    message TEXT NOT NULL
);

-- albums whose tags someone fixed by hand: automatic tagging leaves them alone
CREATE TABLE IF NOT EXISTS manual_albums (
    key TEXT PRIMARY KEY,
    edited_at REAL
);

-- where each user last listened (track, position, queue), so another device can pick up from there
CREATE TABLE IF NOT EXISTS playback_spot (
    user_id INTEGER PRIMARY KEY,
    track_id INTEGER,
    position REAL,
    queue TEXT,                             -- JSON array of track ids
    idx INTEGER,
    device_id TEXT,
    device_name TEXT,
    playing INTEGER,
    updated_at REAL
);

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

-- each album cover's main colour ("#rrggbb"), painted while the cover loads; v is the cover's version
-- (covers.rs), so a new cover gets a new colour. Filled by the cover thumbnail pass.
CREATE TABLE IF NOT EXISTS cover_color (
    album_id TEXT PRIMARY KEY,
    v INTEGER NOT NULL,
    color TEXT NOT NULL
);

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
