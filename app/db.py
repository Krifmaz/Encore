"""Tiny thread-safe SQLite helper."""
import json
import os
import sqlite3
import threading
import time

DATA_DIR = os.environ.get("ENCORE_CONFIG_DIR", "/config")
DB_PATH = os.path.join(DATA_DIR, "encore.db")

_lock = threading.RLock()
_conn: sqlite3.Connection | None = None

SCHEMA = """
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
    ip TEXT
);

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

CREATE TABLE IF NOT EXISTS activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts REAL NOT NULL,
    level TEXT NOT NULL,
    message TEXT NOT NULL
);
"""


def connect():
    global _conn
    os.makedirs(DATA_DIR, exist_ok=True)
    _conn = sqlite3.connect(DB_PATH, check_same_thread=False, isolation_level=None)
    _conn.row_factory = sqlite3.Row
    _conn.execute("PRAGMA journal_mode=WAL")
    _conn.execute("PRAGMA foreign_keys=ON")
    _conn.executescript(SCHEMA)
    migrate()


MIGRATIONS = {
    "tracks": [("isrc", "TEXT"), ("acoustid_id", "TEXT"), ("verify_status", "TEXT"), ("verify_detail", "TEXT"), ("verified_at", "REAL"),
               ("source", "TEXT"), ("source_detail", "TEXT"), ("source_url", "TEXT"), ("request_id", "INTEGER"), ("downloaded_at", "REAL")],
    "playlists": [("remote_ids", "TEXT DEFAULT '{}'")],
    "users": [("auto_download", "INTEGER NOT NULL DEFAULT 1"), ("prefs", "TEXT NOT NULL DEFAULT '{}'")],
}


def migrate():
    for table, cols in MIGRATIONS.items():
        have = {r["name"] for r in query(f"PRAGMA table_info({table})")}
        for name, typ in cols:
            if name not in have:
                execute(f"ALTER TABLE {table} ADD COLUMN {name} {typ}")
    execute("CREATE INDEX IF NOT EXISTS idx_tracks_isrc ON tracks(isrc)")
    execute("CREATE INDEX IF NOT EXISTS idx_tracks_acoustid ON tracks(acoustid_id)")


def query(sql: str, params=()) -> list[dict]:
    with _lock:
        return [dict(r) for r in _conn.execute(sql, params).fetchall()]


def one(sql: str, params=()) -> dict | None:
    rows = query(sql, params)
    return rows[0] if rows else None


def execute(sql: str, params=()) -> int:
    with _lock:
        cur = _conn.execute(sql, params)
        return cur.lastrowid


def executemany(sql: str, seq) -> None:
    with _lock:
        _conn.execute("BEGIN")
        try:
            _conn.executemany(sql, seq)
            _conn.execute("COMMIT")
        except Exception:
            _conn.execute("ROLLBACK")
            raise


def log(level: str, message: str) -> None:
    execute("INSERT INTO activity (ts, level, message) VALUES (?,?,?)", (time.time(), level, message))
    execute("DELETE FROM activity WHERE id < (SELECT MAX(id) - 2000 FROM activity)")


# ---- request helpers -------------------------------------------------------

def get_request(rid: int) -> dict | None:
    r = one("SELECT * FROM requests WHERE id=?", (rid,))
    if r:
        r["detail"] = json.loads(r["detail"] or "{}")
    return r


def update_request(rid: int, **fields) -> None:
    if "detail" in fields and not isinstance(fields["detail"], str):
        fields["detail"] = json.dumps(fields["detail"])
    fields["updated_at"] = time.time()
    cols = ", ".join(f"{k}=?" for k in fields)
    execute(f"UPDATE requests SET {cols} WHERE id=?", (*fields.values(), rid))
