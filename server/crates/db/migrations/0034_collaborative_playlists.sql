-- Schema 34 (0.37): collaborative playlists. The owner turns Collaborative on and invites other accounts on
-- this server (by user name) or hands out an invite link; whoever joins can add, remove and reorder songs.
-- docs/contracts/collaborative-playlists.md.

ALTER TABLE playlists ADD COLUMN collaborative INTEGER NOT NULL DEFAULT 0;   -- 1: members may change the songs
ALTER TABLE playlists ADD COLUMN invite_token TEXT;                          -- the invite link's token (NULL: none)
CREATE UNIQUE INDEX IF NOT EXISTS idx_playlists_invite ON playlists(invite_token);

-- who was invited to, or joined, someone's playlist (never the owner)
CREATE TABLE IF NOT EXISTS playlist_collaborators (
    playlist_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'invited',  -- invited | joined
    invited_by INTEGER,                      -- users.id (NULL: joined from the invite link)
    created_at REAL NOT NULL,
    joined_at REAL,
    PRIMARY KEY (playlist_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_playlist_collaborators_user ON playlist_collaborators(user_id);
