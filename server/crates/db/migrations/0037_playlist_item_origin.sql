-- Schema 37: where a playlist item's artist came from, and the name it had at its source.
-- A YouTube video often names its uploader, not the song's artist (fan uploads, label channels): an item
-- whose artist is only a guess is also matched by title and length (encore_sync::playlists::find_item).
ALTER TABLE playlist_items ADD COLUMN artist_guessed INTEGER;  -- 1: the artist is the uploader's name, not a tag or the title's
ALTER TABLE playlist_items ADD COLUMN source_title TEXT;       -- the video's own title, as the source shows it (NULL: same as title)
