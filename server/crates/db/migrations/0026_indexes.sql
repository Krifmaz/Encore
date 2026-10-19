-- Schema 26: lookups by name (is this album / song / artist in the library? artist pages, new
-- releases) used to read every song. 0.25.3 added the first eight without a version bump, so a schema 25
-- database may have them already; the last six are new: songs by artist (artist pages, car links,
-- for-you), requests by song and by user (catalog and discover pages look a request up per row; the
-- leaderboard counts per user), playlist rows and plays by song (deletes and the cover pass), plays by
-- time (recent plays).
CREATE INDEX IF NOT EXISTS idx_tracks_lalbum ON tracks(lower(album));
CREATE INDEX IF NOT EXISTS idx_tracks_ltitle ON tracks(lower(title));
CREATE INDEX IF NOT EXISTS idx_tracks_lalbumartist ON tracks(lower(albumartist));
CREATE INDEX IF NOT EXISTS idx_tracks_albumartist ON tracks(albumartist);
CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
CREATE INDEX IF NOT EXISTS idx_tracks_mb_albumartist ON tracks(mb_albumartistid);
CREATE INDEX IF NOT EXISTS idx_tracks_mb_artist ON tracks(mb_artistid);
CREATE INDEX IF NOT EXISTS idx_requests_mbid ON requests(mbid);
CREATE INDEX IF NOT EXISTS idx_tracks_lartist ON tracks(lower(artist));
CREATE INDEX IF NOT EXISTS idx_requests_kind_song ON requests(kind, lower(artist), lower(title));
CREATE INDEX IF NOT EXISTS idx_requests_user ON requests(user_id);
CREATE INDEX IF NOT EXISTS idx_pli_track ON playlist_items(track_id);
CREATE INDEX IF NOT EXISTS idx_plays_track ON plays(track_id);
CREATE INDEX IF NOT EXISTS idx_plays_time ON plays(played_at);
