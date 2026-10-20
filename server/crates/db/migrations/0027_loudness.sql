-- Schema 27 (0.31): each song's loudness for volume normalization, its sample rate and bit depth, and the
-- group listens kept through a restart.

-- the song's ReplayGain values in dB relative to the -18 LUFS reference (R128 tags are converted: r128 / 256 + 5)
-- and its peaks (linear, 1.0 = full scale); loudness_lufs is its integrated loudness. loudness_source says
-- where they came from: 'tags' (the file's ReplayGain or R128 tags; loudness_lufs = -18 - the track gain),
-- 'measured' (the "Measure loudness" task: ffmpeg's EBU R128 loudness and true peak; the track gain is
-- -18 - loudness_lufs), 'none' (the task could not measure the file: silence, unreadable), NULL (not looked
-- at yet: the task takes these, so it goes on where it stopped). The album gain and peak columns hold the
-- file's album tags; the album's value the apps get is worked out when the songs are read
-- (encore_library::loudness).
ALTER TABLE tracks ADD COLUMN rg_track_gain REAL;
ALTER TABLE tracks ADD COLUMN rg_track_peak REAL;
ALTER TABLE tracks ADD COLUMN rg_album_gain REAL;
ALTER TABLE tracks ADD COLUMN rg_album_peak REAL;
ALTER TABLE tracks ADD COLUMN loudness_lufs REAL;
ALTER TABLE tracks ADD COLUMN loudness_source TEXT;
-- what the scan reads from the file's audio properties: Hz (2822400 for DSD64; NULL for songs scanned before
-- 0.31 and files that do not say, until the "Measure loudness" task read their tags: then 0 when unknown, so
-- it reads them once) and bits (1 for DSD; NULL for lossy files and when unknown)
ALTER TABLE tracks ADD COLUMN sample_rate INTEGER;
ALTER TABLE tracks ADD COLUMN bit_depth INTEGER;
-- the "Measure loudness" task looks for the songs it has not looked at yet
CREATE INDEX IF NOT EXISTS idx_tracks_loudness ON tracks(loudness_source);

-- group listens (0.31), saved while `group_listen_keep` is on so they come back after a restart (paused at
-- the saved position, with the same members; groups idle for 12 hours are dropped). No token is ever saved.
-- id: the group's opaque id; code: the 6-character invite code; visibility: 'anyone' | 'link'; queue: JSON
-- array of the entries ({id (the song), qid, added_by {user, name, guest}}); idx: the current entry;
-- position: seconds into it; rev: the state's revision; data: anything else the group keeps (JSON object).
CREATE TABLE IF NOT EXISTS group_listens (
    id TEXT PRIMARY KEY,
    code TEXT NOT NULL,
    host_user_id INTEGER,
    visibility TEXT NOT NULL DEFAULT 'anyone',
    everyone_controls INTEGER NOT NULL DEFAULT 0,
    guests_allowed INTEGER NOT NULL DEFAULT 0,
    max_members INTEGER NOT NULL DEFAULT 20,
    queue TEXT NOT NULL DEFAULT '[]',
    idx INTEGER NOT NULL DEFAULT 0,
    position REAL NOT NULL DEFAULT 0,
    rev INTEGER NOT NULL DEFAULT 0,
    data TEXT NOT NULL DEFAULT '{}',
    started_at REAL NOT NULL,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_group_listens_code ON group_listens(code);

-- the members of a saved group: one row per (account, device) or guest. member: the membership's id
-- (what the apps name in "Remove from the group"); user_id for an account, guest_id for a guest (the other
-- one NULL); is_host: 1 for the host's membership; added_by: the account that added this member, when
-- someone did (NULL: joined by themselves).
CREATE TABLE IF NOT EXISTS group_listen_members (
    group_id TEXT NOT NULL REFERENCES group_listens(id) ON DELETE CASCADE,
    member TEXT NOT NULL,
    user_id INTEGER,
    guest_id TEXT,
    name TEXT NOT NULL DEFAULT '',
    device_id TEXT NOT NULL DEFAULT '',
    device_name TEXT NOT NULL DEFAULT '',
    is_host INTEGER NOT NULL DEFAULT 0,
    added_by INTEGER,
    joined_at REAL NOT NULL,
    PRIMARY KEY (group_id, member)
);
CREATE INDEX IF NOT EXISTS idx_group_listen_members_user ON group_listen_members(user_id);
