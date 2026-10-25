-- Schema 41: a time-synced lyric's timing nudge (the player's lyrics view: Earlier / Later). One row per song,
-- shared by every device and listener, so the phone, the TV and the web app show the lines at the same moment.
-- The nudge belongs to the lyrics it was made on: `lyrics_key` names them, and other lyrics (a lyrics report
-- found new ones, the .lrc was replaced) start without a nudge again (encore_library::lyrics::shift_of).
CREATE TABLE IF NOT EXISTS lyrics_shift (
    track_id INTEGER PRIMARY KEY,
    shift_ms INTEGER NOT NULL,              -- how much later every line is shown (negative: earlier), ms
    lyrics_key TEXT NOT NULL,               -- SHA-256 (hex) of the time-synced text it was made on (lines trimmed, empty ones dropped)
    updated_at REAL NOT NULL
);
