-- Schema 39: songs whose file someone replaced with their own (⋯ → Replace with my own file, or Library →
-- Upload your own files). One row per replacement, newest last: who, when, the file name they sent, and where
-- the file it replaced went (<config>/trash, kept 7 days like a reported song's). The song keeps its id, so
-- plays, likes, playlists and lyrics stay; the Signal path says the file is someone's own from these rows.
CREATE TABLE IF NOT EXISTS own_files (
    id INTEGER PRIMARY KEY,
    track_id INTEGER NOT NULL,
    user_id INTEGER,
    name TEXT,                              -- the file name as uploaded ("Song (my rip).flac")
    old_path TEXT,                          -- where the replaced file was
    trash_path TEXT,                        -- where it went ('' when there was no file to move)
    old_format TEXT,                        -- "mp3", "flac" … of the replaced file
    new_format TEXT,                        -- … and of the new one
    created_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS own_files_track ON own_files (track_id, created_at);
