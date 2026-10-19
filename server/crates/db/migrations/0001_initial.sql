-- Schema 1–20: everything up to the database backups and the version stamp. Servers before them
-- left no version number (user_version 0), so this file stands for all of them: the tables as schema 20
-- had them, the columns earlier servers added one by one (an older database gets the ones it lacks),
-- and the indexes on those columns.
--
-- Every migration file runs on every start, in order, so each one is idempotent: CREATE … IF NOT EXISTS,
-- and an ADD COLUMN is skipped when the table has the column already (crates/db/src/lib.rs).

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

-- columns added before schema 20 (a table made by an older server lacks some of them)
ALTER TABLE tracks ADD COLUMN isrc TEXT;
ALTER TABLE tracks ADD COLUMN acoustid_id TEXT;
ALTER TABLE tracks ADD COLUMN verify_status TEXT;
ALTER TABLE tracks ADD COLUMN verify_detail TEXT;
ALTER TABLE tracks ADD COLUMN verified_at REAL;
ALTER TABLE tracks ADD COLUMN source TEXT;
ALTER TABLE tracks ADD COLUMN source_detail TEXT;
ALTER TABLE tracks ADD COLUMN source_url TEXT;
ALTER TABLE tracks ADD COLUMN request_id INTEGER;
ALTER TABLE tracks ADD COLUMN downloaded_at REAL;
ALTER TABLE playlists ADD COLUMN remote_ids TEXT DEFAULT '{}';
ALTER TABLE users ADD COLUMN auto_download INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN prefs TEXT NOT NULL DEFAULT '{}';
ALTER TABLE users ADD COLUMN perms TEXT NOT NULL DEFAULT '{}';
-- added by later servers before the stamp: two-factor sign-in, avatars
ALTER TABLE users ADD COLUMN totp_secret TEXT;
ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN avatar_at REAL;
-- app tokens (bearer sign-in for the native app): web sessions keep kind='web'
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'web';
ALTER TABLE sessions ADD COLUMN device_id TEXT;
ALTER TABLE sessions ADD COLUMN device_name TEXT;
ALTER TABLE sessions ADD COLUMN last_seen_at REAL;

CREATE INDEX IF NOT EXISTS idx_tracks_isrc ON tracks(isrc);
CREATE INDEX IF NOT EXISTS idx_tracks_acoustid ON tracks(acoustid_id);
CREATE INDEX IF NOT EXISTS sessions_user_device ON sessions(user_id, device_id);
CREATE TABLE IF NOT EXISTS login_links (digest TEXT PRIMARY KEY, user_id INTEGER NOT NULL, expires_at REAL NOT NULL);
