-- Schema 21: where each listener left off, so another device can pick up from there.

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
