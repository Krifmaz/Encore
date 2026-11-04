-- Schema 42: how much of a song each play heard (encore_library::taste::play_weight). A play is saved when it
-- counts (half the song, or 4 minutes), before the listener has finished it; when the song ends or the listener
-- moves on, the player tells how far it got (POST /api/taste/listened) and the recommendations weigh the play by
-- it: a song heard to its end counts more than one left soon after the halfway mark. NULL: not told (older plays,
-- players that don't send it), counted as an ordinary play.
ALTER TABLE plays ADD COLUMN listened REAL;
