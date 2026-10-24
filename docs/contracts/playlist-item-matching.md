# Playlist items: where they came from, and matching by hand (schema 37)

A YouTube video names its uploader, which is often not the song's artist (a lyrics channel, a fan, a
label). Imports read the real artist and title from the video (`encore_library::video_title`), and an item
whose artist is still only the uploader is also matched by title and length
(`LibraryIndex::find_by_title`: same title and version, lengths within 3 s, one artist only).

## Item fields (`GET /api/playlists/{pid}` → `items[]`)

| field | meaning |
|---|---|
| `source_title` | the name the item had at its source (a video's own title, or what it was called before a match by hand); `null` when it is the same as `title` |
| `artist_guessed` | `1`: the artist is only the uploader's name (nothing else named one); `0`/`null` otherwise |
| `match_reason` | why it is linked: `"metadata match"`, `"same ISRC"`, `"same title and length (the uploader isn't the artist)"`, `"matched by hand"`, `"unlinked by hand"` … |

## Matching by hand

`PATCH /api/playlists/{pid}/items/{iid}` with `{"track_id": <library song id>}` links the item to that song
(the item takes the song's title, artist, album and length; its old name goes to `source_title`), and
`{"track_id": null}` unlinks a wrong match. → `{"ok": true, "duplicates_removed": n}` (linking can make the
item a copy of another item in the playlist, which is removed). "Re-check library" leaves items matched or
unlinked by hand alone. 404 when the item isn't in that playlist; 400 when the song isn't in the library.

The web app shows a link button on every row (search the library, click a song; "Not this song" unlinks).

## Duplicates

`GET /api/library/duplicates` groups a song downloaded from YouTube under its uploader's name with the
library's own copy (same title, version and length, one other artist). Such a group has `"review": true`
and the reason `"YouTube copy (the uploader was taken for the artist)"`; "Resolve all" skips it, so a copy
only goes when someone deletes it on its card.
